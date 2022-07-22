use serde::{Deserialize, Serialize};


/// Settings to configure the serial port
#[derive(Clone)]
pub struct PortSettings {
    /// UUID of the serial port
    pub id: String,
    /// Port name
    pub name: String,
    /// Port connection speed
    pub baud_rate: u32,
    /// Number of bits per character
    pub data_bits: DataBits,
    /// Flow control modes
    pub flow_control: FlowControl,
    /// Parity checking modes
    pub parity: Parity,
    /// Number of stop bits
    pub stop_bits: StopBits,
}

/// Flow control modes
#[derive(Clone, Deserialize, Serialize)]
pub enum FlowControl {
    /// No flow control.
    None,
    /// Flow control using XON/XOFF bytes.
    Software,
    /// Flow control using RTS/CTS signals.
    Hardware,
}

impl From<FlowControl> for tokio_serial::FlowControl {
    fn from(value : FlowControl) -> Self {
        match value {
            FlowControl::None => tokio_serial::FlowControl::None,
            FlowControl::Software => tokio_serial::FlowControl::Software,
            FlowControl::Hardware => tokio_serial::FlowControl::Hardware,
        }
    }
}

/// Number of bits per character
#[derive(Clone, Deserialize, Serialize)]
pub enum DataBits {
    /// 5 bits per character
    Five,
    /// 6 bits per character
    Six,
    /// 7 bits per character
    Seven,
    /// 8 bits per character
    Eight,
}

impl From<DataBits> for tokio_serial::DataBits {
    fn from(value : DataBits) -> Self {
        match value {
            DataBits::Five => tokio_serial::DataBits::Five,
            DataBits::Six => tokio_serial::DataBits::Six,
            DataBits::Seven => tokio_serial::DataBits::Seven,
            DataBits::Eight => tokio_serial::DataBits::Eight,
        }
    }
}

/// Parity checking modes
///
/// When parity checking is enabled (Odd or Even) an extra bit is transmitted with each character. The value of 
/// the parity bit is arranged so that the number of 1 bits in the character (including the parity bit) is an 
/// even number (Even) or an odd number (Odd).
///
/// Parity checking is disabled by setting None, in which case parity bits are not transmitted.
#[derive(Clone, Deserialize, Serialize)]
pub enum Parity {
    /// No parity bit
    None,
    /// Parity bit sets odd number of 1 bits
    Odd,
    /// Parity bit sets even number of 1 bits
    Even,
}

impl From<Parity> for tokio_serial::Parity {
    fn from(value : Parity) -> Self {
        match value {
            Parity::None => tokio_serial::Parity::None,
            Parity::Odd => tokio_serial::Parity::Odd,
            Parity::Even => tokio_serial::Parity::Even,
        }
    }
}

/// Number of stop bits
///
/// Stop bits are transmitted after every character.
#[derive(Clone, Deserialize, Serialize)]
pub enum StopBits {
    /// One stop bit.
    One,
    /// Two stop bits
    Two,
}

impl From<StopBits> for tokio_serial::StopBits {
    fn from(value : StopBits) -> Self {
        match value {
            StopBits::One => tokio_serial::StopBits::One,
            StopBits::Two => tokio_serial::StopBits::Two,
        }
    }
}