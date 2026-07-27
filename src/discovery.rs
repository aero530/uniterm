//! Serial port enumeration.
//!
//! Ported from the Tauri build's `port_list.rs`. The `#[command]` wrapper is gone, and
//! `list_ports` is no longer `async` — `tokio_serial::available_ports` is a blocking
//! syscall, so pretending otherwise only hid where the cost was.

use serde::{Deserialize, Serialize};

/// Where a port is attached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortKind {
    Usb,
    Pci,
    Bluetooth,
    #[default]
    Unknown,
}

/// A serial port offered to the user.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    /// Port name, e.g. `COM3`.
    pub name: String,
    pub kind: PortKind,
    /// USB product name, when known.
    pub product: String,
    /// USB serial number, when known. Useful for re-identifying a device across replug,
    /// which matters for reconnect (plan task 3) since Windows may hand back a
    /// different COM number.
    pub serial_number: String,
    /// USB manufacturer, when known.
    pub manufacturer: String,
}

impl PortInfo {
    /// Human-readable label. Ported from `formatPortName` in `PortMenu.svelte`.
    pub fn label(&self) -> String {
        match self.kind {
            PortKind::Usb if !self.product.is_empty() => {
                let mut out = String::new();
                if !self.product.contains(&self.name) {
                    out.push_str(&self.name);
                    out.push(' ');
                }
                if !self.manufacturer.is_empty() && !self.product.contains(&self.manufacturer) {
                    out.push_str(&self.manufacturer);
                    out.push(' ');
                }
                out.push_str(&self.product);
                out
            }
            PortKind::Usb => {
                if self.manufacturer.is_empty() {
                    self.name.clone()
                } else {
                    format!("{} - {}", self.name, self.manufacturer)
                }
            }
            PortKind::Pci => format!("{} - PCI", self.name),
            PortKind::Bluetooth => format!("{} - Bluetooth", self.name),
            PortKind::Unknown => self.name.clone(),
        }
    }
}

/// Enumerate the serial ports currently present on the system.
pub fn list_ports() -> Vec<PortInfo> {
    let ports = match tokio_serial::available_ports() {
        Ok(ports) => ports,
        Err(e) => {
            // The old code did `.unwrap()` here, which took the whole app down when
            // enumeration failed.
            tracing::warn!("could not enumerate serial ports: {e}");
            return Vec::new();
        }
    };

    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| {
            let mut info = PortInfo {
                name: p.port_name,
                ..Default::default()
            };
            info.kind = match p.port_type {
                tokio_serial::SerialPortType::UsbPort(usb) => {
                    info.product = usb.product.unwrap_or_default();
                    info.serial_number = usb.serial_number.unwrap_or_default();
                    info.manufacturer = usb.manufacturer.unwrap_or_default();
                    PortKind::Usb
                }
                tokio_serial::SerialPortType::PciPort => PortKind::Pci,
                tokio_serial::SerialPortType::BluetoothPort => PortKind::Bluetooth,
                tokio_serial::SerialPortType::Unknown => PortKind::Unknown,
            };
            info
        })
        .collect();

    out.sort_by(|a, b| natural_port_order(&a.name).cmp(&natural_port_order(&b.name)));
    out
}

/// Is a port with this name currently present?
///
/// Used to notice an unplugged adapter. This is the serial equivalent of the SSH
/// keepalive that plan task 3 needs.
pub fn port_present(name: &str) -> bool {
    match tokio_serial::available_ports() {
        Ok(ports) => ports.iter().any(|p| p.port_name == name),
        // If enumeration fails we cannot conclude the port is gone; assume it is still
        // there and let the next read error decide.
        Err(_) => true,
    }
}

/// Sort key that puts `COM9` before `COM10` instead of after it.
fn natural_port_order(name: &str) -> (String, u64) {
    let digits_at = name
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_ascii_digit())
        .map(|(i, _)| i)
        .last();

    match digits_at {
        Some(i) => (
            name[..i].to_string(),
            name[i..].parse::<u64>().unwrap_or(u64::MAX),
        ),
        None => (name.to_string(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn com_ports_sort_numerically() {
        let mut names = ["COM10", "COM9", "COM1", "COM20"];
        names.sort_by_key(|n| natural_port_order(n));
        assert_eq!(names, ["COM1", "COM9", "COM10", "COM20"]);
    }

    #[test]
    fn ports_without_digits_still_sort() {
        assert_eq!(natural_port_order("ttyS"), ("ttyS".to_string(), 0));
    }

    #[test]
    fn usb_label_avoids_duplicating_fields() {
        let info = PortInfo {
            name: "COM3".into(),
            kind: PortKind::Usb,
            product: "COM3 FTDI Widget".into(),
            manufacturer: "FTDI".into(),
            ..Default::default()
        };
        // Both name and manufacturer already appear in product, so neither is prepended.
        assert_eq!(info.label(), "COM3 FTDI Widget");
    }

    #[test]
    fn unknown_port_label_is_just_the_name() {
        let info = PortInfo {
            name: "COM7".into(),
            ..Default::default()
        };
        assert_eq!(info.label(), "COM7");
    }
}
