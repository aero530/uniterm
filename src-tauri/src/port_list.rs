use serde::{Deserialize, Serialize};
use tauri::command;
use tokio_serial::available_ports;

/// Connection location for this port
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum PortType {
    UsbPort,
    PciPort,
    BluetoothPort,
    #[default]
    Unknown,
}

/// Reply format used to send data to the ui
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SerialPortType {
    pub name: String,
    /// Connection location
    pub port_type: PortType,
    /// Product name
    pub product: String,
    /// SN of the device
    pub serial_number: String,
    /// MFG of the device
    pub manufacturer: String,
}

/// Return a list of available ports to the ui
#[command]
pub async fn get_port_list() -> Result<Vec<SerialPortType>, String> {
    let ports = available_ports().unwrap();
    let mut reply = Vec::new();
    for p in ports {
        let mut o = SerialPortType::default();
        o.name = p.port_name;
        o.port_type = match p.port_type {
            tokio_serial::SerialPortType::UsbPort(u) => {
                o.product = u.product.unwrap_or_default();
                o.serial_number = u.serial_number.unwrap_or_default();
                o.manufacturer = u.manufacturer.unwrap_or_default();
                PortType::UsbPort
            }
            tokio_serial::SerialPortType::PciPort => PortType::PciPort,
            tokio_serial::SerialPortType::BluetoothPort => PortType::BluetoothPort,
            tokio_serial::SerialPortType::Unknown => PortType::Unknown,
        };
        reply.push(o);
    }
    Ok(reply)
}
