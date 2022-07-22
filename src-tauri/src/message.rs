use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Package of data coming from the UI to the serial interface backend
pub struct Message {
    /// Command type
    pub command: MessageCommand,
    /// Data that goes along with the command
    pub package: MessageData,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
/// Different commands the UI can send to the serial interface backend
pub enum MessageCommand {
    /// Transmit data on the serial interface
    Tx,
    /// Update display settings
    Settings,
    /// Clear the output vector
    Clear,
    /// Update the log settings
    Logging,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
/// Message data formats
pub enum MessageData {
    /// Single number
    Char(u8),
    /// Generic string
    String(String),
    /// Vector of numbers
    Chars(Vec<u8>),
    /// Display settings
    DisplaySettings(DisplaySettings),
    /// Log file settings
    LogSettings(LogSettings),
}

/// Settings related to how the data should be displayed
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct DisplaySettings {
    /// Size of the output vector.  This defines the scrollback length in the UI
    pub max_bytes: usize,
    /// How the incoming bytes from the serial interface should be interpreted & displayed
    pub display_mode: DisplayMode,
}

/// How the incoming bytes from the serial interface should be interpreted & displayed
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub enum DisplayMode {
    /// Display as Ascii
    #[default]
    Ascii,
    /// Display as Ansi (todo)
    Ansi,
    /// Display as decimal
    Decimal,
    /// Display as hex
    Hex,
}

/// Settings related to if & how the incoming serial data should be logged
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogSettings {
    /// Should new data be written to a file
    pub enabled: bool,
    /// File path to the log file
    pub path: String,
}
