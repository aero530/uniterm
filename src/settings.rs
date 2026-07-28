//! Connection and display settings.
//!
//! Ported from the Tauri build's `port_settings.rs` and the display-mode half of
//! `message.rs`. These types are deliberately free of any UI or transport dependency so
//! that the SSH work (plan task 2) can wrap them in a `ConnectionKind` enum without
//! touching the widgets.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which kind of connection a tab holds.
///
/// Serial parameters (baud, parity, data bits, stop bits) are meaningless over SSH, and SSH
/// parameters are meaningless over serial, so the UI is kind-aware rather than greying out
/// half its controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionKind {
    #[default]
    Serial,
    Ssh,
}

impl ConnectionKind {
    pub const ALL: &'static [Self] = &[Self::Serial, Self::Ssh];

    pub fn label(self) -> &'static str {
        match self {
            Self::Serial => "Serial",
            Self::Ssh => "SSH",
        }
    }
}

/// Everything needed to open a connection of either kind.
///
/// Both sub-structs are kept populated regardless of the selected kind, so switching kind
/// back and forth does not discard what you already typed.
///
/// Note what is *absent*: passwords and key passphrases. Those live only in
/// [`crate::session::Session`] for the lifetime of the process and are never written here,
/// because this struct is what plan tasks 4 and 5 will persist to disk.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectionSettings {
    pub kind: ConnectionKind,
    pub serial: SerialSettings,
    pub ssh: SshSettings,
}

impl ConnectionSettings {
    /// Short label for the tab header.
    pub fn label(&self) -> String {
        match self.kind {
            ConnectionKind::Serial => {
                if self.serial.name.is_empty() {
                    "(no port)".to_owned()
                } else {
                    self.serial.name.clone()
                }
            }
            ConnectionKind::Ssh => {
                if self.ssh.host.is_empty() {
                    "(no host)".to_owned()
                } else if self.ssh.user.is_empty() {
                    self.ssh.host.clone()
                } else {
                    format!("{}@{}", self.ssh.user, self.ssh.host)
                }
            }
        }
    }

    /// Stable key identifying this connection, used to deduplicate the recents list.
    ///
    /// Serial includes the line parameters, not just the port name: for a serial tool the baud
    /// rate and framing are part of what you are trying to remember, so `COM3 @ 9600 8N1` and
    /// `COM3 @ 115200 8N1` are properly two different things to reopen.
    ///
    /// SSH keys on `user@host:port` only. The authentication method is how you get in, not what
    /// you are connecting to, so switching from a password to a key does not create a duplicate.
    pub fn identity(&self) -> String {
        match self.kind {
            ConnectionKind::Serial => {
                let s = &self.serial;
                format!(
                    "serial:{}@{}:{:?}:{:?}:{:?}:{:?}",
                    s.name, s.baud_rate, s.data_bits, s.parity, s.stop_bits, s.flow_control
                )
            }
            ConnectionKind::Ssh => format!("ssh:{}", self.ssh.identity()),
        }
    }

    /// Longer, human-readable description for the recents list.
    pub fn description(&self) -> String {
        match self.kind {
            ConnectionKind::Serial => {
                let s = &self.serial;
                format!("{} · {} baud", s.name, s.baud_rate)
            }
            ConnectionKind::Ssh => format!("{} · {}", self.ssh.identity(), self.ssh.auth.label()),
        }
    }

    /// Whether enough has been filled in to attempt a connection.
    pub fn is_complete(&self) -> Result<(), &'static str> {
        match self.kind {
            ConnectionKind::Serial if self.serial.name.is_empty() => Err("Select a port first."),
            ConnectionKind::Ssh if self.ssh.host.is_empty() => Err("Enter a host first."),
            ConnectionKind::Ssh if self.ssh.user.is_empty() => Err("Enter a username first."),
            _ => Ok(()),
        }
    }
}

/// How to authenticate an SSH connection.
///
/// ssh-agent is deliberately absent: it needs a named-pipe transport on Windows and a
/// separate code path, and a half-working option is worse than no option. It belongs with
/// the credential-storage work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshAuth {
    #[default]
    Password,
    /// A private key file on disk, optionally passphrase-protected.
    PublicKey,
}

impl SshAuth {
    pub const ALL: &'static [Self] = &[Self::Password, Self::PublicKey];

    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::PublicKey => "Private key",
        }
    }
}

/// Everything needed to open an SSH session, minus the secrets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// Private key file, used when `auth` is [`SshAuth::PublicKey`].
    pub key_path: Option<PathBuf>,
    /// `TERM` advertised to the remote end.
    pub term: String,
    /// Host key store to use. `None` means `~/.ssh/known_hosts`.
    ///
    /// Overridable so a session can be pointed at a separate store, and so tests can verify
    /// the trust policy without touching the user's real file.
    pub known_hosts: Option<PathBuf>,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: SshAuth::default(),
            key_path: None,
            // 256-colour is what the emulator actually supports, so claim it.
            term: "xterm-256color".to_owned(),
            known_hosts: None,
        }
    }
}

impl SshSettings {
    /// `user@host:port`, the identity used for the recent-connections list and known_hosts.
    pub fn identity(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

/// Everything needed to open a serial port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialSettings {
    /// Port name, e.g. `COM3` or `/dev/ttyUSB0`.
    pub name: String,
    /// Connection speed in baud.
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub flow_control: FlowControl,
    pub parity: Parity,
    pub stop_bits: StopBits,
    /// USB serial number of the device this port belongs to, when known.
    ///
    /// Recorded so a replugged adapter can be found again even if the operating system hands
    /// it a different port number, which Windows routinely does.
    pub usb_serial: Option<String>,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            baud_rate: 115_200,
            data_bits: DataBits::Eight,
            flow_control: FlowControl::None,
            parity: Parity::None,
            stop_bits: StopBits::One,
            usb_serial: None,
        }
    }
}

/// Flow control modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowControl {
    /// No flow control.
    #[default]
    None,
    /// Flow control using XON/XOFF bytes.
    Software,
    /// Flow control using RTS/CTS signals.
    Hardware,
}

impl From<FlowControl> for tokio_serial::FlowControl {
    fn from(value: FlowControl) -> Self {
        match value {
            FlowControl::None => Self::None,
            FlowControl::Software => Self::Software,
            FlowControl::Hardware => Self::Hardware,
        }
    }
}

/// Number of bits per character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    #[default]
    Eight,
}

impl From<DataBits> for tokio_serial::DataBits {
    fn from(value: DataBits) -> Self {
        match value {
            DataBits::Five => Self::Five,
            DataBits::Six => Self::Six,
            DataBits::Seven => Self::Seven,
            DataBits::Eight => Self::Eight,
        }
    }
}

/// Parity checking modes.
///
/// When parity checking is enabled an extra bit is transmitted with each character, chosen
/// so the number of 1 bits (including the parity bit) is even (`Even`) or odd (`Odd`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

impl From<Parity> for tokio_serial::Parity {
    fn from(value: Parity) -> Self {
        match value {
            Parity::None => Self::None,
            Parity::Odd => Self::Odd,
            Parity::Even => Self::Even,
        }
    }
}

/// Number of stop bits transmitted after every character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopBits {
    #[default]
    One,
    Two,
}

impl From<StopBits> for tokio_serial::StopBits {
    fn from(value: StopBits) -> Self {
        match value {
            StopBits::One => Self::One,
            StopBits::Two => Self::Two,
        }
    }
}

/// How incoming bytes are interpreted for display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisplayMode {
    /// Text, with escape sequences shown as visible control pictures.
    #[default]
    Ascii,
    /// Text, with SGR escape sequences applied as colours and attributes.
    Ansi,
    /// One decimal number per byte.
    Decimal,
    /// One hex number per byte.
    Hex,
}

/// How text typed into the send box is converted to bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendMode {
    #[default]
    Ascii,
    Decimal,
    Hex,
}

/// Baud rates offered in the UI. Ported from `PortMenuOptions.ts`.
pub const BAUD_RATES: &[u32] = &[
    300, 600, 1200, 1800, 2400, 4000, 4800, 7200, 9600, 14_400, 16_000, 19_200, 28_800, 38_400,
    51_200, 56_000, 57_600, 64_000, 76_800, 115_200, 128_000, 153_600, 230_400, 250_000, 256_000,
    460_800, 500_000, 576_000, 921_600, 1_000_000, 1_200_000, 1_500_000, 2_000_000, 2_250_000,
    3_000_000, 4_500_000,
];

/// Render a baud rate the way the old dropdown did.
pub fn baud_label(baud: u32) -> String {
    if baud >= 1_000_000 {
        format!("{:.2} Mbaud", baud as f64 / 1e6)
    } else if baud >= 10_000 {
        format!("{:.1} kbaud", baud as f64 / 1e3)
    } else {
        format!("{baud} baud")
    }
}

impl FlowControl {
    pub const ALL: &'static [Self] = &[Self::None, Self::Software, Self::Hardware];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No Flow Ctrl",
            Self::Software => "Software",
            Self::Hardware => "Hardware",
        }
    }
}

impl DataBits {
    pub const ALL: &'static [Self] = &[Self::Five, Self::Six, Self::Seven, Self::Eight];

    pub fn label(self) -> &'static str {
        match self {
            Self::Five => "Five Data Bits",
            Self::Six => "Six Data Bits",
            Self::Seven => "Seven Data Bits",
            Self::Eight => "Eight Data Bits",
        }
    }
}

impl Parity {
    pub const ALL: &'static [Self] = &[Self::None, Self::Odd, Self::Even];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "No Parity",
            Self::Odd => "Odd Parity",
            Self::Even => "Even Parity",
        }
    }
}

impl StopBits {
    pub const ALL: &'static [Self] = &[Self::One, Self::Two];

    pub fn label(self) -> &'static str {
        match self {
            Self::One => "One Stop Bit",
            Self::Two => "Two Stop Bits",
        }
    }
}

impl DisplayMode {
    pub const ALL: &'static [Self] = &[Self::Ascii, Self::Ansi, Self::Decimal, Self::Hex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ascii => "ASCII",
            Self::Ansi => "ANSI",
            Self::Decimal => "Decimal",
            Self::Hex => "Hex",
        }
    }
}

impl SendMode {
    pub const ALL: &'static [Self] = &[Self::Ascii, Self::Decimal, Self::Hex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ascii => "Ascii",
            Self::Decimal => "Decimal",
            Self::Hex => "Hex",
        }
    }
}

/// Parse the contents of the send box into raw bytes.
///
/// Returns an error message suitable for showing to the user, matching the alerts the
/// Svelte `sendCommand` used to raise.
pub fn encode_send(
    text: &str,
    mode: SendMode,
    append_cr: bool,
    append_lf: bool,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match mode {
        SendMode::Ascii => out.extend_from_slice(text.as_bytes()),
        SendMode::Decimal | SendMode::Hex => {
            let radix = if matches!(mode, SendMode::Decimal) { 10 } else { 16 };
            for token in text.split([' ', ',', '\t']).filter(|t| !t.is_empty()) {
                let value = u32::from_str_radix(token, radix)
                    .map_err(|_| format!("`{token}` is not a valid {} value.", mode.label()))?;
                if value > 0xFF {
                    return Err(format!(
                        "Data is sent as bytes, so values must be <= 255 (0xFF). Got `{token}`. \
                         Separate multiple bytes with a comma or space."
                    ));
                }
                out.push(value as u8);
            }
        }
    }
    if append_cr {
        out.push(b'\r');
    }
    if append_lf {
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_send_appends_line_endings() {
        assert_eq!(encode_send("hi", SendMode::Ascii, true, true).unwrap(), b"hi\r\n");
        assert_eq!(encode_send("hi", SendMode::Ascii, false, false).unwrap(), b"hi");
    }

    #[test]
    fn numeric_send_accepts_comma_and_space() {
        assert_eq!(
            encode_send("1, 2 3", SendMode::Decimal, false, false).unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            encode_send("0a,ff", SendMode::Hex, false, false).unwrap(),
            vec![0x0a, 0xff]
        );
    }

    #[test]
    fn numeric_send_rejects_out_of_range() {
        assert!(encode_send("255", SendMode::Decimal, false, false).is_ok());
        assert!(encode_send("256", SendMode::Decimal, false, false).is_err());
        assert!(encode_send("ff", SendMode::Hex, false, false).is_ok());
        // 0x100 is 256, one past a byte.
        assert!(encode_send("100", SendMode::Hex, false, false).is_err());
        assert!(encode_send("1FF", SendMode::Hex, false, false).is_err());
    }

    #[test]
    fn numeric_send_rejects_garbage() {
        assert!(encode_send("zz", SendMode::Hex, false, false).is_err());
    }

    #[test]
    fn baud_labels_match_old_dropdown() {
        assert_eq!(baud_label(300), "300 baud");
        assert_eq!(baud_label(9600), "9600 baud");
        assert_eq!(baud_label(115_200), "115.2 kbaud");
        assert_eq!(baud_label(1_000_000), "1.00 Mbaud");
    }
}
