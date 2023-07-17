//! Background
//!
//! Manage background processes to run the serial interfaces.

use crate::message::Message;
use crate::serial::run;
use crate::port_settings::PortSettings;
use crate::port_list::check_port_present;

use std::thread;
use tokio::sync::{broadcast, mpsc};
use tokio::task;
use tracing::{debug, error};

/// Communications channel with the background task
#[derive(Clone)]
pub struct BgComs {
    /// Communication channel to send commands to the serial port
    pub command_sender: mpsc::Sender<Message>,
}

/// Thread control for the background task running this port connection
pub struct BgHandle {
    /// Thread handle
    pub handle: thread::JoinHandle<Result<(), String>>,
    /// Communication channel to shut down the background thread
    pub stop_sender: broadcast::Sender<()>,
}

impl BgHandle {
    /// Send command to shut down the background thread
    pub fn stop(&self) {
        debug!("Stopping background task.");
        // Error can only occur when channel is already closed
        let _ = self.stop_sender.send(());
    }
    /// Wait for the background thread to terminate
    pub fn wait_until_stopped(self) -> Result<(), String> {
        match self.handle.join() {
            Ok(result) => result,
            Err(e) => Err(format!("Interval thread error: {:#?}", e)),
        }
    }
}

/// Spawn a thread and return the handle and communications channel
///
/// # Arguments
///
/// * `window` - Number of pixels in the x direction
/// * `settings` - Settings for the serial port to be opened
pub fn spawn(window: tauri::Window, settings: PortSettings) -> (BgHandle, BgComs) {
    debug!("Starting background task for port {}", settings.id);
    let (stop_sender, _stop_receiver) = broadcast::channel(1);
    let (command_sender, command_receiver) = mpsc::channel(32);

    let stop_sender_2 = stop_sender.clone();
    // Errors from start are lost here.  That is why we emit them to the UI from within start / run
    let tokio_thread =
        thread::spawn(move || start(settings, window, stop_sender_2, command_receiver));
    (
        BgHandle {
            handle: tokio_thread,
            stop_sender,
        },
        BgComs { command_sender },
    )
}

/// Create race between background task and the stop signal and close on whichever completes first.
///
/// # Arguments
///
/// * `settings` - Settings for the serial port to be opened
/// * `window` - Number of pixels in the x direction
/// * `stop_receiver` - Communication channel to stop the background process
/// * `command_receiver` - Communication channel to send commands to the serial port
#[tokio::main]
async fn start(
    settings: PortSettings,
    window: tauri::Window,
    stop_sender: broadcast::Sender<()>,
    command_receiver: mpsc::Receiver<Message>,
) -> Result<(), String> {
    let port_name = settings.name.clone();
    let mut stop_receiver = stop_sender.subscribe();
    let handle = task::spawn(async move {
        tokio::select! {
            // _ = run(options, command_receiver) => {
            // 	return Ok(())
            // }
            result_1 = run(settings, window, command_receiver) => {
                match result_1 {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        error!("run error. {}",&e);
                        Err(e)
                    }
                }
            }
            result_2 = stop_receiver.recv() => {
                match result_2 {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        Err(e.to_string())
                    }
                }
            }
            // Background task to monitor serial port.  This way if connection is lost we can close the port properly
            result_3 = check_port_present(port_name) => {
                match result_3 {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        error!("{}",&e);
                        Err(e.to_string())
                    }
                }
            }
        }
    });

    let result = handle.await;
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("start error. {}", &e);
            Err(e.to_string())
        }
    }
}
