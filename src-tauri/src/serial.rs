//! Serial
//!
//! Serial port interface

use crate::state::AppState;

// use ansi_parser::{Output, AnsiParser};
// use ansi_parser::AnsiSequence;

// use cansi::*;

use tauri::command;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error};
use std::cmp::Ordering;


use crate::port::{Port, SerialReply};
use crate::port_settings::{PortSettings};
use crate::message::{DisplayMode, Message, MessageCommand, MessageData};

/// Wrapper to expose sending data over the serial port to the ui
///
/// # Arguments
///
/// * `state` - Application state
/// * `id` - UUID for the serial port to be opened
/// * `command` - Command to send to the serial port
#[command]
pub async fn send_message(state: AppState<'_>, id: String, message: Message) -> Result<(), String> {
    debug!(
        "Sending serial message through to port {} {:#?}",
        id.clone(),
        message.command
    );
    let state = state.0.lock().await;

    if state.connections.contains_key(&id) {
        match &state.connections[&id].bg_coms {
            Some(bg_coms) => match bg_coms.command_sender.send(message).await {
                Ok(_) => (),
                Err(e) => {
                    error!("Lost coms with serial port. {}", e);
                    return Err(format!("Lost coms with serial port. {}", e));
                }
            },
            None => {
                error!("Serial port not currently connected.");
                return Err("Serial port not currently connected.".to_string());
            }
        }
    }
    Ok(())
}

/// Open the serial port then monitor for data to send to or receive from the port.
///
/// # Arguments
///
/// * `options` - Serial port connection parameters
/// * `window` - Tauri window to send data to
/// * `command_receiver` - Communication channel to send commands to the serial port
pub async fn run(
    settings: PortSettings,
    window: tauri::Window,
    mut command_receiver: mpsc::Receiver<Message>,
) -> Result<(), String> {
    let mut serial = match tokio_serial::SerialStream::open(
        &tokio_serial::new(
            &settings.name,
            settings.baud_rate,
        )
        .data_bits(settings.data_bits.clone().into())
        .flow_control(settings.flow_control.clone().into())
        .parity(settings.parity.clone().into())
        .stop_bits(settings.stop_bits.clone().into())
    ) {
        Ok(p) => p,
        Err(e) => {
            let payload = SerialReply {
                id: settings.id,
                command: "close".to_string(),
                data: format!(
                    "Unable to open port. {:#?} {:#?}",
                    e.kind, e.description
                ),
            };
            let _ = window.emit("serial", payload);
            return Err(format!("Unable to open serial port. {:#?}", e));
        }
    };

    let mut port_wrapper = Port::new(settings);

    // After port is opened loop forever waiting for incoming serial data or messages from the UI
    loop {
        // Check to see if there are messages from the UI
        match command_receiver.try_recv() {
            Ok(message) => {
                debug!("{:?}", message);
                match message.command {
                    MessageCommand::Tx => {
                        // debug!("Got Tx Command");
                        match message.package {
                            MessageData::Char(data) => {
                                debug!("Data u8 {:#?}", data);
                                serial.try_write(&[data]).map_err(|e| {
                                    format!("Unable to send command to serial port. {:#?}", e)
                                })?;
                            }
                            MessageData::String(data) => {
                                debug!("Data string {:#?}", data);
                                serial.try_write(data.as_bytes()).map_err(|e| {
                                    format!("Unable to send command to serial port. {:#?}", e)
                                })?;
                            }
                            MessageData::Chars(data) => {
                                debug!("Data char vector {:#?}", data);
                                serial.try_write(&data).map_err(|e| {
                                    format!("Unable to send command to serial port. {:#?}", e)
                                })?;
                            }
                            _ => {
                                debug!("Data format unknown");
                            }
                        }
                    }
                    MessageCommand::Settings => {
                        match message.package {
                            MessageData::DisplaySettings(data) => {
                                // debug!("Settings {:#?}",data);
                                // debug!("current {:#?}",port_wrapper.max_bytes);

                                match data.max_bytes.cmp(&port_wrapper.max_bytes) {
                                    Ordering::Greater => {
                                        // calculate how many bytes to add
                                        let at = data.max_bytes - port_wrapper.max_bytes;
                                        // create a vector of null characters
                                        let mut new = vec![0; at];
                                        // if output doe not start with a null char add a new line char to seperate
                                        // the nulls we are adding from the existing data in output
                                        if !port_wrapper.output.is_empty() && port_wrapper.output[0] != 0
                                        {
                                            // add a new line if there isn't one
                                            new.push(10);
                                        }
                                        // combine the two vectors
                                        new.append(&mut port_wrapper.output);
                                        // update output with the new vector
                                        port_wrapper.output = new;
                                    },
                                    Ordering::Less => {
                                        let at = port_wrapper.max_bytes - data.max_bytes;
                                        // debug!("at {:#?}", at);
                                        if at <= port_wrapper.output.len() {
                                            let v2 = port_wrapper.output.split_off(at);
                                            port_wrapper.output = v2;
                                        }
                                    },
                                    Ordering::Equal => {},
                                }

                                port_wrapper.max_bytes = data.max_bytes;
                                port_wrapper.display_mode = data.display_mode;

                                // resend output based on new display settings
                                let payload = port_wrapper.package_output();
                                match payload {
                                    Some(p) => {
                                        window.emit("serial", p).map_err(|e| {
                                            format!("Unable to send to the UI. {}", e)
                                        })?;
                                    }
                                    None => {}
                                }
                            }
                            _ => {
                                debug!("Data format unknown");
                            }
                        }
                    },
                    MessageCommand::Clear => {
                        port_wrapper.output.clear();
                        let payload = port_wrapper.package_output();
                        match payload {
                            Some(p) => {
                                window
                                    .emit("serial", p)
                                    .map_err(|e| format!("Unable to send to the UI. {}", e))?;
                            }
                            None => {}
                        }
                    },
                    MessageCommand::Logging => if let MessageData::LogSettings(data) = message.package {
                        port_wrapper.log_enabled = data.enabled;
                        port_wrapper.log_path = data.path;
                    },
                }
            }
            Err(_e) => {} // if there is nothing to rx then move on with life
        }

        // Check to see if there is serial data to process
        match serial.try_read(&mut port_wrapper.buffer) {
            // If there is no data then break out of this iteration of the loop
            Ok(0) => break,
            // Process the available serial data. Count is the number of available bytes.
            Ok(count) => {
                // There is a logic flaw here when the buffer is larger than max_bytes.

                // Add the bytes to the end of the output buffer
                port_wrapper
                    .output
                    .extend_from_slice(&port_wrapper.buffer[..count]);

                // If the output vector is larger than the user specified setting, pop off the oldest data (at the beginning of the vector)
                if port_wrapper.output.len() > port_wrapper.max_bytes {
                    let at = port_wrapper.output.len() - port_wrapper.max_bytes;
                    if at <= port_wrapper.output.len() {
                        let temp = port_wrapper.output.split_off(at);
                        port_wrapper.output = temp;
                    }
                }
                
                // Send the new bytes to the UI
                match port_wrapper.display_mode {
                    DisplayMode::Ansi => {
                        let payload = port_wrapper.package_output();
                        match payload {
                            Some(p) => {
                                window
                                    .emit("serial", p)
                                    .map_err(|e| format!("Unable to send to the UI. {}", e))?;
                            }
                            None => {}
                        }
                    },
                    _ => {
                        let payload = port_wrapper.package_buffer(count);
                        match payload {
                            Some(p) => {
                                window
                                    .emit("serial", p)
                                    .map_err(|e| format!("Unable to send to the UI. {}", e))?;
                            }
                            None => {}
                        }
                    }
                }


                // If logging is enabled, write the new bytes to the log file
                if port_wrapper.log_enabled {
                    let payload = port_wrapper.log_buffer(count).await;
                    match payload {
                        Some(p) => {
                            window
                                .emit("serial", p)
                                .map_err(|e| format!("Unable to send to log to file. {}", e))?;
                        }
                        None => {}
                    }
                }

                
            }
            Err(_e) => {} // if there is nothing to read then move on with life
        };

        sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
