//! Connection sessions.
//!
//! Replaces `state.rs`, `background.rs`, `serial.rs` and the logging half of `port.rs` from
//! the Tauri build. Three things changed beyond dropping the Tauri types:
//!
//! * **One runtime, not one per port.** `background.rs` spawned an OS thread per connection
//!   and gave each its own `#[tokio::main]` runtime. Sessions are now tasks on one shared
//!   multi-threaded runtime.
//! * **Real async reads.** The old loop called `try_read`/`try_recv` in a 10 ms sleep loop,
//!   and treated a zero-byte read as `break` — which silently ended the session and
//!   returned `Ok(())`, leaving the UI showing a live connection. Reads now await, and every
//!   exit path reports why.
//! * **The buffer belongs to the tab.** Scrollback is passed in, not created here, so it
//!   survives disconnect and reconnect (plan task 3).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::discovery;
use crate::settings::{DisplayMode, SendMode, SerialSettings};
use crate::term::emu::{self, Emulator, TermSize};
use crate::term::input::InputModes;
use crate::term::{TermBuffer, DEFAULT_MAX_BYTES};

/// Size of each read from the port.
const READ_BUFFER: usize = 8192;
/// How often to check the port is still present.
const PRESENCE_INTERVAL: Duration = Duration::from_millis(500);

/// UI to session-task messages.
#[derive(Debug)]
enum Command {
    Send(Vec<u8>),
    SetLogging { enabled: bool, path: Option<PathBuf> },
}

/// Session-task to UI messages.
#[derive(Debug)]
enum Event {
    Connected,
    /// The session ended. `reason` is `None` for a clean, user-requested close.
    Closed { reason: Option<String> },
    /// Non-fatal problem, e.g. the log file could not be written.
    Warning(String),
}

/// Connection lifecycle.
///
/// The Tauri build tracked this as two loosely-related booleans (`is_active`, `is_running`).
/// A single enum makes the button states unambiguous and gives plan task 3 somewhere to add
/// `Reconnecting`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
}

/// One terminal tab: its settings, its scrollback, and its transport if connected.
pub struct Session {
    pub settings: SerialSettings,
    pub display_mode: DisplayMode,
    pub state: ConnectionState,
    /// Last failure, shown in the tab so errors are not lost to a dismissed dialog.
    pub last_error: Option<String>,

    /// Scrollback. Shared with the session task, which appends to it.
    pub buffer: Arc<Mutex<TermBuffer>>,
    pub max_bytes: usize,

    /// The terminal screen, present only while ANSI mode is selected.
    ///
    /// Kept out of the shared buffer deliberately: it is a derived view, rebuilt by replaying
    /// the ring, and it is only touched from the UI thread.
    emulator: Option<Emulator>,

    pub log_enabled: bool,
    pub log_path: Option<PathBuf>,

    pub send_text: String,
    pub send_mode: SendMode,
    pub append_cr: bool,
    pub append_lf: bool,
    /// Whether Return transmits CR+LF or CR alone while the terminal has focus.
    pub enter_crlf: bool,
    pub font_size: f32,
    /// Measured height of the controls strip, fed back each frame to lay out the tab.
    pub controls_height: f32,

    commands: Option<mpsc::UnboundedSender<Command>>,
    events: Option<mpsc::UnboundedReceiver<Event>>,
}

impl Session {
    pub fn new(settings: SerialSettings) -> Self {
        Self {
            settings,
            display_mode: DisplayMode::default(),
            state: ConnectionState::Disconnected,
            last_error: None,
            buffer: Arc::new(Mutex::new(TermBuffer::new(DEFAULT_MAX_BYTES))),
            max_bytes: DEFAULT_MAX_BYTES,
            emulator: None,
            log_enabled: false,
            log_path: None,
            send_text: String::new(),
            send_mode: SendMode::default(),
            append_cr: false,
            append_lf: false,
            enter_crlf: true,
            font_size: 13.0,
            controls_height: 150.0,
            commands: None,
            events: None,
        }
    }

    /// Tab label.
    pub fn title(&self) -> String {
        let name = if self.settings.name.is_empty() {
            "(no port)"
        } else {
            &self.settings.name
        };
        match self.state {
            ConnectionState::Connected => format!("● {name}"),
            ConnectionState::Connecting => format!("◌ {name}"),
            ConnectionState::Disconnected => name.to_string(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Connecting | ConnectionState::Connected
        )
    }

    /// Start the session task.
    pub fn connect(&mut self, rt: &Handle, ctx: &egui::Context) {
        if self.is_busy() {
            return;
        }
        if self.settings.name.is_empty() {
            self.last_error = Some("Select a port first.".to_owned());
            return;
        }

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();

        let settings = self.settings.clone();
        let buffer = Arc::clone(&self.buffer);
        let ctx = ctx.clone();
        let log = if self.log_enabled {
            self.log_path.clone()
        } else {
            None
        };

        rt.spawn(async move {
            run(settings, buffer, ctx, cmd_rx, evt_tx, log).await;
        });

        self.commands = Some(cmd_tx);
        self.events = Some(evt_rx);
        self.state = ConnectionState::Connecting;
        self.last_error = None;
    }

    /// Ask the session task to stop.
    ///
    /// Dropping the command sender closes the channel, which the task's `select!` notices
    /// even while it is parked on a read, so there is no need to abort the task.
    pub fn disconnect(&mut self) {
        self.commands = None;
        self.state = ConnectionState::Disconnected;
    }

    /// Drain task messages. Called once per frame.
    pub fn poll(&mut self) {
        let Some(events) = self.events.as_mut() else {
            return;
        };
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Connected => self.state = ConnectionState::Connected,
                Event::Closed { reason } => {
                    self.state = ConnectionState::Disconnected;
                    self.commands = None;
                    self.last_error = reason;
                }
                Event::Warning(message) => self.last_error = Some(message),
            }
        }
        if self.events.as_ref().is_some_and(|_| self.commands.is_none())
            && self.state == ConnectionState::Disconnected
        {
            self.events = None;
        }
    }

    /// Transmit bytes, if connected.
    pub fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let Some(commands) = self.commands.as_ref() else {
            return;
        };
        if commands.send(Command::Send(bytes)).is_err() {
            // The task is gone; reflect that rather than silently dropping input.
            self.state = ConnectionState::Disconnected;
            self.commands = None;
            self.last_error = Some("Lost connection to the port.".to_owned());
        }
    }

    /// Push the current log settings to a running session.
    pub fn apply_logging(&mut self) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(Command::SetLogging {
                enabled: self.log_enabled,
                path: self.log_path.clone(),
            });
        }
    }

    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.set_max_bytes(max_bytes);
        }
        // The emulator counts scrollback in lines, so its limit has to be re-derived and its
        // content rebuilt.
        if let Some(emulator) = self.emulator.as_mut() {
            emulator.set_history(emu::history_lines(max_bytes));
            self.replay();
        }
    }

    pub fn clear(&mut self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
        if let Some(emulator) = self.emulator.as_mut() {
            emulator.reset();
        }
    }

    /// The emulator, if ANSI mode is selected.
    pub fn emulator(&self) -> Option<&Emulator> {
        self.emulator.as_ref()
    }

    pub fn emulator_mut(&mut self) -> Option<&mut Emulator> {
        self.emulator.as_mut()
    }

    /// Terminal state that changes how keys are encoded.
    pub fn input_modes(&self) -> InputModes {
        self.emulator
            .as_ref()
            .map(|e| InputModes::from_term_mode(e.mode()))
            .unwrap_or_default()
    }

    /// Bring the emulator into line with the selected display mode, then feed it any bytes
    /// that have arrived. Called once per frame before drawing.
    pub fn sync_emulator(&mut self) {
        match self.display_mode {
            DisplayMode::Ansi => {
                if self.emulator.is_none() {
                    // Entering ANSI mode: build a screen and replay the ring through it. A
                    // terminal cannot be handed the tail of a stream and show a correct
                    // screen, so this replay is what makes mode switching work at all.
                    let size = TermSize::new(80, 24, emu::history_lines(self.max_bytes));
                    self.emulator = Some(Emulator::new(size));
                    self.replay();
                }
                self.feed_emulator();
            }
            // Leaving ANSI mode frees the grid; the ring still has everything needed to
            // rebuild it on the way back.
            _ => self.emulator = None,
        }
    }

    /// Rebuild the screen from the whole retained ring.
    fn replay(&mut self) {
        let Some(emulator) = self.emulator.as_mut() else {
            return;
        };
        let Ok(buffer) = self.buffer.lock() else {
            return;
        };
        emulator.reset();
        let (bytes, at) = buffer.slice_from(0);
        // Anything already trimmed from the ring is unrecoverable; start from what is left.
        emulator.skip_to(at);
        emulator.feed(bytes);
        // Replies produced while replaying are stale — the device already answered these
        // queries the first time round, so sending them again would confuse it.
        emulator.take_replies();
    }

    /// Feed bytes that arrived since the last frame, and transmit any replies.
    fn feed_emulator(&mut self) {
        let Some(emulator) = self.emulator.as_mut() else {
            return;
        };
        let replies = {
            let Ok(buffer) = self.buffer.lock() else {
                return;
            };
            let (bytes, at) = buffer.slice_from(emulator.fed_to());
            emulator.skip_to(at);
            emulator.feed(bytes);
            emulator.take_replies()
        };
        // Programs query the terminal and wait for an answer, so replies have to go back out.
        if !replies.is_empty() {
            self.send(replies);
        }
    }
}

/// The session task: open the port, then pump it until told to stop.
async fn run(
    settings: SerialSettings,
    buffer: Arc<Mutex<TermBuffer>>,
    ctx: egui::Context,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    log_path: Option<PathBuf>,
) {
    let builder = tokio_serial::new(&settings.name, settings.baud_rate)
        .data_bits(settings.data_bits.into())
        .flow_control(settings.flow_control.into())
        .parity(settings.parity.into())
        .stop_bits(settings.stop_bits.into());

    let mut port = match tokio_serial::SerialStream::open(&builder) {
        Ok(port) => port,
        Err(e) => {
            let _ = events.send(Event::Closed {
                reason: Some(format!("Unable to open {}: {e}", settings.name)),
            });
            ctx.request_repaint();
            return;
        }
    };

    let _ = events.send(Event::Connected);
    ctx.request_repaint();
    debug!("session open on {}", settings.name);

    let mut log = Logger::new(log_path, &events).await;
    let mut read_buffer = vec![0u8; READ_BUFFER];
    let mut presence = tokio::time::interval(PRESENCE_INTERVAL);
    presence.tick().await; // the first tick completes immediately

    let reason = loop {
        tokio::select! {
            read = port.read(&mut read_buffer) => match read {
                // A real end-of-stream. The old code treated this as `break` and reported
                // success, so the UI kept showing a connected port.
                Ok(0) => break Some("Port closed the connection.".to_owned()),
                Ok(count) => {
                    let data = &read_buffer[..count];
                    if let Ok(mut buffer) = buffer.lock() {
                        buffer.append(data);
                    }
                    log.write(data, &events).await;
                    ctx.request_repaint();
                }
                Err(e) => break Some(format!("Read failed: {e}")),
            },

            command = commands.recv() => match command {
                // Sender dropped: the user asked to disconnect.
                None => break None,
                Some(Command::Send(bytes)) => {
                    if let Err(e) = port.write_all(&bytes).await {
                        break Some(format!("Write failed: {e}"));
                    }
                }
                Some(Command::SetLogging { enabled, path }) => {
                    log = Logger::new(if enabled { path } else { None }, &events).await;
                }
            },

            _ = presence.tick() => {
                // Notice an unplugged adapter. Enumeration is a blocking syscall, so it
                // does not belong inline in an async task.
                let name = settings.name.clone();
                let present = tokio::task::spawn_blocking(move || discovery::port_present(&name))
                    .await
                    .unwrap_or(true);
                if !present {
                    break Some(format!("{} disappeared (device unplugged?).", settings.name));
                }
            }
        }
    };

    log.flush().await;
    debug!("session on {} closed: {reason:?}", settings.name);
    let _ = events.send(Event::Closed { reason });
    ctx.request_repaint();
}

/// Appends raw received bytes to a file.
///
/// The Tauri build logged the *formatted* output, so a hex-mode session wrote hex to disk
/// and the log's content depended on a display setting. Raw bytes are lossless and let the
/// log be replayed through any view.
struct Logger {
    file: Option<tokio::fs::File>,
}

impl Logger {
    async fn new(path: Option<PathBuf>, events: &mpsc::UnboundedSender<Event>) -> Self {
        let Some(path) = path else {
            return Self { file: None };
        };
        match tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await
        {
            Ok(file) => Self { file: Some(file) },
            Err(e) => {
                warn!("could not open log file {}: {e}", path.display());
                let _ = events.send(Event::Warning(format!(
                    "Could not open log file {}: {e}",
                    path.display()
                )));
                Self { file: None }
            }
        }
    }

    async fn write(&mut self, data: &[u8], events: &mpsc::UnboundedSender<Event>) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if let Err(e) = file.write_all(data).await {
            let _ = events.send(Event::Warning(format!("Log write failed: {e}")));
            // Stop trying after the first failure rather than reporting once per read.
            self.file = None;
        }
    }

    async fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush().await;
        }
    }
}
