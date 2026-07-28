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
mod persist;
mod recents;
mod session;
mod settings;
mod term;
mod ui;

use tracing_subscriber::EnvFilter;

/// Window icon, embedded so the binary stays self-contained now that the Tauri bundler is
/// no longer wiring up resources.
const ICON_PNG: &[u8] = include_bytes!("../resources/icons/128x128.png");

/// Wayland has no window icon of its own: the compositor finds one by matching the surface's
/// app id to a `.desktop` file of the same name. snapd installs ours as
/// `<snap instance>_uniterm.desktop`, so inside a snap the id has to match that or the window
/// shows a generic placeholder. `SNAP_INSTANCE_NAME` is set by snapd and accounts for parallel
/// installs, where the instance is `uniterm_foo` rather than `uniterm`.
fn app_id(snap_instance: Option<std::ffi::OsString>) -> String {
    match snap_instance.as_deref().and_then(|s| s.to_str()) {
        Some(instance) if !instance.is_empty() => format!("{instance}_uniterm"),
        _ => "uniterm".to_owned(),
    }
}

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
        .with_app_id(app_id(std::env::var_os("SNAP_INSTANCE_NAME")))
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
            Ok(Box::new(app::UniTermApp::new(
                handle,
                cc.storage,
                &cc.egui_ctx,
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_is_the_plain_name_outside_a_snap() {
        assert_eq!(app_id(None), "uniterm");
        assert_eq!(app_id(Some("".into())), "uniterm");
    }

    /// Must line up with the desktop file snapd generates, or the window has no icon on
    /// Wayland. See `app_id`.
    #[test]
    fn app_id_matches_the_desktop_file_snapd_installs() {
        assert_eq!(app_id(Some("uniterm".into())), "uniterm_uniterm");
        // Parallel install: `snap install --name uniterm_dev uniterm`.
        assert_eq!(app_id(Some("uniterm_dev".into())), "uniterm_dev_uniterm");
    }
}
