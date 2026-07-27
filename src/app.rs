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
use crate::persist;
use crate::recents::{self, Recents};
use crate::session::Session;
use crate::settings::ConnectionSettings;
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
    /// A saved payload that could not be read. Held so the next save sets it aside rather than
    /// overwriting it, and so the user can be told once.
    unreadable_state: Option<String>,
    /// Message about the restore, shown in the toolbar until dismissed.
    restore_notice: Option<String>,
    /// Connections that have worked before, offered for one-click reopening.
    recents: Recents,
}

impl UniTermApp {
    /// Build the app, restoring saved state when there is any.
    pub fn new(rt: Handle, storage: Option<&dyn eframe::Storage>, ctx: &egui::Context) -> Self {
        let ports = discovery::list_ports();
        let mut app = Self {
            dock: DockState::new(Vec::new()),
            sessions: BTreeMap::new(),
            ports,
            next_id: 0,
            rt,
            unreadable_state: None,
            restore_notice: None,
            recents: Recents::default(),
        };

        match storage.map(persist::load) {
            Some(persist::Loaded::Restored(state)) => app.restore(*state, ctx),
            Some(persist::Loaded::Unreadable { reason, payload }) => {
                app.unreadable_state = Some(payload);
                app.restore_notice = Some(reason);
                app.start_fresh();
            }
            Some(persist::Loaded::Fresh) | None => app.start_fresh(),
        }
        app
    }

    /// One empty tab, so the window is never blank.
    fn start_fresh(&mut self) {
        let id = self.new_session();
        self.dock = DockState::new(vec![id]);
    }

    /// Rebuild tabs and layout from saved state.
    ///
    /// The dock and the session map are stored separately, so they are reconciled rather than
    /// trusted: a layout referring to a tab with no definition would otherwise render as a dead
    /// "(closed)" pane forever.
    fn restore(&mut self, state: persist::PersistedState, ctx: &egui::Context) {
        let mut skipped = Vec::new();
        self.recents = Recents::from_entries(state.recents);

        for tab in state.tabs {
            let id = tab.id;
            let mut session = Session::new(tab.settings.clone());
            session.display_mode = tab.display_mode;
            session.set_max_bytes(tab.max_bytes);
            session.font_size = tab.font_size;
            session.enter_crlf = tab.enter_crlf;
            session.auto_reconnect = tab.auto_reconnect;
            session.auto_connect = tab.auto_connect;
            session.send_mode = tab.send_mode;
            session.append_cr = tab.append_cr;
            session.append_lf = tab.append_lf;
            session.log_path = tab.log_path;
            session.log_enabled = tab.log_enabled;

            // Dial only what the user opted in for, and only when it is safe to.
            if tab.auto_connect {
                match persist::may_auto_connect(&tab.settings, &self.ports) {
                    persist::AutoConnect::Yes => session.connect(&self.rt, ctx),
                    persist::AutoConnect::No(reason) => {
                        session.last_error = Some(reason.clone());
                        skipped.push(format!("{}: {reason}", tab.settings.label()));
                    }
                }
            }

            self.sessions.insert(id, session);
            self.next_id = self.next_id.max(id.0 + 1);
        }
        self.next_id = self.next_id.max(state.next_id);

        // Drop layout entries with no matching definition.
        self.dock = state.dock;
        let known: Vec<TabId> = self.sessions.keys().copied().collect();
        self.dock.retain_tabs(|tab| known.contains(tab));

        // Any definition the layout lost track of would be invisible; re-attach it.
        let placed: Vec<TabId> = self.dock.iter_all_tabs().map(|(_, id)| *id).collect();
        for id in known {
            if !placed.contains(&id) {
                self.dock.push_to_focused_leaf(id);
            }
        }
        if self.dock.iter_all_tabs().next().is_none() {
            self.start_fresh();
        }

        if !skipped.is_empty() {
            self.restore_notice = Some(format!(
                "{} tab(s) were not connected automatically. {}",
                skipped.len(),
                skipped.join(" · ")
            ));
        }
    }

    /// Snapshot for saving.
    fn snapshot(&self) -> persist::PersistedState {
        persist::PersistedState {
            version: persist::SCHEMA_VERSION,
            next_id: self.next_id,
            dock: self.dock.clone(),
            tabs: self
                .sessions
                .iter()
                .map(|(id, session)| persist::PersistedTab {
                    id: *id,
                    settings: session.settings.clone(),
                    display_mode: session.display_mode,
                    max_bytes: session.max_bytes,
                    font_size: session.font_size,
                    enter_crlf: session.enter_crlf,
                    auto_reconnect: session.auto_reconnect,
                    auto_connect: session.auto_connect,
                    send_mode: session.send_mode,
                    append_cr: session.append_cr,
                    append_lf: session.append_lf,
                    log_path: session.log_path.clone(),
                    log_enabled: session.log_enabled,
                })
                .collect(),
            recents: self.recents.entries().to_vec(),
        }
    }

    /// Create a session pre-filled with the first available port, matching the old
    /// `addConnection` behaviour.
    fn new_session(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;

        let mut settings = ConnectionSettings::default();
        if let Some(first) = self.ports.first() {
            settings.serial.name = first.name.clone();
        }

        self.sessions.insert(id, Session::new(settings));
        id
    }

    fn refresh_ports(&mut self) {
        self.ports = discovery::list_ports();
    }

    /// Open a tab for a remembered connection.
    ///
    /// Connecting reuses the same policy as startup auto-connect, so a serial device that is not
    /// attached, a port that now holds different hardware, or an SSH tab whose password was never
    /// saved opens ready-to-go with the reason shown rather than failing.
    fn open_recent(&mut self, settings: ConnectionSettings, ctx: &egui::Context) {
        let id = TabId(self.next_id);
        self.next_id += 1;

        let mut session = Session::new(settings.clone());
        match persist::may_auto_connect(&settings, &self.ports) {
            persist::AutoConnect::Yes => session.connect(&self.rt, ctx),
            persist::AutoConnect::No(reason) => session.last_error = Some(reason),
        }
        self.sessions.insert(id, session);
        self.dock.push_to_focused_leaf(id);
    }

    /// Menu listing remembered connections.
    fn recents_menu(&mut self, ui: &mut Ui) {
        let mut to_open = None;
        let mut to_pin = None;
        let mut to_remove = None;
        let mut clear = false;
        let now = recents::now_seconds();

        let label = if self.recents.is_empty() {
            "Recent ▾".to_owned()
        } else {
            format!("Recent ({}) ▾", self.recents.len())
        };
        ui.menu_button(label, |ui| {
            if self.recents.is_empty() {
                ui.label("Nothing yet.");
                ui.weak("Connections appear here once they have worked.");
                return;
            }
            ui.set_min_width(320.0);
            for entry in self.recents.entries() {
                let identity = entry.identity();
                ui.horizontal(|ui| {
                    let pin = if entry.pinned { "★" } else { "☆" };
                    if ui
                        .small_button(pin)
                        .on_hover_text("Pin so it is kept and listed first")
                        .clicked()
                    {
                        to_pin = Some(identity.clone());
                    }
                    if ui
                        .button(entry.settings.description())
                        .on_hover_text(format!(
                            "Last used {} · opened {} time(s)",
                            recents::relative_time(now, entry.last_used),
                            entry.uses
                        ))
                        .clicked()
                    {
                        to_open = Some(entry.settings.clone());
                        ui.close();
                    }
                    ui.weak(recents::relative_time(now, entry.last_used));
                    if ui.small_button("✖").on_hover_text("Forget this one").clicked() {
                        to_remove = Some(identity.clone());
                    }
                });
            }
            ui.separator();
            if ui
                .button("Clear history")
                .on_hover_text("Forget everything except pinned entries")
                .clicked()
            {
                clear = true;
                ui.close();
            }
        });

        if let Some(identity) = to_pin {
            self.recents.toggle_pin(&identity);
        }
        if let Some(identity) = to_remove {
            self.recents.remove(&identity);
        }
        if clear {
            self.recents.clear_unpinned();
        }
        if let Some(settings) = to_open {
            let ctx = ui.ctx().clone();
            self.open_recent(settings, &ctx);
        }
    }

    /// Shown in place of the dock when every tab has been closed.
    ///
    /// This is where the recents list earns its keep: a menu tucked into the toolbar mostly does
    /// not get found, whereas an empty window is exactly the moment someone wants to reopen
    /// something.
    fn launcher(&mut self, ui: &mut Ui) {
        let mut to_open = None;
        let mut new_tab = false;
        let now = recents::now_seconds();

        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            // Fill the pane, or the frame's background stops at its content and leaves a seam
            // against the darker area behind it.
            ui.set_min_size(ui.available_size());
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading("No open connections");
                ui.add_space(4.0);
                if ui.button("＋  New connection").clicked() {
                    new_tab = true;
                }
                ui.add_space(24.0);

                if self.recents.is_empty() {
                    ui.weak("Connections you use will be listed here for one-click reopening.");
                    return;
                }

                ui.label("Recent connections");
                ui.add_space(6.0);
                // Bounded so a long history cannot push the button off-screen.
                for entry in self.recents.entries().iter().take(10) {
                    ui.horizontal(|ui| {
                        // Clamped at zero: a narrow pane would otherwise ask for negative
                        // padding to centre a row wider than the space available.
                        ui.add_space((ui.available_width() / 2.0 - 190.0).max(0.0));
                        if entry.pinned {
                            ui.label("★");
                        }
                        if ui
                            .add_sized(
                                [260.0, 24.0],
                                egui::Button::new(entry.settings.description()),
                            )
                            .clicked()
                        {
                            to_open = Some(entry.settings.clone());
                        }
                        ui.weak(recents::relative_time(now, entry.last_used));
                    });
                }
            });
        });

        if new_tab {
            let id = self.new_session();
            self.dock.push_to_focused_leaf(id);
        }
        if let Some(settings) = to_open {
            let ctx = ui.ctx().clone();
            self.open_recent(settings, &ctx);
        }
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

                self.recents_menu(ui);

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
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        persist::save(storage, &self.snapshot(), self.unreadable_state.as_deref());
        // Written once; keep the live payload valid from here on.
        self.unreadable_state = None;
    }

    /// How often state is written while running.
    ///
    /// Shorter than eframe's 30 second default because everything worth saving here is cheap to
    /// serialise, and the window between a change and a crash is what gets lost.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(10)
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // Drain each session's task messages before drawing, so button states and any
        // errors reflect this frame.
        let now = recents::now_seconds();
        let mut established = Vec::new();
        for session in self.sessions.values_mut() {
            if session.poll(&self.rt, ui.ctx()) {
                established.push(session.settings.clone());
            }
        }
        // Only connections that actually worked are worth remembering.
        for settings in established {
            self.recents.record(&settings, now);
        }

        self.toolbar(ui);

        // With no tabs there is nothing for the dock to draw, and an empty window is exactly
        // when someone wants to reopen something.
        if self.dock.iter_all_tabs().next().is_none() {
            self.launcher(ui);
            return;
        }

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
