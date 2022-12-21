//! Uniterm
//!
//! uniterm
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use crate::state::{AppData, ArcMutex};
use std::collections::BTreeMap;
use std::env;
use tauri::{Manager, WindowBuilder, WindowUrl};
use tracing::{debug, Level};
use tracing_subscriber::FmtSubscriber;

mod background;
mod message;
mod port;
mod port_list;
mod port_settings;
mod serial;
mod state;
mod ansi_to_html;

#[tokio::main]
async fn main() {
    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(Level::TRACE)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let ctx = tauri::generate_context!();

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            state::open_connection,
            state::close_connection,
            serial::send_message,
            port_list::get_port_list,
            // port_list::monitor_serial_presence,
        ])
        .setup(move |app| {
            // `main` here is the window label; it is defined on the window creation or under `tauri.conf.json`
            // the default value is `main`. note that it must be unique
            let win = WindowBuilder::new(app, "main", WindowUrl::default())
                .title("UniTerm")
                .inner_size(1600.0, 1000.0) // 2600 1600
                .min_inner_size(400.0, 150.0)
                .build()
                .expect("Unable to create window");

            let data = AppData {
                connections: BTreeMap::new(),
                window: win,
            };
            app.manage(ArcMutex::new(data));

            Ok(())
        })
        // .menu(tauri::Menu::os_default(&ctx.package_info().name))
        .build(ctx)
        .expect("error while running tauri application");

    debug!("Starting application");
    app.run(|_app_handle, _e| {
        //     if let tauri::RunEvent::WindowEvent { event, .. } = e {
        //             if let tauri::WindowEvent::CloseRequested { api: _api, .. } = event {
        //                     #[cfg(target_os = "macos")]
        //                     {
        //                         // hide the application
        //                         // manual for now (PR https://github.com/tauri-apps/tauri/pull/3689)
        //                         _api.prevent_close();
        //                         use objc::*;
        //                         let cls = objc::runtime::Class::get("NSApplication").unwrap();
        //                         let app: cocoa::base::id = unsafe { msg_send![cls, sharedApplication] };
        //                         unsafe { msg_send![app, hide: 0] }
        //                     }
        //             }
        // }
    });
}
