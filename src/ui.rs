//! Per-tab controls.
//!
//! Ported from `PortMenu.svelte`. Errors are shown inline in the tab instead of via
//! `alert()`, which blocked the whole window and lost the message once dismissed.

use eframe::egui::{self, Ui};
use tokio::runtime::Handle;

use crate::discovery::PortInfo;
use crate::session::Session;
use crate::settings::{
    self, baud_label, DataBits, DisplayMode, FlowControl, Parity, SendMode, StopBits, BAUD_RATES,
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

    // ---- connection parameters and connect/disconnect ----
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt((salt, "port"))
                .selected_text(if session.settings.name.is_empty() {
                    "Select port".to_owned()
                } else {
                    session.settings.name.clone()
                })
                .width(220.0)
                .show_ui(ui, |ui| {
                    if ports.is_empty() {
                        ui.label("No serial ports found");
                    }
                    for port in ports {
                        ui.selectable_value(
                            &mut session.settings.name,
                            port.name.clone(),
                            port.label(),
                        );
                    }
                });

            combo(ui, (salt, "baud"), 110.0, &baud_label(session.settings.baud_rate), |ui| {
                for baud in BAUD_RATES {
                    ui.selectable_value(&mut session.settings.baud_rate, *baud, baud_label(*baud));
                }
            });
            enum_combo(ui, (salt, "flow"), 130.0, &mut session.settings.flow_control, FlowControl::ALL, FlowControl::label);
            enum_combo(ui, (salt, "data"), 130.0, &mut session.settings.data_bits, DataBits::ALL, DataBits::label);
            enum_combo(ui, (salt, "parity"), 110.0, &mut session.settings.parity, Parity::ALL, Parity::label);
            enum_combo(ui, (salt, "stop"), 120.0, &mut session.settings.stop_bits, StopBits::ALL, StopBits::label);
        });

        // These flow with the row rather than being right-aligned: aligning to the right
        // inside a wrapped row pushed the content past the pane width and off-screen.
        ui.separator();
        if ui
            .add_enabled(!busy, egui::Button::new("Connect"))
            .clicked()
        {
            session.connect(rt, ui.ctx());
        }
        if ui
            .add_enabled(connected, egui::Button::new("Disconnect"))
            .clicked()
        {
            session.disconnect();
        }
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
        let slider = ui.add(
            egui::Slider::new(&mut scrollback_kb, (MIN_MAX_BYTES / 1000) as u32..=(MAX_MAX_BYTES / 1000) as u32)
                .suffix(" kB")
                .logarithmic(true),
        );
        if slider.changed() {
            session.set_max_bytes(scrollback_kb as usize * 1000);
        }

        ui.separator();
        ui.label("Font");
        ui.add(egui::Slider::new(&mut session.font_size, 8.0..=24.0).suffix(" px"));

        ui.separator();
        if let Ok(buffer) = session.buffer.lock() {
            ui.weak(format!(
                "{} received · {} held / {}",
                bytes_label(buffer.total_received()),
                bytes_label(buffer.retained_bytes() as u64),
                bytes_label(buffer.max_bytes() as u64),
            ))
            .on_hover_text("Bytes received this session, and how much scrollback is retained");
        }
        if let Some(emulator) = session.emulator() {
            let size = emulator.size();
            ui.weak(format!("· {}x{}", size.columns, size.screen_lines))
                .on_hover_text(
                    "Terminal size in columns and rows, derived from the pane size and font. \
                     Resizing the pane resizes the terminal.",
                );
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
                ui.weak("No log file selected");
            }
        }
    });

    ui.add_space(2.0);

    // ---- send box ----
    ui.horizontal(|ui| {
        let hint = match session.send_mode {
            SendMode::Ascii => "text to send",
            SendMode::Decimal => "e.g. 72 101 108",
            SendMode::Hex => "e.g. 48 65 6c",
        };
        let entry = ui.add_enabled(
            connected,
            egui::TextEdit::singleline(&mut session.send_text)
                .desired_width(f32::INFINITY)
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
            ui.colored_label(ui.visuals().error_fg_color, "⚠");
            ui.colored_label(ui.visuals().error_fg_color, error);
            if ui.small_button("Dismiss").clicked() {
                session.last_error = None;
            }
        });
    }
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

fn combo(
    ui: &mut Ui,
    salt: impl egui::AsIdSalt,
    width: f32,
    selected: &str,
    contents: impl FnOnce(&mut Ui),
) {
    egui::ComboBox::from_id_salt(salt)
        .selected_text(selected)
        .width(width)
        .show_ui(ui, contents);
}
