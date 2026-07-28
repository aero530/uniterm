//! Connection sessions.
//!
//! Replaces `state.rs`, `background.rs`, `serial.rs` and the logging half of `port.rs` from
//! the Tauri build. Beyond dropping the Tauri types:
//!
//! * **One runtime, not one per port.** `background.rs` spawned an OS thread per connection
//!   and gave each its own `#[tokio::main]` runtime. Sessions are tasks on one shared
//!   multi-threaded runtime.
//! * **Real async reads.** The old loop polled `try_read` in a 10 ms sleep loop and treated a
//!   zero-byte read as `break`, silently ending the session while the UI still showed it
//!   connected. Reads await, and every exit path reports why.
//! * **The buffer belongs to the tab.** Scrollback is passed in, not created here, so it
//!   survives disconnect and reconnect (plan task 3).
//! * **Transport-agnostic.** The loop runs over [`transport::Transport`], so serial and SSH
//!   share one code path.

pub mod log;
pub mod ssh;
#[cfg(test)]
mod ssh_tests;
pub mod transport;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tracing::debug;

use crate::knownhosts::{self, Rejection};
use crate::settings::{
    ConnectionKind, ConnectionSettings, DisplayMode, SendMode,
};
use crate::term::emu::{self, Emulator, TermSize};
use crate::term::input::InputModes;
use crate::term::{TermBuffer, DEFAULT_MAX_BYTES};

use log::Logger;
use transport::{Incoming, Transport};

/// How often to run the transport's liveness check.
const LIVENESS_INTERVAL: Duration = Duration::from_millis(500);

/// UI to session-task messages.
enum Command {
    Send(Vec<u8>),
    SetLogging { enabled: bool, path: Option<PathBuf> },
    Resize { columns: u16, rows: u16 },
}

/// Session-task to UI messages.
enum Event {
    Connected,
    /// The session ended. `reason` is `None` for a clean, user-requested close.
    Closed { reason: Option<String> },
    /// The host key was refused. Carries what to show the user.
    HostKey(Rejection),
    /// The serial device reappeared under a different port name.
    PortChanged(String),
    /// Non-fatal problem, e.g. the log file could not be written.
    Warning(String),
}

/// Current time as `HH:MM:SS UTC`.
///
/// UTC rather than local time because `std` cannot convert to local time, and a whole date
/// library for one divider line is not worth it. Labelling it beats quietly showing the wrong
/// zone.
fn utc_hms() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 86_400;
    format!(
        "{:02}:{:02}:{:02} UTC",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// Connection lifecycle.
///
/// The Tauri build tracked this as two loosely-related booleans (`is_active`, `is_running`),
/// which made it impossible to tell "never connected" from "dropped".
///
/// There is deliberately no `Failed` state: a failed attempt returns to `Disconnected` with
/// `last_error` set, which is exactly what re-enables the button so the user can try again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    /// A reconnect is in flight. Distinct from `Connecting` only so the button can say so.
    Reconnecting,
    Connected,
}

/// Delay before each successive automatic retry, in seconds.
///
/// Backing off matters because the common cause of a drop — a host rebooting, a link
/// flapping — takes longer than one second to clear, and hammering it helps nobody.
const RETRY_BACKOFF_SECONDS: &[u64] = &[1, 2, 5, 10, 20, 30];

/// One terminal tab: its settings, its scrollback, and its transport if connected.
pub struct Session {
    pub settings: ConnectionSettings,
    pub display_mode: DisplayMode,
    pub state: ConnectionState,
    /// Last failure, shown in the tab so errors are not lost to a dismissed dialog.
    pub last_error: Option<String>,
    /// A host key awaiting the user's decision.
    pub pending_host_key: Option<Rejection>,

    /// Whether this session has ever been connected.
    ///
    /// Drives the Connect/Reconnect label, and gates automatic retries: retrying a connection
    /// that never worked in the first place would just repeat a configuration error.
    has_connected: bool,
    /// How many times this session has reconnected, shown on the divider.
    reconnect_count: u32,
    /// Connect this tab automatically on startup. Off by default; the decision is guarded by
    /// [`crate::persist::may_auto_connect`].
    pub auto_connect: bool,
    /// Retry automatically after an unexpected drop. Off by default — a reconnect can be a
    /// visible action on the remote host, so it should be the user's choice.
    pub auto_reconnect: bool,
    /// When the next automatic retry is due.
    retry_at: Option<Instant>,
    /// Index into [`RETRY_BACKOFF_SECONDS`] for the next automatic retry.
    retry_attempt: usize,

    /// Secrets, held in memory only and never persisted.
    pub credentials: ssh::Credentials,

    /// Scrollback. Shared with the session task, which appends to it.
    pub buffer: Arc<Mutex<TermBuffer>>,
    pub max_bytes: usize,

    /// The terminal screen, present only while ANSI mode is selected.
    emulator: Option<Emulator>,
    /// Last size pushed to the transport, so a resize is only sent when it changes.
    sent_size: Option<(u16, u16)>,

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
    /// Give the terminal view keyboard focus the next time this tab is drawn.
    ///
    /// Set when a connection is established: having just connected, the next thing you want
    /// is to type. Consumed by the tab viewer rather than acted on here, because focus only
    /// makes sense for a tab that is actually visible, and only the viewer knows that.
    pub focus_terminal: bool,

    commands: Option<mpsc::UnboundedSender<Command>>,
    events: Option<mpsc::UnboundedReceiver<Event>>,
}

impl Session {
    pub fn new(settings: ConnectionSettings) -> Self {
        // SSH shells emit escape sequences constantly, so a styled-text view would be
        // unreadable; serial devices are more often plain.
        let display_mode = match settings.kind {
            ConnectionKind::Ssh => DisplayMode::Ansi,
            ConnectionKind::Serial => DisplayMode::default(),
        };
        Self {
            settings,
            display_mode,
            state: ConnectionState::Disconnected,
            last_error: None,
            pending_host_key: None,
            has_connected: false,
            reconnect_count: 0,
            auto_connect: false,
            auto_reconnect: false,
            retry_at: None,
            retry_attempt: 0,
            credentials: ssh::Credentials::default(),
            buffer: Arc::new(Mutex::new(TermBuffer::new(DEFAULT_MAX_BYTES))),
            max_bytes: DEFAULT_MAX_BYTES,
            emulator: None,
            sent_size: None,
            log_enabled: false,
            log_path: None,
            send_text: String::new(),
            send_mode: SendMode::default(),
            append_cr: false,
            append_lf: false,
            enter_crlf: true,
            font_size: 13.0,
            controls_height: 150.0,
            focus_terminal: false,
            commands: None,
            events: None,
        }
    }

    /// Tab label.
    pub fn title(&self) -> String {
        let name = self.settings.label();
        match self.state {
            ConnectionState::Connected => format!("• {name}"),
            ConnectionState::Connecting | ConnectionState::Reconnecting => format!("○ {name}"),
            ConnectionState::Disconnected => name,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    /// Whether a connection exists or is being established.
    ///
    /// Includes `Reconnecting`, which is what makes the reconnect button idempotent: a second
    /// press, an Enter-key repeat or an automatic retry firing mid-attempt is a no-op rather
    /// than a second session.
    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Connecting | ConnectionState::Reconnecting | ConnectionState::Connected
        )
    }

    /// Whether the user can ask for a (re)connection right now.
    pub fn can_connect(&self) -> bool {
        !self.is_busy() && self.pending_host_key.is_none()
    }

    /// Whether this session has been connected before, so the button reads "Reconnect".
    pub fn has_connected(&self) -> bool {
        self.has_connected
    }

    /// How long until the next automatic retry, if one is scheduled.
    pub fn retry_countdown(&self) -> Option<Duration> {
        self.retry_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    /// Start the session task.
    pub fn connect(&mut self, rt: &Handle, ctx: &egui::Context) {
        // An explicit press clears any pending automatic retry, and starts the backoff over.
        self.retry_at = None;
        self.retry_attempt = 0;
        self.connect_inner(rt, ctx, None);
    }

    /// Re-establish a connection that dropped, keeping the terminal contents.
    ///
    /// Idempotent: while an attempt is in flight [`Self::connect_inner`] refuses, so a second
    /// press cannot open a second session. A failure returns the state to `Disconnected`, which
    /// re-enables the button so the user can try as often as they like.
    pub fn reconnect(&mut self, rt: &Handle, ctx: &egui::Context) {
        if !self.can_connect() {
            return;
        }
        self.retry_at = None;
        self.retry_attempt = 0;
        self.connect_inner(rt, ctx, None);
    }

    /// Retry after the user accepted an unknown host key.
    ///
    /// The approval is passed through as a fingerprint, so it only authorises the exact key
    /// the user was shown.
    pub fn accept_host_key_and_connect(&mut self, rt: &Handle, ctx: &egui::Context) {
        let Some(Rejection::Unknown { fingerprint, .. }) = self.pending_host_key.clone() else {
            return;
        };
        self.pending_host_key = None;
        self.connect_inner(rt, ctx, Some(fingerprint));
    }

    pub fn reject_host_key(&mut self) {
        self.pending_host_key = None;
    }

    fn connect_inner(
        &mut self,
        rt: &Handle,
        ctx: &egui::Context,
        approved_fingerprint: Option<String>,
    ) {
        if self.is_busy() {
            return;
        }
        if let Err(message) = self.settings.is_complete() {
            self.last_error = Some(message.to_owned());
            return;
        }
        if self.settings.kind == ConnectionKind::Ssh {
            if let Err(message) = self.credentials.satisfies(self.settings.ssh.auth) {
                self.last_error = Some(message.to_owned());
                return;
            }
        }

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::unbounded_channel();

        let settings = self.settings.clone();
        let credentials = self.credentials.clone();
        let buffer = Arc::clone(&self.buffer);
        let ctx = ctx.clone();
        let log_path = if self.log_enabled {
            self.log_path.clone()
        } else {
            None
        };
        // Start the remote terminal at the size we are actually showing.
        let size = self
            .emulator
            .as_ref()
            .map(|e| (e.size().columns as u16, e.size().screen_lines as u16))
            .unwrap_or((80, 24));

        rt.spawn(async move {
            run(
                settings,
                credentials,
                approved_fingerprint,
                size,
                buffer,
                ctx,
                cmd_rx,
                evt_tx,
                log_path,
            )
            .await;
        });

        self.commands = Some(cmd_tx);
        self.events = Some(evt_rx);
        self.state = if self.has_connected {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Connecting
        };
        self.last_error = None;
        self.pending_host_key = None;
        self.sent_size = None;
    }

    /// Bytes marking the seam between one connection and the next.
    ///
    /// The remote end's screen state died with the connection, so the new session must not
    /// inherit the old cursor position, colours, scroll region or alternate-screen flag. This
    /// resets all of those *without* clearing the screen, so the scrollback above stays exactly
    /// as it was — which is the whole point of the feature.
    ///
    /// It is injected into the ring rather than special-cased in the renderer, so it shows up in
    /// every display mode and survives a mode-switch replay. It never reaches the log file,
    /// because the logger only sees what the transport returned.
    fn reconnect_divider(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // Leave the alternate screen, drop any scroll region, soft-reset modes, clear
        // attributes, then start on a fresh line.
        out.extend_from_slice(b"\x1b[?1049l\x1b[r\x1b[!p\x1b[0m\r\n");
        out.extend_from_slice(
            format!(
                "\u{2500}\u{2500} reconnected #{} at {} \u{2500}\u{2500}\r\n",
                self.reconnect_count,
                utc_hms()
            )
            .as_bytes(),
        );
        out
    }

    /// Write the divider into the ring.
    fn mark_reconnected(&mut self) {
        let divider = self.reconnect_divider();
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.append_local(&divider);
        }
    }

    /// Schedule an automatic retry, if enabled.
    fn schedule_retry(&mut self) {
        if !self.auto_reconnect || !self.has_connected {
            return;
        }
        let seconds = RETRY_BACKOFF_SECONDS
            .get(self.retry_attempt)
            .copied()
            .unwrap_or_else(|| *RETRY_BACKOFF_SECONDS.last().unwrap_or(&30));
        self.retry_attempt = (self.retry_attempt + 1).min(RETRY_BACKOFF_SECONDS.len());
        self.retry_at = Some(Instant::now() + Duration::from_secs(seconds));
    }

    /// Ask the session task to stop.
    ///
    /// Dropping the command sender closes the channel, which the task's `select!` notices
    /// even while parked on a read, so there is no need to abort the task.
    pub fn disconnect(&mut self) {
        self.commands = None;
        self.state = ConnectionState::Disconnected;
    }

    /// Drain task messages and fire any due automatic retry. Called once per frame.
    ///
    /// Returns whether a connection was established this frame, so the caller can record it in
    /// the recents list. Only successes are worth remembering — a list of connections that never
    /// worked would just offer to repeat the user's typos.
    pub fn poll(&mut self, rt: &Handle, ctx: &egui::Context) -> bool {
        let mut connected = false;
        let mut dropped = false;

        if let Some(events) = self.events.as_mut() {
            while let Ok(event) = events.try_recv() {
                match event {
                    Event::Connected => {
                        self.state = ConnectionState::Connected;
                        connected = true;
                    }
                    Event::Closed { reason } => {
                        self.state = ConnectionState::Disconnected;
                        self.commands = None;
                        // `None` is a clean, user-requested close: not a drop, and not
                        // something to retry.
                        if reason.is_some() {
                            self.last_error = reason;
                            dropped = true;
                        }
                    }
                    Event::HostKey(rejection) => {
                        self.state = ConnectionState::Disconnected;
                        self.commands = None;
                        if rejection.is_promptable() {
                            self.pending_host_key = Some(rejection);
                        } else {
                            // A changed key is never promptable; show it as an error instead.
                            self.last_error = Some(rejection.message());
                        }
                    }
                    Event::PortChanged(name) => {
                        // The device came back on a different port; follow it.
                        self.settings.serial.name = name;
                    }
                    Event::Warning(message) => self.last_error = Some(message),
                }
            }
        }

        if connected {
            // Mark the seam only for a genuine reconnection, not the first connection.
            if self.has_connected {
                self.reconnect_count += 1;
                self.mark_reconnected();
            }
            self.has_connected = true;
            self.retry_attempt = 0;
            self.retry_at = None;
        }
        if dropped {
            self.schedule_retry();
        }
        if self.commands.is_none() && self.state == ConnectionState::Disconnected {
            self.events = None;
        }

        // Drive the retry timer. egui only repaints on demand, so a wake-up has to be asked
        // for or a scheduled retry would wait for unrelated input.
        if let Some(at) = self.retry_at {
            let remaining = at.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.retry_at = None;
                if self.can_connect() {
                    self.connect_inner(rt, ctx, None);
                }
            } else {
                ctx.request_repaint_after(remaining);
            }
        }

        connected
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
            self.state = ConnectionState::Disconnected;
            self.commands = None;
            self.last_error = Some("Lost the connection.".to_owned());
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
        // The emulator counts scrollback in lines, so its limit is re-derived and its content
        // rebuilt.
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

    /// Bring the emulator into line with the selected display mode, feed it any bytes that
    /// have arrived, and tell the far end if the terminal changed size. Called once per frame
    /// before drawing.
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
                self.push_size();
            }
            // Leaving ANSI mode frees the grid; the ring still has everything needed to
            // rebuild it on the way back.
            _ => self.emulator = None,
        }
    }

    /// Tell the transport the terminal size, when it changes.
    fn push_size(&mut self) {
        let Some(emulator) = self.emulator.as_ref() else {
            return;
        };
        let size = (
            emulator.size().columns as u16,
            emulator.size().screen_lines as u16,
        );
        if self.sent_size == Some(size) {
            return;
        }
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(Command::Resize {
                columns: size.0,
                rows: size.1,
            });
            self.sent_size = Some(size);
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
        // Replies produced while replaying are stale — the far end already answered these
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

/// Find the port to open, following the device if it moved.
///
/// Returns the port name to use, or `None` to keep the recorded one. Enumeration is blocking, so
/// it runs off the async worker.
async fn resolve_serial_port(settings: &crate::settings::SerialSettings) -> Option<String> {
    let usb_serial = settings.usb_serial.clone()?;
    if usb_serial.is_empty() {
        return None;
    }
    let name = settings.name.clone();
    tokio::task::spawn_blocking(move || {
        let ports = crate::discovery::list_ports();
        // The recorded name is still there: prefer it and change nothing.
        if ports.iter().any(|p| p.name == name) {
            return None;
        }
        ports
            .into_iter()
            .find(|p| p.serial_number == usb_serial)
            .map(|p| p.name)
    })
    .await
    .ok()
    .flatten()
}

/// The session task: open the transport, then pump it until told to stop.
#[allow(clippy::too_many_arguments)]
async fn run(
    settings: ConnectionSettings,
    credentials: ssh::Credentials,
    approved_fingerprint: Option<String>,
    size: (u16, u16),
    buffer: Arc<Mutex<TermBuffer>>,
    ctx: egui::Context,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    log_path: Option<PathBuf>,
) {
    let label = settings.label();

    // ---- open ----
    let transport = match settings.kind {
        ConnectionKind::Serial => {
            // A USB adapter that was unplugged and replugged can come back on a different port
            // number — Windows in particular does not guarantee COM numbering — so the device
            // is re-found by its USB serial number before giving up on the recorded name.
            let mut serial = settings.serial.clone();
            if let Some(found) = resolve_serial_port(&serial).await {
                if found != serial.name {
                    let _ = events.send(Event::Warning(format!(
                        "{} is gone; reconnected on {} (same device serial {}).",
                        serial.name,
                        found,
                        serial.usb_serial.as_deref().unwrap_or("?")
                    )));
                    let _ = events.send(Event::PortChanged(found.clone()));
                    serial.name = found;
                }
            }
            transport::open_serial(&serial).map_err(|e| (e, None))
        }
        ConnectionKind::Ssh => {
            let known_hosts = match settings
                .ssh
                .known_hosts
                .clone()
                .or_else(knownhosts::default_path)
            {
                Some(path) => path,
                None => {
                    let _ = events.send(Event::Closed {
                        reason: Some(
                            "Could not locate a home directory for known_hosts.".to_owned(),
                        ),
                    });
                    ctx.request_repaint();
                    return;
                }
            };
            ssh::connect(
                settings.ssh.clone(),
                credentials,
                approved_fingerprint,
                known_hosts,
                size.0,
                size.1,
            )
            .await
            .map(Transport::Ssh)
            .map_err(|e| match e {
                ssh::Error::HostKey(rejection) => (rejection.message(), Some(rejection)),
                other => (other.message(), None),
            })
        }
    };

    let mut transport = match transport {
        Ok(transport) => transport,
        Err((message, rejection)) => {
            match rejection {
                Some(rejection) => {
                    let _ = events.send(Event::HostKey(rejection));
                }
                None => {
                    let _ = events.send(Event::Closed {
                        reason: Some(message),
                    });
                }
            }
            ctx.request_repaint();
            return;
        }
    };

    let _ = events.send(Event::Connected);
    ctx.request_repaint();
    debug!("session open: {label}");

    let (mut logger, warning) = Logger::open(log_path).await;
    if let Some(warning) = warning {
        let _ = events.send(Event::Warning(warning));
    }

    let mut liveness = tokio::time::interval(LIVENESS_INTERVAL);
    liveness.tick().await; // the first tick completes immediately

    // ---- pump ----
    let reason = loop {
        tokio::select! {
            incoming = transport.recv() => match incoming {
                Incoming::Data(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    if let Ok(mut buffer) = buffer.lock() {
                        buffer.append(&data);
                    }
                    if let Some(warning) = logger.write(&data).await {
                        let _ = events.send(Event::Warning(warning));
                    }
                    ctx.request_repaint();
                }
                Incoming::Closed(reason) => break reason,
            },

            command = commands.recv() => match command {
                // Sender dropped: the user asked to disconnect.
                None => break None,
                Some(Command::Send(bytes)) => {
                    if let Err(e) = transport.send(&bytes).await {
                        break Some(e);
                    }
                }
                Some(Command::Resize { columns, rows }) => {
                    if let Err(e) = transport.resize(columns, rows).await {
                        // A failed resize is not worth dropping the connection over.
                        let _ = events.send(Event::Warning(e));
                    }
                }
                Some(Command::SetLogging { enabled, path }) => {
                    logger.flush().await;
                    let (new_logger, warning) =
                        Logger::open(if enabled { path } else { None }).await;
                    logger = new_logger;
                    if let Some(warning) = warning {
                        let _ = events.send(Event::Warning(warning));
                    }
                }
            },

            _ = liveness.tick() => {
                if let Some(reason) = transport.check_alive().await {
                    break Some(reason);
                }
            }
        }
    };

    logger.flush().await;
    transport.close().await;
    debug!("session closed: {label}: {reason:?}");
    let _ = events.send(Event::Closed { reason });
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{SerialSettings, SshSettings};

    fn serial_session() -> Session {
        Session::new(ConnectionSettings {
            kind: ConnectionKind::Serial,
            serial: SerialSettings {
                name: "COM9".into(),
                ..Default::default()
            },
            ssh: SshSettings::default(),
        })
    }

    fn ssh_session() -> Session {
        Session::new(ConnectionSettings {
            kind: ConnectionKind::Ssh,
            serial: SerialSettings::default(),
            ssh: SshSettings {
                host: "srv".into(),
                user: "phil".into(),
                ..Default::default()
            },
        })
    }

    #[test]
    fn ssh_sessions_default_to_ansi_mode() {
        // A remote shell emits escape sequences constantly, so the styled-text view would be
        // unreadable.
        assert_eq!(ssh_session().display_mode, DisplayMode::Ansi);
        assert_eq!(serial_session().display_mode, DisplayMode::Ascii);
    }

    #[test]
    fn titles_reflect_the_kind_and_state() {
        let mut serial = serial_session();
        assert_eq!(serial.title(), "COM9");
        serial.state = ConnectionState::Connected;
        assert_eq!(serial.title(), "• COM9");

        let mut ssh = ssh_session();
        assert_eq!(ssh.title(), "phil@srv");
        ssh.state = ConnectionState::Connecting;
        assert_eq!(ssh.title(), "○ phil@srv");
    }

    #[test]
    fn incomplete_settings_are_reported_rather_than_dialled() {
        let mut session = Session::new(ConnectionSettings::default());
        assert!(session.settings.is_complete().is_err());
        // No runtime needed: connect refuses before spawning anything.
        session.last_error = session
            .settings
            .is_complete()
            .err()
            .map(|m| m.to_owned());
        assert_eq!(session.last_error.as_deref(), Some("Select a port first."));
    }

    #[test]
    fn ssh_needs_a_host_and_a_user() {
        let mut settings = ConnectionSettings {
            kind: ConnectionKind::Ssh,
            ..Default::default()
        };
        assert_eq!(settings.is_complete(), Err("Enter a host first."));
        settings.ssh.host = "srv".into();
        assert_eq!(settings.is_complete(), Err("Enter a username first."));
        settings.ssh.user = "phil".into();
        assert!(settings.is_complete().is_ok());
    }

    #[test]
    fn sending_while_disconnected_is_dropped_not_panicking() {
        let mut session = serial_session();
        session.send(b"hello".to_vec());
        assert_eq!(session.state, ConnectionState::Disconnected);
    }

    #[test]
    fn a_changed_host_key_is_surfaced_as_an_error_not_a_prompt() {
        let mut session = ssh_session();
        let rejection = Rejection::Changed {
            host: "srv".into(),
            port: 22,
            line: 4,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:x".into(),
        };
        // Mimic what poll() does for this event.
        assert!(!rejection.is_promptable());
        session.last_error = Some(rejection.message());
        assert!(session.pending_host_key.is_none());
        assert!(session.last_error.as_deref().unwrap().contains("line 4"));
    }

    #[test]
    fn accepting_a_host_key_requires_a_promptable_rejection() {
        let mut session = ssh_session();
        // Nothing pending: accepting is a no-op rather than a panic.
        session.pending_host_key = None;
        session.reject_host_key();
        assert!(session.pending_host_key.is_none());

        session.pending_host_key = Some(Rejection::Unknown {
            host: "srv".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        });
        session.reject_host_key();
        assert!(session.pending_host_key.is_none(), "rejecting clears it");
    }

    #[test]
    fn credentials_never_reach_the_serialised_settings() {
        // Structurally they cannot — `ConnectionSettings` has no field for them — but assert it
        // against distinctive sentinel values so a future refactor that "helpfully" moves
        // credentials into settings fails loudly here.
        let mut session = ssh_session();
        session.credentials = ssh::Credentials {
            password: "sentinel-pw-9c3f".into(),
            passphrase: "sentinel-pp-4a71".into(),
        };
        let encoded = serde_json::to_string(&session.settings).expect("settings serialise");
        assert!(!encoded.contains("sentinel-pw-9c3f"), "password leaked: {encoded}");
        assert!(!encoded.contains("sentinel-pp-4a71"), "passphrase leaked: {encoded}");
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut session = serial_session();
        session.buffer.lock().unwrap().append(b"hello\n");
        assert!(session.buffer.lock().unwrap().retained_bytes() > 0);
        session.clear();
        assert_eq!(session.buffer.lock().unwrap().retained_bytes(), 0);
    }

    #[test]
    fn switching_to_ansi_builds_an_emulator_and_replays() {
        let mut session = serial_session();
        session.buffer.lock().unwrap().append(b"hello\r\n");
        assert!(session.emulator().is_none());

        session.display_mode = DisplayMode::Ansi;
        session.sync_emulator();
        assert!(session.emulator().is_some());
        assert!(
            session.emulator().unwrap().all_text().contains("hello"),
            "the ring must be replayed into the new screen"
        );

        // Leaving ANSI mode frees it again.
        session.display_mode = DisplayMode::Hex;
        session.sync_emulator();
        assert!(session.emulator().is_none());
    }

    #[test]
    fn returning_to_ansi_replays_from_the_ring_again() {
        let mut session = serial_session();
        session.buffer.lock().unwrap().append(b"first\r\n");
        session.display_mode = DisplayMode::Ansi;
        session.sync_emulator();
        session.display_mode = DisplayMode::Ascii;
        session.sync_emulator();
        session.buffer.lock().unwrap().append(b"second\r\n");
        session.display_mode = DisplayMode::Ansi;
        session.sync_emulator();

        let text = session.emulator().unwrap().all_text();
        assert!(text.contains("first"), "history survives the round trip");
        assert!(text.contains("second"));
    }

    #[test]
    fn resize_is_only_pushed_when_it_changes() {
        let mut session = serial_session();
        session.display_mode = DisplayMode::Ansi;
        session.sync_emulator();
        // No transport, so nothing is sent and the cache stays empty.
        assert_eq!(session.sent_size, None);

        // With a transport attached the first push records the size.
        let (tx, mut rx) = mpsc::unbounded_channel();
        session.commands = Some(tx);
        session.push_size();
        let first = session.sent_size;
        assert!(first.is_some());
        assert!(rx.try_recv().is_ok(), "the first size is sent");

        // Unchanged size sends nothing more.
        session.push_size();
        assert!(rx.try_recv().is_err());
        assert_eq!(session.sent_size, first);
    }

    // ---- reconnect ----

    /// Drive the session through a connect/drop/reconnect cycle without a real transport, by
    /// feeding it the events a session task would send.
    fn feed(session: &mut Session, events: Vec<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        for event in events {
            tx.send(event).expect("channel open");
        }
        session.events = Some(rx);
        // A dummy sender so the session believes a task is attached.
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
        session.commands = Some(cmd_tx);
        drain(session);
    }

    /// Process queued events without needing a runtime or egui context for the retry timer.
    fn drain(session: &mut Session) {
        let mut connected = false;
        let mut dropped = false;
        if let Some(events) = session.events.as_mut() {
            while let Ok(event) = events.try_recv() {
                match event {
                    Event::Connected => {
                        session.state = ConnectionState::Connected;
                        connected = true;
                    }
                    Event::Closed { reason } => {
                        session.state = ConnectionState::Disconnected;
                        session.commands = None;
                        if reason.is_some() {
                            session.last_error = reason;
                            dropped = true;
                        }
                    }
                    Event::PortChanged(name) => session.settings.serial.name = name,
                    Event::Warning(m) => session.last_error = Some(m),
                    Event::HostKey(_) => {}
                }
            }
        }
        if connected {
            if session.has_connected {
                session.reconnect_count += 1;
                session.mark_reconnected();
            }
            session.has_connected = true;
            session.retry_attempt = 0;
            session.retry_at = None;
        }
        if dropped {
            session.schedule_retry();
        }
    }

    fn buffer_text(session: &Session) -> String {
        let buffer = session.buffer.lock().unwrap();
        (0..buffer.line_count())
            .filter_map(|i| buffer.line(i))
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reconnecting_keeps_the_terminal_contents() {
        // The headline requirement: a reconnect must not clear the window. The Tauri build did
        // the opposite, wiping the buffer on every connect.
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        session.buffer.lock().unwrap().append(b"important output\r\n");

        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped".into()),
            }],
        );
        assert_eq!(session.state, ConnectionState::Disconnected);
        assert!(
            buffer_text(&session).contains("important output"),
            "a drop must not clear the buffer"
        );

        feed(&mut session, vec![Event::Connected]);
        let text = buffer_text(&session);
        assert!(
            text.contains("important output"),
            "a reconnect must not clear the buffer"
        );
        assert!(text.contains("reconnected #1"), "and must mark the seam");
    }

    #[test]
    fn the_first_connection_is_not_marked_as_a_reconnect() {
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        assert!(!buffer_text(&session).contains("reconnected"));
        assert!(session.has_connected());
    }

    #[test]
    fn each_reconnect_is_numbered() {
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        for expected in 1..=3 {
            feed(
                &mut session,
                vec![Event::Closed {
                    reason: Some("dropped".into()),
                }],
            );
            feed(&mut session, vec![Event::Connected]);
            assert!(buffer_text(&session).contains(&format!("reconnected #{expected}")));
        }
    }

    #[test]
    fn the_divider_resets_terminal_state_without_clearing_scrollback() {
        // The remote screen state died with the connection, so the new session must not
        // inherit a scroll region, the alternate screen, or leftover colours.
        let mut session = ssh_session();
        session.display_mode = DisplayMode::Ansi;
        feed(&mut session, vec![Event::Connected]);

        // Leave the old session in a thoroughly odd state.
        session
            .buffer
            .lock()
            .unwrap()
            .append(b"before the drop\r\n\x1b[?1049h\x1b[1;5r\x1b[31m");
        session.sync_emulator();
        assert!(session
            .emulator()
            .unwrap()
            .mode()
            .contains(alacritty_terminal::term::TermMode::ALT_SCREEN));

        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped".into()),
            }],
        );
        feed(&mut session, vec![Event::Connected]);
        session.sync_emulator();

        let emulator = session.emulator().unwrap();
        assert!(
            !emulator
                .mode()
                .contains(alacritty_terminal::term::TermMode::ALT_SCREEN),
            "the alternate screen must be left"
        );
        assert!(
            emulator.all_text().contains("before the drop"),
            "scrollback from before the drop must survive"
        );
        assert!(emulator.all_text().contains("reconnected"));
    }

    #[test]
    fn the_divider_is_not_counted_as_received_data() {
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        session.buffer.lock().unwrap().append(b"12345");
        let received = session.buffer.lock().unwrap().total_received();

        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped".into()),
            }],
        );
        feed(&mut session, vec![Event::Connected]);

        assert_eq!(
            session.buffer.lock().unwrap().total_received(),
            received,
            "an injected divider is not device output"
        );
    }

    #[test]
    fn a_clean_disconnect_is_not_treated_as_a_drop() {
        let mut session = ssh_session();
        session.auto_reconnect = true;
        feed(&mut session, vec![Event::Connected]);
        // `None` means the user asked to close.
        feed(&mut session, vec![Event::Closed { reason: None }]);
        assert!(
            session.retry_countdown().is_none(),
            "closing on purpose must not schedule a retry"
        );
        assert!(session.last_error.is_none());
    }

    #[test]
    fn auto_reconnect_is_off_by_default_and_schedules_nothing() {
        let mut session = ssh_session();
        assert!(!session.auto_reconnect);
        feed(&mut session, vec![Event::Connected]);
        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped".into()),
            }],
        );
        assert!(session.retry_countdown().is_none());
    }

    #[test]
    fn auto_reconnect_backs_off_between_attempts() {
        let mut session = ssh_session();
        session.auto_reconnect = true;
        feed(&mut session, vec![Event::Connected]);

        let mut delays = Vec::new();
        for _ in 0..4 {
            feed(
                &mut session,
                vec![Event::Closed {
                    reason: Some("dropped".into()),
                }],
            );
            delays.push(session.retry_countdown().expect("a retry is scheduled").as_secs());
            // Simulate the attempt starting and failing again.
            session.state = ConnectionState::Disconnected;
        }
        // Strictly increasing, so a flapping link is not hammered.
        assert!(
            delays.windows(2).all(|w| w[1] > w[0]),
            "delays should grow: {delays:?}"
        );
    }

    #[test]
    fn a_successful_connection_resets_the_backoff() {
        let mut session = ssh_session();
        session.auto_reconnect = true;
        feed(&mut session, vec![Event::Connected]);
        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped".into()),
            }],
        );
        let first = session.retry_countdown().unwrap().as_secs();
        feed(&mut session, vec![Event::Connected]);
        assert!(session.retry_countdown().is_none(), "success clears the timer");

        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("dropped again".into()),
            }],
        );
        assert_eq!(
            session.retry_countdown().unwrap().as_secs(),
            first,
            "the backoff starts over after a success"
        );
    }

    #[test]
    fn auto_reconnect_does_not_retry_a_connection_that_never_worked() {
        // Retrying a configuration error just repeats it.
        let mut session = ssh_session();
        session.auto_reconnect = true;
        feed(
            &mut session,
            vec![Event::Closed {
                reason: Some("no route to host".into()),
            }],
        );
        assert!(!session.has_connected());
        assert!(session.retry_countdown().is_none());
    }

    #[test]
    fn a_reconnect_while_one_is_in_flight_is_ignored() {
        // Idempotency is enforced by the state machine, not just by disabling the button.
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        session.state = ConnectionState::Reconnecting;
        assert!(!session.can_connect());
        assert!(session.is_busy());
    }

    #[test]
    fn a_failed_attempt_re_enables_the_button() {
        // The user's requirement: after a failure they can try again, repeatedly.
        let mut session = ssh_session();
        feed(&mut session, vec![Event::Connected]);
        for _ in 0..3 {
            session.state = ConnectionState::Reconnecting;
            assert!(!session.can_connect());
            feed(
                &mut session,
                vec![Event::Closed {
                    reason: Some("refused".into()),
                }],
            );
            assert_eq!(session.state, ConnectionState::Disconnected);
            assert!(session.can_connect(), "the button must be usable again");
        }
    }

    #[test]
    fn a_pending_host_key_blocks_connecting_until_resolved() {
        let mut session = ssh_session();
        session.pending_host_key = Some(Rejection::Unknown {
            host: "srv".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        });
        assert!(!session.can_connect());
        session.reject_host_key();
        assert!(session.can_connect());
    }

    #[test]
    fn a_moved_serial_port_is_followed() {
        let mut session = serial_session();
        assert_eq!(session.settings.serial.name, "COM9");
        feed(&mut session, vec![Event::PortChanged("COM14".into())]);
        assert_eq!(
            session.settings.serial.name, "COM14",
            "a replugged adapter on a new port number should be followed"
        );
    }

    #[test]
    fn utc_timestamps_are_well_formed() {
        let stamp = utc_hms();
        assert!(stamp.ends_with(" UTC"), "got {stamp}");
        let time = stamp.trim_end_matches(" UTC");
        let parts: Vec<_> = time.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].parse::<u32>().unwrap() < 24);
        assert!(parts[1].parse::<u32>().unwrap() < 60);
        assert!(parts[2].parse::<u32>().unwrap() < 60);
    }

    #[test]
    fn setting_scrollback_rebuilds_the_screen() {
        let mut session = serial_session();
        session.buffer.lock().unwrap().append(b"keep me\r\n");
        session.display_mode = DisplayMode::Ansi;
        session.sync_emulator();
        session.set_max_bytes(500_000);
        assert!(session.emulator().unwrap().all_text().contains("keep me"));
    }
}
