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
use tauri::Manager;

use tracing::Level;
use tracing_subscriber::FmtSubscriber;

mod ansi_to_html;
mod background;
mod message;
mod port;
mod port_list;
mod port_settings;
mod serial;
mod state;

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


    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())

        .setup(|app| {
            // `main` here is the window label; it is defined on the window creation or under `tauri.conf.json`
            // the default value is `main`. note that it must be unique
            // tauri::WebviewWindowBuilder::new(app, "label", tauri::WebviewUrl::App("index.html".into()))
            
            let handle = app.handle();

            let data = AppData {
                connections: BTreeMap::new(),
                // window: win,
                app_handle: handle.clone(),
            };
            app.manage(ArcMutex::new(data));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            state::open_connection,
            state::close_connection,
            serial::send_message,
            port_list::get_port_list,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

}
