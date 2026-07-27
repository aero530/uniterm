//! Application shell: the dock, the toolbar, and the tab viewer.
//!
//! Replaces `+page.svelte`, `stores.ts`, the four fixed `layout/*.svelte` panes and the
//! hand-rolled `tabs/` components.
//!
//! The old layouts were Single/Double/Triple/Quad splits where *every* pane listed *every*
//! port, so the splits were N independent tab selectors over one global list. `egui_dock`
//! gives arbitrary splits and drag-and-drop instead, which is a superset — except that a
//! tab now lives in exactly one node, so the same connection can no longer be shown in two
//! panes at once. See PLAN.md task 1 for that trade-off.

use std::collections::BTreeMap;

use eframe::egui::{self, Ui};
use egui_dock::widgets::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, NodePath, TabViewer};
use tokio::runtime::Handle;

use crate::discovery::{self, PortInfo};
use crate::session::Session;
use crate::settings::SerialSettings;
use crate::term::{input, render};
use crate::ui;

/// Floor for the controls strip, so it never collapses to nothing.
const MIN_CONTROLS_HEIGHT: f32 = 40.0;
/// Floor for the terminal, so shrinking a pane cannot squeeze it out entirely.
const MIN_TERMINAL_HEIGHT: f32 = 60.0;

/// Identifies a tab. Stable across a run so `egui_dock` and the session map agree, and
/// serializable so plan task 4 can persist the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TabId(pub u64);

pub struct UniTermApp {
    dock: DockState<TabId>,
    sessions: BTreeMap<TabId, Session>,
    ports: Vec<PortInfo>,
    next_id: u64,
    rt: Handle,
}

impl UniTermApp {
    pub fn new(rt: Handle) -> Self {
        let ports = discovery::list_ports();
        let mut app = Self {
            dock: DockState::new(Vec::new()),
            sessions: BTreeMap::new(),
            ports,
            next_id: 0,
            rt,
        };
        // Open one tab so the window is not empty on first run.
        let id = app.new_session();
        app.dock = DockState::new(vec![id]);
        app
    }

    /// Create a session pre-filled with the first available port, matching the old
    /// `addConnection` behaviour.
    fn new_session(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;

        let mut settings = SerialSettings::default();
        if let Some(first) = self.ports.first() {
            settings.name = first.name.clone();
        }

        self.sessions.insert(id, Session::new(settings));
        id
    }

    fn refresh_ports(&mut self) {
        self.ports = discovery::list_ports();
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui
                    .button("↻ Refresh ports")
                    .on_hover_text("Re-enumerate the serial ports attached to this machine")
                    .clicked()
                {
                    self.refresh_ports();
                }

                if ui
                    .button("+ New tab")
                    .on_hover_text("Open another terminal tab")
                    .clicked()
                {
                    let id = self.new_session();
                    self.dock.push_to_focused_leaf(id);
                }

                ui.separator();
                ui.label(format!("{} port(s)", self.ports.len()));

                // Right-aligning inside a horizontal layout needs the remaining space
                // allocated explicitly, or the reversed layout has nothing to align against.
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        egui::widgets::global_theme_preference_switch(ui);
                        ui.separator();
                        ui.weak("drag a tab header to split the view");
                    },
                );
            });
            ui.add_space(2.0);
        });
    }
}

impl eframe::App for UniTermApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // Drain each session's task messages before drawing, so button states and any
        // errors reflect this frame.
        for session in self.sessions.values_mut() {
            session.poll();
        }

        self.toolbar(ui);

        let mut closed = Vec::new();
        let mut added = Vec::new();

        let style = egui_dock::Style::from_egui(ui.style().as_ref());
        let mut viewer = Viewer {
            sessions: &mut self.sessions,
            ports: &self.ports,
            rt: &self.rt,
            closed: &mut closed,
            added: &mut added,
        };

        DockArea::new(&mut self.dock)
            .style(style)
            .show_add_buttons(true)
            .show_leaf_close_all_buttons(false)
            .show_inside(ui, &mut viewer);

        for id in closed {
            // Dropping the session drops its command sender, which stops the task.
            self.sessions.remove(&id);
        }
        for path in added {
            let id = self.new_session();
            self.dock.set_focused_node_and_surface(path);
            self.dock.push_to_focused_leaf(id);
        }
    }
}

struct Viewer<'a> {
    sessions: &'a mut BTreeMap<TabId, Session>,
    ports: &'a [PortInfo],
    rt: &'a Handle,
    closed: &'a mut Vec<TabId>,
    added: &'a mut Vec<NodePath>,
}

impl TabViewer for Viewer<'_> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match self.sessions.get(tab) {
            Some(session) => session.title().into(),
            None => "(closed)".into(),
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("uniterm-tab", tab.0))
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        self.closed.push(*tab);
        OnCloseResponse::Close
    }

    fn on_add(&mut self, path: NodePath) {
        self.added.push(path);
    }

    /// The terminal draws its own background and manages its own scrolling.
    fn clear_background(&self, _tab: &Self::Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [false, false]
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        let Some(session) = self.sessions.get_mut(tab) else {
            ui.label("This tab's session has been closed.");
            return;
        };

        // Terminal on top taking all spare height, controls underneath. Mirrors the old
        // `grid-rows-[minmax(0,1fr),minmax(8rem,12rem)]` layout.
        //
        // This is ordinary top-down flow with an explicitly sized box for the terminal.
        // Positioning the two halves at absolute rects does not work here: the scroll area
        // grows to whatever vertical space it can reach and paints over the controls.
        // The controls' height is measured each frame and fed back in for the next one.
        let available = ui.available_height();
        let controls_height = session
            .controls_height
            .clamp(MIN_CONTROLS_HEIGHT, (available - MIN_TERMINAL_HEIGHT).max(MIN_CONTROLS_HEIGHT));
        let terminal_height = (available - controls_height).max(MIN_TERMINAL_HEIGHT);

        // Bring the terminal screen into line with the selected mode and feed it this
        // frame's bytes before anything is drawn.
        session.sync_emulator();

        let view_id = egui::Id::new(("terminal", tab.0));
        let font_size = session.font_size;
        let display_mode = session.display_mode;
        let buffer = std::sync::Arc::clone(&session.buffer);

        let response = ui
            .allocate_ui(egui::vec2(ui.available_width(), terminal_height), |ui| {
                match session.emulator_mut() {
                    // ANSI: a real screen, which owns its own scrollback and scroll position.
                    Some(emulator) => {
                        render::grid_view(ui, view_id, emulator, font_size, terminal_height)
                    }
                    // Everything else reads the raw byte ring.
                    None => {
                        let buffer = buffer.lock().expect("scrollback mutex poisoned");
                        render::buffer_view(
                            ui,
                            view_id,
                            &buffer,
                            display_mode,
                            font_size,
                            terminal_height,
                        )
                    }
                }
            })
            .inner;

        let controls = ui.scope(|ui| {
            ui.add_space(4.0);
            ui::controls(ui, session, self.ports, self.rt, tab.0);
        });
        session.controls_height = controls.response.rect.height() + 8.0;

        // Typing goes straight down the wire while the view has focus, which is how the
        // old app's "click in the terminal then type" mode worked.
        if response.focused {
            let events = ui.input(|i| i.events.clone());

            // Ctrl+Shift+C / Ctrl+Shift+V are UI shortcuts, not data. `encode_events`
            // deliberately drops them so they are never transmitted.
            let (copy, paste) = ui.input(|i| {
                (
                    i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::C),
                    i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::V),
                )
            });
            if copy {
                if let Some(text) = session
                    .emulator()
                    .and_then(|e| e.selection_text())
                {
                    ui.ctx().copy_text(text);
                }
            }
            if paste {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }

            if session.is_connected() {
                let modes = session.input_modes();
                let bytes = input::encode_events(&events, session.enter_crlf, modes);
                if !bytes.is_empty() {
                    // Typing jumps back to the newest output, as every terminal does —
                    // otherwise you type blind into a scrolled-back view.
                    if let Some(emulator) = session.emulator_mut() {
                        emulator.scroll_to_bottom();
                    }
                    session.send(bytes);
                }
            }
        }
    }
}
