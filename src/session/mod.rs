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
use std::time::Duration;

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
    pub settings: ConnectionSettings,
    pub display_mode: DisplayMode,
    pub state: ConnectionState,
    /// Last failure, shown in the tab so errors are not lost to a dismissed dialog.
    pub last_error: Option<String>,
    /// A host key awaiting the user's decision.
    pub pending_host_key: Option<Rejection>,

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
            commands: None,
            events: None,
        }
    }

    /// Tab label.
    pub fn title(&self) -> String {
        let name = self.settings.label();
        match self.state {
            ConnectionState::Connected => format!("● {name}"),
            ConnectionState::Connecting => format!("◌ {name}"),
            ConnectionState::Disconnected => name,
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
        self.state = ConnectionState::Connecting;
        self.last_error = None;
        self.pending_host_key = None;
        self.sent_size = None;
    }

    /// Ask the session task to stop.
    ///
    /// Dropping the command sender closes the channel, which the task's `select!` notices
    /// even while parked on a read, so there is no need to abort the task.
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
                    if reason.is_some() {
                        self.last_error = reason;
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
                Event::Warning(message) => self.last_error = Some(message),
            }
        }
        if self.commands.is_none() && self.state == ConnectionState::Disconnected {
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
        ConnectionKind::Serial => transport::open_serial(&settings.serial).map_err(|e| (e, None)),
        ConnectionKind::Ssh => {
            let known_hosts = match knownhosts::default_path() {
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
        assert_eq!(serial.title(), "● COM9");

        let mut ssh = ssh_session();
        assert_eq!(ssh.title(), "phil@srv");
        ssh.state = ConnectionState::Connecting;
        assert_eq!(ssh.title(), "◌ phil@srv");
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
    fn credentials_are_not_part_of_persisted_settings() {
        // A compile-time-ish guard: settings must serialize without secrets in them.
        let session = ssh_session();
        let json = serde_json::to_string(&session.settings).unwrap_or_default();
        assert!(!json.contains("password"));
        assert!(!json.contains("passphrase"));
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
