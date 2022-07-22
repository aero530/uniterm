use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt};
use tracing::{debug, error};
use serde::{Deserialize, Serialize};

use crate::port_settings::{PortSettings};
use crate::message::DisplayMode;
use crate::ansi_to_html::ansi_to_html;


/// Reply format used to send data to the ui
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SerialReply {
    /// UUID of the serial port this data came from
    #[serde(skip_deserializing)]
    pub id: String,
    /// Type of reply
    pub command: String,
    /// Data for the reply
    pub data: String,
}

/// Settings to configure the serial port
#[derive(Clone)]
pub struct Port {
    /// UUID of the serial port
    pub settings: PortSettings,
    /// Port name
    pub display_mode: DisplayMode,
    /// Max number of bytes to store in the output buffer
    pub max_bytes: usize,
    /// Buffer to store the stream of incoming data from the serial port
    pub buffer: [u8; 2000],
    /// History of data from the serial port.  This is to enable scroll back in the UI.
    pub output: Vec<u8>,
    /// Setting to enable / disable logging to a file
    pub log_enabled: bool,
    /// Path of file to store data to.
    pub log_path: String,
}

/// String representation of the enum value
impl Port {
    pub fn new(settings: PortSettings) -> Self {
        Self {
            settings,
            display_mode: DisplayMode::default(),
            max_bytes: 20000,
            buffer: [0; 2000],
            output: Vec::new(),
            log_enabled: false,
            log_path: "".to_string(),
        }
    }

    /// Parse a byte slice of data to a vector of strings for display & writing to file
    ///
    /// # Arguments
    ///
    /// * `bytes` - The data to format
    fn parse_data(&self, bytes: &[u8]) -> String {
        match self.display_mode {
            DisplayMode::Ascii => String::from_utf8_lossy(bytes).into_owned(),
            DisplayMode::Ansi => ansi_to_html(bytes),
            DisplayMode::Decimal => bytes.iter().map(|n| format!("{}", n)).collect::<Vec<String>>().join(" "),
            DisplayMode::Hex => bytes.iter().map(|n| format!("{:#04x}", n)).collect::<Vec<String>>().join(" "),
        }
    }

    /// Package up the data currently in the buffer for sending to the UI
    ///
    /// # Arguments
    ///
    /// * `count` - The number of bytes of data from buffer to include in the package
    pub fn package_buffer(&self, count: usize) -> Option<SerialReply> {
        let bytes = self.buffer[..count].to_vec();
        let data = self.parse_data(&bytes);

        let payload = SerialReply {
            id: self.settings.id.to_string(),
            command: "rx_bytes".to_string(),
            data,
        };

        Some(payload)
    }

    /// Write buffer to the log file
    ///
    /// # Arguments
    ///
    /// * `count` - The number of bytes of data from buffer to write from the buffer
    pub async fn log_buffer(&self, count: usize) -> Option<SerialReply> {
        let bytes = self.buffer[..count].to_vec();
        let data = self.parse_data(&bytes);

        let mut file = match OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.log_path.clone())
            .await
        {
            Ok(file) => file,
            Err(e) => {
                error!("Can't open the file.");
                let payload = SerialReply {
                    id: self.settings.id.to_string(),
                    command: "error".to_string(),
                    data: format!("{:#?}", e),
                };
                return Some(payload);
            }
        };

        match file.write_all(data.as_bytes()).await {
            Ok(()) => None,
            Err(e) => {
                let payload = SerialReply {
                    id: self.settings.id.to_string(),
                    command: "error".to_string(),
                    data: format!("{:#?}", e),
                };
                Some(payload)
            }
        }
    }

    /// Package up the entire output vector for sending to the UI
    ///
    /// This is used when the display mode is changed.
    pub fn package_output(&self) -> Option<SerialReply> {
        let data = self.parse_data(&self.output);

        let payload = SerialReply {
            id: self.settings.id.to_string(),
            command: "rx_buffer".to_string(),
            data,
        };

        Some(payload)
    }
}
