//! Per-tab controls.
//!
//! Ported from `PortMenu.svelte`. Errors are shown inline in the tab instead of via
//! `alert()`, which blocked the whole window and lost the message once dismissed.

use eframe::egui::{self, Ui};
use tokio::runtime::Handle;

use crate::discovery::PortInfo;
use crate::session::{ConnectionState, Session};
use crate::settings::{
    self, baud_label, ConnectionKind, DataBits, DisplayMode, FlowControl, Parity, SendMode, SshAuth,
    StopBits, BAUD_RATES,
};
use crate::term::{render, MAX_MAX_BYTES, MIN_MAX_BYTES};

/// Draw the controls below a terminal view.
pub fn controls(
    ui: &mut Ui,
    session: &mut Session,
    ports: &[PortInfo],
    rt: &Handle,
    salt: u64,
) {
    let connected = session.is_connected();
    let busy = session.is_busy();

    // A host key waiting on the user takes over the strip: nothing else is actionable until
    // it is resolved, and it must not be easy to miss.
    if session.pending_host_key.is_some() {
        host_key_prompt(ui, session, rt);
        return;
    }

    // ---- connection parameters ----
    //
    // Every widget is a direct child of the wrapped row. That is load-bearing: a nested `Ui` —
    // which is what `add_enabled_ui`, `scope` and friends create — is allocated only *after* its
    // contents are laid out, so the wrapping row cannot know how wide it will be and places it at
    // the cursor regardless. The result is content running off the right edge instead of folding
    // onto the next line, and it happens even when the nested `Ui` holds a single widget.
    //
    // So enablement is applied to the row as a whole with `ui.disable()`, and the connect buttons
    // live on their own row because they need the opposite condition.
    ui.horizontal_wrapped(|ui| {
        if busy {
            ui.disable();
        }
        let previous_kind = session.settings.kind;
        enum_combo(
            ui,
            (salt, "kind"),
            80.0,
            &mut session.settings.kind,
            ConnectionKind::ALL,
            ConnectionKind::label,
        );
        // Switching to SSH switches the view too: a remote shell emits escape sequences
        // constantly, and the ASCII view would render them as `^[` noise.
        if session.settings.kind != previous_kind
            && session.settings.kind == ConnectionKind::Ssh
            && session.display_mode != DisplayMode::Ansi
        {
            session.display_mode = DisplayMode::Ansi;
        }
        ui.separator();
        // The parameters of one kind are meaningless for the other, so only one set is
        // shown rather than half the row being greyed out.
        match session.settings.kind {
            ConnectionKind::Serial => serial_fields(ui, session, ports, salt),
            ConnectionKind::Ssh => ssh_fields(ui, session, salt),
        }
    });

    // ---- connect / disconnect ----
    //
    // A separate row from the parameters above, which are disabled while a connection is up
    // whereas these are not. Buttons flow with the row rather than being right-aligned: aligning
    // to the right inside a wrapped row pushed the content past the pane width and off-screen.
    ui.horizontal_wrapped(|ui| {
        connect_buttons(ui, session, rt);
    });

    ui.add_space(2.0);

    // ---- display settings ----
    ui.horizontal_wrapped(|ui| {
        ui.label("Display");
        let previous_mode = session.display_mode;
        enum_combo(ui, (salt, "display"), 100.0, &mut session.display_mode, DisplayMode::ALL, DisplayMode::label);
        if session.display_mode != previous_mode {
            // Switching mode only changes how the retained bytes are drawn. The Tauri build
            // had to round-trip the whole buffer to the webview here; nothing to do now.
            ui.ctx().request_repaint();
        }

        ui.separator();

        ui.label("Scrollback");
        let mut scrollback_kb = (session.max_bytes / 1000) as u32;
        let slider = wrappable_slider(ui, |ui| {
            ui.add(
                egui::Slider::new(
                    &mut scrollback_kb,
                    (MIN_MAX_BYTES / 1000) as u32..=(MAX_MAX_BYTES / 1000) as u32,
                )
                .suffix(" kB")
                .logarithmic(true),
            )
        });
        if slider.changed() {
            session.set_max_bytes(scrollback_kb as usize * 1000);
        }

        ui.separator();
        ui.label("Font");
        wrappable_slider(ui, |ui| {
            ui.add(egui::Slider::new(&mut session.font_size, 8.0..=24.0).suffix(" px"))
        });

        ui.separator();
        if let Ok(buffer) = session.buffer.lock() {
            weak_label(
                ui,
                format!(
                    "{} received · {} held / {}",
                    bytes_label(buffer.total_received()),
                    bytes_label(buffer.retained_bytes() as u64),
                    bytes_label(buffer.max_bytes() as u64),
                ),
            )
            .on_hover_text("Bytes received this session, and how much scrollback is retained");
        }
        if let Some(emulator) = session.emulator() {
            let size = emulator.size();
            weak_label(ui, format!("· {}x{}", size.columns, size.screen_lines))
                .on_hover_text(
                    "Terminal size in columns and rows, derived from the pane size and font. \
                     Resizing the pane resizes the terminal, and SSH sessions tell the remote \
                     end so full-screen programs reflow.",
                );
        }
        if session.settings.kind == ConnectionKind::Ssh {
            // The tab header omits the port, so show the full identity here.
            weak_label(ui, format!("· {}", session.settings.ssh.identity()));
        }

        ui.separator();
        if ui.button("Clear").clicked() {
            session.clear();
        }
        let has_selection = session.emulator().is_some_and(|e| e.has_selection());
        if ui
            .add_enabled(has_selection, egui::Button::new("Copy"))
            .on_hover_text("Copy the selection (Ctrl+Shift+C). Drag in the terminal to select.")
            .clicked()
        {
            if let Some(text) = session.emulator().and_then(|e| e.selection_text()) {
                ui.ctx().copy_text(text);
            }
        }
        if ui
            .button("Copy All")
            .on_hover_text("Copy the whole scrollback")
            .clicked()
        {
            // In ANSI mode the grid holds the rendered screen and its scrollback, which is
            // what the user sees; the other modes copy the raw ring.
            let text = match session.emulator() {
                Some(emulator) => Some(emulator.all_text()),
                None => session
                    .buffer
                    .lock()
                    .ok()
                    .map(|buffer| render::plain_text(&buffer, session.display_mode)),
            };
            if let Some(text) = text {
                ui.ctx().copy_text(text);
            }
        }
        ui.checkbox(&mut session.enter_crlf, "Enter = CR+LF")
            .on_hover_text(
                "When the terminal has focus, Return transmits CR+LF. \
                 Turn off to send CR only, which is what remote shells expect.",
            );
    });

    ui.add_space(2.0);

    // ---- logging ----
    ui.horizontal_wrapped(|ui| {
        if ui.button("Log to file…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Text", &["txt", "log"])
                .set_file_name("uniterm.log")
                .save_file()
            {
                session.log_path = Some(path);
                session.log_enabled = true;
                session.apply_logging();
            }
        }

        let has_path = session.log_path.is_some();
        let label = if session.log_enabled { "Stop Log" } else { "Start Log" };
        if ui.add_enabled(has_path, egui::Button::new(label)).clicked() {
            session.log_enabled = !session.log_enabled;
            session.apply_logging();
        }

        match session.log_path.as_ref() {
            Some(path) => {
                let state = if session.log_enabled { "appending to" } else { "paused;" };
                ui.label(format!("{state} {}", path.display()));
            }
            None => {
                weak_label(ui, "No log file selected");
            }
        }
    });

    ui.add_space(2.0);

    // ---- send box ----
    ui.horizontal_wrapped(|ui| {
        let hint = match session.send_mode {
            SendMode::Ascii => "text to send",
            SendMode::Decimal => "e.g. 72 101 108",
            SendMode::Hex => "e.g. 48 65 6c",
        };
        // The entry takes the leftover width rather than `f32::INFINITY`. Asking for infinity
        // makes it swallow the whole row, pushing the mode selector and Send button off the
        // right edge where they cannot be reached at all. Reserving room for them and keeping a
        // usable minimum lets them wrap onto the next line instead when the pane is narrow.
        const TRAILING_CONTROLS_WIDTH: f32 = 330.0;
        const MIN_ENTRY_WIDTH: f32 = 120.0;
        let entry_width = (ui.available_width() - TRAILING_CONTROLS_WIDTH).max(MIN_ENTRY_WIDTH);

        let entry = ui.add_enabled(
            connected,
            egui::TextEdit::singleline(&mut session.send_text)
                .desired_width(entry_width)
                .hint_text(hint)
                .id_salt((salt, "send")),
        );

        ui.label("as");
        enum_combo(ui, (salt, "sendmode"), 90.0, &mut session.send_mode, SendMode::ALL, SendMode::label);
        ui.checkbox(&mut session.append_cr, "+CR");
        ui.checkbox(&mut session.append_lf, "+LF");

        let submitted =
            entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && connected;
        if ui.add_enabled(connected, egui::Button::new("Send")).clicked() || submitted {
            match settings::encode_send(
                &session.send_text,
                session.send_mode,
                session.append_cr,
                session.append_lf,
            ) {
                Ok(bytes) => {
                    session.send(bytes);
                    session.send_text.clear();
                }
                Err(message) => session.last_error = Some(message),
            }
        }
    });

    // ---- inline status / error ----
    if let Some(error) = session.last_error.clone() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(ui.visuals().error_fg_color, "!");
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.small_button("Dismiss").clicked() {
                session.last_error = None;
            }
        });
    }
}

/// Connect, reconnect and disconnect.
///
/// One button covers connecting and reconnecting: after a session has been up once it reads
/// "Reconnect", because that is what pressing it means. It is disabled while an attempt is in
/// flight and re-enabled the moment one fails, so a failed reconnect can be retried
/// immediately — and `Session::reconnect` refuses a second concurrent attempt regardless, so
/// the guard is not merely visual.
fn connect_buttons(ui: &mut Ui, session: &mut Session, rt: &Handle) {
    let state = session.state;
    let can_connect = session.can_connect();

    let label = match state {
        ConnectionState::Connecting => "Connecting…",
        ConnectionState::Reconnecting => "Reconnecting…",
        _ if session.has_connected() => "Reconnect",
        _ => "Connect",
    };

    let button = ui.add_enabled(can_connect, egui::Button::new(label));
    let button = if session.has_connected() {
        button.on_hover_text(
            "Re-establish the connection. The terminal contents are kept; a divider marks \
             where the new session starts.",
        )
    } else {
        button
    };
    if button.clicked() {
        if session.has_connected() {
            session.reconnect(rt, ui.ctx());
        } else {
            session.connect(rt, ui.ctx());
        }
    }

    if ui
        .add_enabled(
            session.is_connected(),
            egui::Button::new("Disconnect"),
        )
        .clicked()
    {
        session.disconnect();
    }

    // Automatic retry is opt-in: a reconnect can be a visible event on the remote host, and
    // silently redialling is not always wanted.
    ui.checkbox(&mut session.auto_reconnect, "Auto")
        .on_hover_text(
            "Automatically retry after an unexpected drop, backing off between attempts. \
             Only applies once the connection has worked at least once.",
        );

    // Opening a port at startup asserts control over hardware unprompted, so it is opt-in too.
    ui.checkbox(&mut session.auto_connect, "On start")
        .on_hover_text(
            "Connect this tab when UniTerm starts. Skipped if the saved device is not attached, \
             if the port now holds different hardware, or if the connection needs a password \
             (passwords are not saved).",
        );

    // Show the countdown so a pending retry is not a mystery.
    if let Some(remaining) = session.retry_countdown() {
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_millis() > 0);
        weak_label(ui, format!("retrying in {seconds}s"));
        if ui
            .small_button("Now")
            .on_hover_text("Retry immediately instead of waiting")
            .clicked()
        {
            session.reconnect(rt, ui.ctx());
        }
    }
}

/// Serial port parameters.
fn serial_fields(ui: &mut Ui, session: &mut Session, ports: &[PortInfo], salt: u64) {
    let serial = &mut session.settings.serial;

    egui::ComboBox::from_id_salt((salt, "port"))
        .selected_text(if serial.name.is_empty() {
            "Select port".to_owned()
        } else {
            serial.name.clone()
        })
        .width(200.0)
        .show_ui(ui, |ui| {
            if ports.is_empty() {
                ui.label("No serial ports found");
            }
            for port in ports {
                // Record the device serial alongside the name, so a replugged adapter can be
                // followed to whatever port it comes back on.
                if ui
                    .selectable_label(serial.name == port.name, port.label())
                    .clicked()
                {
                    serial.name = port.name.clone();
                    serial.usb_serial = Some(port.serial_number.clone())
                        .filter(|s| !s.is_empty());
                }
            }
        });

    combo(ui, (salt, "baud"), 110.0, &baud_label(serial.baud_rate), |ui| {
        for baud in BAUD_RATES {
            ui.selectable_value(&mut serial.baud_rate, *baud, baud_label(*baud));
        }
    });
    enum_combo(ui, (salt, "flow"), 130.0, &mut serial.flow_control, FlowControl::ALL, FlowControl::label);
    enum_combo(ui, (salt, "data"), 130.0, &mut serial.data_bits, DataBits::ALL, DataBits::label);
    enum_combo(ui, (salt, "parity"), 110.0, &mut serial.parity, Parity::ALL, Parity::label);
    enum_combo(ui, (salt, "stop"), 120.0, &mut serial.stop_bits, StopBits::ALL, StopBits::label);
}

/// SSH connection parameters and credentials.
fn ssh_fields(ui: &mut Ui, session: &mut Session, salt: u64) {
    ui.label("Host");
    ui.add(
        egui::TextEdit::singleline(&mut session.settings.ssh.host)
            .desired_width(160.0)
            .hint_text("hostname or IP")
            .id_salt((salt, "host")),
    );

    ui.label("Port");
    ui.add(
        egui::DragValue::new(&mut session.settings.ssh.port)
            .range(1..=65535)
            .speed(1.0),
    );

    ui.label("User");
    ui.add(
        egui::TextEdit::singleline(&mut session.settings.ssh.user)
            .desired_width(110.0)
            .id_salt((salt, "user")),
    );

    enum_combo(
        ui,
        (salt, "auth"),
        120.0,
        &mut session.settings.ssh.auth,
        SshAuth::ALL,
        SshAuth::label,
    );

    // Secrets are typed here and held in memory only; they are never written to disk.
    match session.settings.ssh.auth {
        SshAuth::Password => {
            ui.add(
                egui::TextEdit::singleline(&mut session.credentials.password)
                    .desired_width(130.0)
                    .password(true)
                    .hint_text("password")
                    .id_salt((salt, "password")),
            )
            .on_hover_text("Held in memory for this session only. Never written to disk.");
        }
        SshAuth::PublicKey => {
            let label = session
                .settings
                .ssh
                .key_path
                .as_ref()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "Choose key…".to_owned());
            if ui
                .button(label)
                .on_hover_text(
                    session
                        .settings
                        .ssh
                        .key_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Select an OpenSSH private key file".to_owned()),
                )
                .clicked()
            {
                let mut dialog = rfd::FileDialog::new();
                if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
                {
                    dialog = dialog.set_directory(std::path::PathBuf::from(home).join(".ssh"));
                }
                if let Some(path) = dialog.pick_file() {
                    session.settings.ssh.key_path = Some(path);
                }
            }
            ui.add(
                egui::TextEdit::singleline(&mut session.credentials.passphrase)
                    .desired_width(120.0)
                    .password(true)
                    .hint_text("passphrase")
                    .id_salt((salt, "passphrase")),
            )
            .on_hover_text("Leave blank for an unencrypted key. Never written to disk.");
        }
    }
}

/// Trust-on-first-use prompt for an unrecognised host key.
///
/// Only reached for [`crate::knownhosts::Rejection::Unknown`]. A changed key is reported as an
/// error and has no accept path at all — waving that through is exactly what recording host
/// keys exists to prevent.
fn host_key_prompt(ui: &mut Ui, session: &mut Session, rt: &Handle) {
    let Some(rejection) = session.pending_host_key.clone() else {
        return;
    };
    let (host, port, algorithm, fingerprint) = match &rejection {
        crate::knownhosts::Rejection::Unknown {
            host,
            port,
            algorithm,
            fingerprint,
        } => (host, port, algorithm, fingerprint),
        // Not promptable; nothing to draw.
        crate::knownhosts::Rejection::Changed { .. } => return,
    };

    egui::Frame::default()
        .fill(ui.visuals().widgets.active.bg_fill.gamma_multiply(0.35))
        .stroke(egui::Stroke::new(1.0, ui.visuals().warn_fg_color))
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().warn_fg_color, "!");
                ui.strong(format!("{host}:{port} is not in your known_hosts."));
            });
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Key type");
                ui.code(algorithm);
                ui.label("fingerprint");
                ui.code(fingerprint);
            });
            ui.add_space(2.0);
            ui.label(
                "Confirm this fingerprint through a channel you already trust before accepting. \
                 Accepting records it in ~/.ssh/known_hosts and connects.",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Accept and connect").clicked() {
                    session.accept_host_key_and_connect(rt, ui.ctx());
                }
                if ui.button("Reject").clicked() {
                    session.reject_host_key();
                }
                if ui.button("Copy fingerprint").clicked() {
                    ui.ctx().copy_text(fingerprint.clone());
                }
            });
        });
}

/// Compact byte count for the status readout.
fn bytes_label(bytes: u64) -> String {
    match bytes {
        0..=999 => format!("{bytes} B"),
        1_000..=999_999 => format!("{:.1} kB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// A combo box over a fixed set of enum variants.
fn enum_combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: impl egui::AsIdSalt,
    width: f32,
    current: &mut T,
    all: &[T],
    label: fn(T) -> &'static str,
) {
    combo(ui, salt, width, label(*current), |ui| {
        for option in all {
            ui.selectable_value(current, *option, label(*option));
        }
    });
}

/// Draw a widget that manages its own cursor, while still letting the row wrap around it.
///
/// egui only applies a wrapping layout's line breaks inside `Ui::allocate_space`. Some widgets
/// never call it: `ComboBox` measures its own content and finishes with
/// `advance_cursor_after_rect`, so in a `horizontal_wrapped` row it simply walks the cursor past
/// the right edge and gets clipped instead of moving to the next line.
///
/// Reserving the space up front with an explicit allocation puts the wrap decision back in the
/// layout's hands; the widget then draws into the rect it was given. `new_child` is used rather
/// than `scope` because a scope advances the cursor a second time, which would double the spacing.
fn wrappable<R>(ui: &mut Ui, size: egui::Vec2, content: impl FnOnce(&mut Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    content(&mut child)
}

/// Width reserved for a slider's value box, on top of the track.
const SLIDER_VALUE_WIDTH: f32 = 74.0;

/// A slider that participates in row wrapping.
///
/// `Slider` lays itself out with `ui.horizontal`, so like [`combo`] it never consults the wrapping
/// layout and would otherwise run its value box off the right edge. The reserved width is the
/// configured track width plus room for the value box.
fn wrappable_slider<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = egui::vec2(
        ui.spacing().slider_width + SLIDER_VALUE_WIDTH,
        ui.spacing().interact_size.y,
    );
    wrappable(ui, size, add)
}

/// A combo box that participates in row wrapping.
///
/// `ComboBox::width` is documented as the *minimum overall width*, so reserving exactly that
/// matches what the widget goes on to draw.
fn combo(
    ui: &mut Ui,
    salt: impl egui::AsIdSalt,
    width: f32,
    selected: &str,
    contents: impl FnOnce(&mut Ui),
) {
    let size = egui::vec2(width, ui.spacing().interact_size.y);
    wrappable(ui, size, |ui| {
        egui::ComboBox::from_id_salt(salt)
            .selected_text(selected)
            .width(width)
            .show_ui(ui, contents);
    });
}

/// A dimmed, non-wrapping label.
///
/// `Extend` matters in a wrapped row: a label defaults to wrapping its *text*, so when little
/// width is left it squeezes into the remainder and gets clipped mid-word. Extending instead keeps
/// it one atomic item, which the row then moves to the next line whole.
fn weak_label(ui: &mut Ui, text: impl Into<String>) -> egui::Response {
    ui.add(
        egui::Label::new(egui::RichText::new(text.into()).weak())
            .wrap_mode(egui::TextWrapMode::Extend),
    )
}
