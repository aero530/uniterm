//! Application State
//!
//! Manage the data stored in the application's state.  This is primarily managing the various serial port connections.
//! A new background process is spawned for each port.

use crate::background;
use crate::port_settings::{PortSettings, DataBits, FlowControl, Parity, StopBits};
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::{command, State};
use tokio::sync::Mutex;
use tracing::{debug, error};

/// Connection info stored in the application state
pub struct Connection {
    /// Serial port name
    pub name: String,
    /// Serial port connection speed
    pub baud_rate: u32,
    /// Thread control for the background task running this port connection
    pub bg_handle: Option<background::BgHandle>,
    /// Communications channel with the background task
    pub bg_coms: Option<background::BgComs>,
}

/// Application state
pub type AppState<'a> = State<'a, ArcMutex>;

/// Arc Mutex around the application data
pub struct ArcMutex(pub Arc<Mutex<AppData>>);

impl ArcMutex {
    /// Create new arc mutex struct for use as the application state
    ///
    /// # Arguments
    ///
    /// * `data` - Application state data
    pub fn new(data: AppData) -> Self {
        Self(Arc::new(Mutex::new(data)))
    }
}

/// Application state data structure
pub struct AppData {
    /// Map of serial port connections
    pub connections: BTreeMap<String, Connection>,
    /// Application window
    pub window: tauri::Window,
}

impl AppData {
    /// Create port in application state and start background thread to open the port
    ///
    /// # Arguments
    ///
    /// * `settings` - Settings for the serial port to be opened
    pub fn open_connection(&mut self, settings: PortSettings) -> Result<(), String> {
        self.close_connection(&settings.id).unwrap_or_default();
        let background = background::spawn(self.window.clone(), settings.clone());
        let connection = Connection {
            name: settings.name,
            baud_rate: settings.baud_rate,
            bg_handle: Some(background.0),
            bg_coms: Some(background.1),
        };
        self.connections.insert(settings.id, connection);
        Ok(())
    }

    /// Close a serial port and stop the background process that monitors the port
    ///
    /// # Arguments
    ///
    /// * `id` - UUID for the serial port to be opened
    pub fn close_connection(&mut self, id: &str) -> Result<(), String> {
        let connection = match self.connections.get_mut(id) {
            Some(c) => c,
            None => return Ok(()),
        };

        if let Some(bg_handle) = connection.bg_handle.take() {
            bg_handle.stop();
            bg_handle.wait_until_stopped()?;
            // (self.connections[&id].bg_handle, self.connections[&id].bg_coms) = (None, None);
            self.connections.get_mut(id).unwrap().bg_handle = None;
            self.connections.get_mut(id).unwrap().bg_coms = None;
        } else {
            error!("No active connection to close");
            return Err("No active connection to close".to_string());
        }
        Ok(())
    }
}

/// Wrapper to expose opening a port to the ui
///
/// # Arguments
///
/// * `state` - Application state
/// * `id` - UUID for the serial port to be opened
/// * `name` - Port / name of the serial port (such as COM3)
/// * `baud_rate` - Connection speed to use (such as 115200)
#[command]
pub async fn open_connection(
    state: AppState<'_>,
    id: String,
    name: String,
    baud_rate: u32,
    data_bits: DataBits,
    flow_control: FlowControl,
    parity: Parity,
    stop_bits: StopBits,
) -> Result<(), String> {
    let mut state = state.0.lock().await;
    let settings = PortSettings {
        id: id.clone(),
        name: name.clone(),
        baud_rate,
        data_bits,
        flow_control,
        parity,
        stop_bits,
    };
    debug!("Opening connection to {} {} {}", name, baud_rate, id);
    state.open_connection(settings)?;
    Ok(())
}

/// Wrapper to expose port closing to the ui
///
/// # Arguments
///
/// * `state` - Application state
/// * `id` - UUID for the serial port to be opened
#[command]
pub async fn close_connection(state: AppState<'_>, id: String) -> Result<(), String> {
    let mut state = state.0.lock().await;
    state.close_connection(&id)?;
    Ok(())
}
