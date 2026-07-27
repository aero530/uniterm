//! Transport abstraction over serial and SSH.
//!
//! An enum rather than a trait object: async trait methods are not dyn-compatible, and with
//! two variants an enum is both simpler and cheaper than pulling in `async-trait`.
//!
//! The session loop is written against this, so adding the reconnect button (plan task 3)
//! means driving one state machine rather than two.

use russh::ChannelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialStream;

use super::ssh::SshTransport;
use crate::discovery;

/// Size of each serial read.
const READ_BUFFER: usize = 8192;

/// What came back from the link.
pub enum Incoming {
    /// Received bytes.
    Data(Vec<u8>),
    /// The link ended. `None` is a clean end, `Some` carries the reason.
    Closed(Option<String>),
}

/// A live connection.
pub enum Transport {
    Serial(SerialTransport),
    Ssh(SshTransport),
}

impl Transport {
    /// Wait for the next bytes, or for the link to end.
    pub async fn recv(&mut self) -> Incoming {
        match self {
            Self::Serial(serial) => serial.recv().await,
            Self::Ssh(ssh) => {
                // Both stdout and stderr of the remote shell are terminal output.
                match ssh.read_half().wait().await {
                    Some(ChannelMsg::Data { data }) => Incoming::Data(data.to_vec()),
                    Some(ChannelMsg::ExtendedData { data, .. }) => Incoming::Data(data.to_vec()),
                    Some(ChannelMsg::Eof | ChannelMsg::Close) | None => Incoming::Closed(Some(
                        "The remote host closed the connection.".to_owned(),
                    )),
                    Some(ChannelMsg::ExitStatus { exit_status }) => Incoming::Closed(Some(
                        format!("The remote shell exited with status {exit_status}."),
                    )),
                    // Window adjustments and the like: nothing to show.
                    Some(_) => Incoming::Data(Vec::new()),
                }
            }
        }
    }

    pub async fn send(&mut self, data: &[u8]) -> Result<(), String> {
        match self {
            Self::Serial(serial) => serial.send(data).await,
            Self::Ssh(ssh) => ssh.send(data).await,
        }
    }

    /// Tell the far end the terminal size changed. A no-op for serial, which has no concept
    /// of one.
    pub async fn resize(&mut self, columns: u16, rows: u16) -> Result<(), String> {
        match self {
            Self::Serial(_) => Ok(()),
            Self::Ssh(ssh) => ssh.resize(columns, rows).await,
        }
    }

    /// Periodic liveness check.
    ///
    /// Serial polls for the port still being present, which is how an unplugged USB adapter
    /// is noticed. SSH relies on russh's keepalive, which surfaces a dead link by closing the
    /// channel, so there is nothing to poll.
    pub async fn check_alive(&mut self) -> Option<String> {
        match self {
            Self::Serial(serial) => serial.check_alive().await,
            Self::Ssh(_) => None,
        }
    }

    pub async fn close(self) {
        match self {
            Self::Serial(_) => {}
            Self::Ssh(ssh) => ssh.close().await,
        }
    }
}

/// A live serial port.
pub struct SerialTransport {
    stream: SerialStream,
    name: String,
    buffer: Vec<u8>,
}

impl SerialTransport {
    pub fn new(stream: SerialStream, name: String) -> Self {
        Self {
            stream,
            name,
            buffer: vec![0; READ_BUFFER],
        }
    }

    async fn recv(&mut self) -> Incoming {
        match self.stream.read(&mut self.buffer).await {
            // A real end-of-stream. The Tauri build treated this as `break` and reported
            // success, so the UI kept showing a connected port.
            Ok(0) => Incoming::Closed(Some("Port closed the connection.".to_owned())),
            Ok(count) => Incoming::Data(self.buffer[..count].to_vec()),
            Err(e) => Incoming::Closed(Some(format!("Read failed: {e}"))),
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<(), String> {
        self.stream
            .write_all(data)
            .await
            .map_err(|e| format!("Write failed: {e}"))
    }

    async fn check_alive(&mut self) -> Option<String> {
        let name = self.name.clone();
        // Enumeration is a blocking syscall, so it does not belong inline in an async task.
        let present = tokio::task::spawn_blocking(move || discovery::port_present(&name))
            .await
            .unwrap_or(true);
        if present {
            None
        } else {
            Some(format!("{} disappeared (device unplugged?).", self.name))
        }
    }
}

/// Open a serial port.
pub fn open_serial(settings: &crate::settings::SerialSettings) -> Result<Transport, String> {
    let builder = tokio_serial::new(&settings.name, settings.baud_rate)
        .data_bits(settings.data_bits.into())
        .flow_control(settings.flow_control.into())
        .parity(settings.parity.into())
        .stop_bits(settings.stop_bits.into());

    match SerialStream::open(&builder) {
        Ok(stream) => Ok(Transport::Serial(SerialTransport::new(
            stream,
            settings.name.clone(),
        ))),
        Err(e) => Err(format!("Unable to open {}: {e}", settings.name)),
    }
}
