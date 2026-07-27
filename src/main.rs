//! UniTerm — serial port terminal.
//!
//! Native egui application. Replaces the Tauri 2 + SvelteKit build; see PLAN.md task 1.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app;
mod discovery;
mod knownhosts;
mod session;
mod settings;
mod term;
mod ui;

use tracing_subscriber::EnvFilter;

/// Window icon, embedded so the binary stays self-contained now that the Tauri bundler is
/// no longer wiring up resources.
const ICON_PNG: &[u8] = include_bytes!("../resources/icons/128x128.png");

fn main() -> eframe::Result {
    // The Tauri build hard-coded `Level::TRACE`, which buried anything useful. Default to
    // warnings and let `RUST_LOG` turn detail back on.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,uniterm=info")),
        )
        .init();

    // eframe owns the main thread, so the async work runs on its own runtime and talks to
    // the UI over channels.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("uniterm-io")
        .build()
        .expect("failed to start the tokio runtime");
    let handle = runtime.handle().clone();

    // The default size is in logical points, so on a scaled display it can exceed the
    // physical screen — at 150% a 1600x1000 request is a 2400x1500 window, which pushes the
    // per-tab controls off the bottom of the screen with no way to reach them. Ask for a
    // conservative size and let winit clamp it to the monitor.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 820.0])
        .with_min_inner_size([720.0, 480.0])
        .with_clamp_size_to_monitor_size(true)
        .with_title("UniTerm");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "UniTerm",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.all_styles_mut(|style| {
                style.visuals.window_corner_radius = 4.0.into();
            });
            Ok(Box::new(app::UniTermApp::new(handle)))
        }),
    )
}
