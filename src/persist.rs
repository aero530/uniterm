//! Saved state.
//!
//! Layout and connection definitions are written when the window closes, and restored on the
//! next start. `eframe` provides *where* and *when* — a platform-appropriate directory, a save
//! on shutdown, and a periodic autosave — while the schema, its versioning and its recovery
//! behaviour live here.
//!
//! The payload is stored under a single key rather than going through [`eframe::set_value`],
//! which silently returns `None` on any problem. Owning the serialisation makes it possible to
//! tell "nothing saved yet" apart from "something saved but unreadable", and to tell the user
//! about the second.
//!
//! # Why RON and not JSON
//!
//! `DockState` stores an `egui::Rect` per node, and egui initialises those to `Rect::NOTHING`,
//! whose components are infinite. JSON has no representation for infinity: `serde_json` writes
//! `null` and then refuses to read it back as an `f32`, so a saved layout would never reload.
//! RON round-trips it, which is the same reason eframe uses RON internally.
//!
//! # What is deliberately not saved
//!
//! * **Passwords and key passphrases.** They are held in memory for the life of the process and
//!   never reach disk. A restored SSH tab using password auth therefore cannot dial on its own.
//! * **Scrollback contents.** Terminal output routinely contains secrets, and it can be
//!   megabytes; writing it to disk on every close is a data-sensitivity decision the user has
//!   not made.
//! * **Live connection state.** Restored tabs come back defined but disconnected unless the
//!   user opted a tab into auto-connect, for the reasons in [`may_auto_connect`].

use std::path::PathBuf;

use egui_dock::DockState;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::app::TabId;
use crate::discovery::PortInfo;
use crate::settings::{ConnectionKind, ConnectionSettings, DisplayMode, SendMode, SshAuth};

/// Bumped when the schema changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Storage key holding the state.
const KEY: &str = "uniterm-state";
/// Storage key where an unreadable payload is set aside instead of being destroyed.
const BACKUP_KEY: &str = "uniterm-state-unreadable";

/// Everything carried across a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    /// Next tab id, so restored ids are never reused.
    #[serde(default)]
    pub next_id: u64,
    pub dock: DockState<TabId>,
    #[serde(default)]
    pub tabs: Vec<PersistedTab>,
    /// Recently used connections. Added after version 1 and defaulted, so a file written by an
    /// earlier build still loads — which is why the schema version did not need bumping.
    #[serde(default)]
    pub recents: Vec<crate::recents::Recent>,
}

/// One tab's definition. Every field defaults so that adding one does not invalidate
/// files written by an older build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersistedTab {
    pub id: TabId,
    #[serde(default)]
    pub settings: ConnectionSettings,
    #[serde(default)]
    pub display_mode: DisplayMode,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_true")]
    pub enter_crlf: bool,
    #[serde(default)]
    pub auto_reconnect: bool,
    /// Dial this tab on startup. Off by default; see [`may_auto_connect`].
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub send_mode: SendMode,
    #[serde(default)]
    pub append_cr: bool,
    #[serde(default)]
    pub append_lf: bool,
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    #[serde(default)]
    pub log_enabled: bool,
}

fn default_max_bytes() -> usize {
    crate::term::DEFAULT_MAX_BYTES
}
fn default_font_size() -> f32 {
    13.0
}
fn default_true() -> bool {
    true
}

/// Outcome of trying to restore.
///
/// The restored state is boxed: it is much larger than the other variants, and this is returned
/// by value from a once-per-startup call where the indirection costs nothing.
pub enum Loaded {
    /// Nothing stored yet.
    Fresh,
    Restored(Box<PersistedState>),
    /// Something was stored but could not be used. The payload is handed back so it can be set
    /// aside on the next save rather than overwritten and lost.
    Unreadable { reason: String, payload: String },
}

/// Read the saved state.
///
/// Never returns an error: a file that cannot be used degrades to a fresh start, because a bad
/// config must not stop the application from opening. What it does *not* do is discard the bad
/// payload silently — see [`Loaded::Unreadable`].
pub fn load(storage: &dyn eframe::Storage) -> Loaded {
    let Some(payload) = storage.get_string(KEY) else {
        return Loaded::Fresh;
    };
    if payload.trim().is_empty() {
        return Loaded::Fresh;
    }

    match ron::from_str::<PersistedState>(&payload) {
        Ok(state) if state.version > SCHEMA_VERSION => {
            warn!(
                "saved state is version {} but this build understands {SCHEMA_VERSION}",
                state.version
            );
            Loaded::Unreadable {
                reason: format!(
                    "The saved layout was written by a newer version of UniTerm \
                     (format {} vs {SCHEMA_VERSION}), so it was not loaded. Starting fresh.",
                    state.version
                ),
                payload,
            }
        }
        Ok(state) => Loaded::Restored(Box::new(state)),
        Err(e) => {
            warn!("could not read saved state: {e}");
            Loaded::Unreadable {
                reason: format!("The saved layout could not be read ({e}). Starting fresh."),
                payload,
            }
        }
    }
}

/// Write the state, and set aside any payload that could not be read earlier.
pub fn save(storage: &mut dyn eframe::Storage, state: &PersistedState, unreadable: Option<&str>) {
    if let Some(payload) = unreadable {
        // Kept under its own key so the user has something to inspect or recover from.
        storage.set_string(BACKUP_KEY, payload.to_owned());
    }
    // Pretty-printed so the file is readable if someone needs to inspect or hand-edit it.
    match ron::ser::to_string_pretty(state, ron::ser::PrettyConfig::default()) {
        Ok(payload) => storage.set_string(KEY, payload),
        Err(e) => warn!("could not serialise state: {e}"),
    }
}

/// Whether a restored tab may dial on startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoConnect {
    Yes,
    /// Skipped, with something to show in the tab.
    No(String),
}

/// Decide whether a restored tab may connect without being asked.
///
/// Opening a port at startup is not free of consequence: it asserts control over hardware the
/// user has not touched yet, it can take a port another application wanted, and on Windows port
/// numbers are not stable — `COM3` today may be a different device tomorrow. So a recorded USB
/// serial number must still match, and a bare name match is only trusted when no serial was
/// recorded to check against.
pub fn may_auto_connect(settings: &ConnectionSettings, ports: &[PortInfo]) -> AutoConnect {
    if let Err(reason) = settings.is_complete() {
        return AutoConnect::No(reason.to_owned());
    }

    match settings.kind {
        ConnectionKind::Serial => {
            let serial = &settings.serial;
            let by_name = ports.iter().find(|p| p.name == serial.name);

            match (by_name, serial.usb_serial.as_deref()) {
                // Recorded a device identity: it has to match.
                (Some(port), Some(expected)) if port.serial_number == expected => AutoConnect::Yes,
                (Some(port), Some(expected)) => {
                    // Same port number, different hardware. Do not touch it.
                    let found = if port.serial_number.is_empty() {
                        "an unidentified device".to_owned()
                    } else {
                        format!("device {}", port.serial_number)
                    };
                    AutoConnect::No(format!(
                        "{} is now {found}, not the {expected} this tab was saved with. \
                         Not connecting automatically.",
                        serial.name
                    ))
                }
                // No identity recorded; a name match is the best that can be checked.
                (Some(_), None) => AutoConnect::Yes,
                // Gone from that port: follow the device if it moved.
                (None, Some(expected)) => {
                    if ports.iter().any(|p| p.serial_number == expected) {
                        AutoConnect::Yes
                    } else {
                        AutoConnect::No(format!("{} is not attached.", serial.name))
                    }
                }
                (None, None) => AutoConnect::No(format!("{} is not attached.", serial.name)),
            }
        }
        ConnectionKind::Ssh => match settings.ssh.auth {
            // The password was never written to disk, so there is nothing to dial with.
            SshAuth::Password => AutoConnect::No(
                "Passwords are not saved, so this tab needs its password before connecting."
                    .to_owned(),
            ),
            SshAuth::PublicKey => AutoConnect::Yes,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::PortKind;
    use eframe::Storage as _;
    use crate::settings::{SerialSettings, SshSettings};
    use std::collections::BTreeMap;

    /// In-memory stand-in for eframe's storage.
    #[derive(Default)]
    struct MemoryStorage {
        values: BTreeMap<String, String>,
    }

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.values.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.values.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.values.remove(key);
        }
        fn flush(&mut self) {}
    }

    fn sample_state() -> PersistedState {
        PersistedState {
            version: SCHEMA_VERSION,
            next_id: 7,
            dock: DockState::new(vec![TabId(3), TabId(5)]),
            recents: Vec::new(),
            tabs: vec![
                PersistedTab {
                    id: TabId(3),
                    settings: ConnectionSettings {
                        kind: ConnectionKind::Serial,
                        serial: SerialSettings {
                            name: "COM3".into(),
                            baud_rate: 9600,
                            usb_serial: Some("SN123".into()),
                            ..Default::default()
                        },
                        ssh: SshSettings::default(),
                    },
                    display_mode: DisplayMode::Hex,
                    max_bytes: 50_000,
                    font_size: 15.0,
                    enter_crlf: false,
                    auto_reconnect: true,
                    auto_connect: false,
                    send_mode: SendMode::Hex,
                    append_cr: true,
                    append_lf: false,
                    log_path: Some(PathBuf::from("/tmp/x.log")),
                    log_enabled: true,
                },
                PersistedTab {
                    id: TabId(5),
                    settings: ConnectionSettings {
                        kind: ConnectionKind::Ssh,
                        serial: SerialSettings::default(),
                        ssh: SshSettings {
                            host: "srv".into(),
                            user: "phil".into(),
                            port: 2222,
                            ..Default::default()
                        },
                    },
                    display_mode: DisplayMode::Ansi,
                    max_bytes: 200_000,
                    font_size: 13.0,
                    enter_crlf: true,
                    auto_reconnect: false,
                    auto_connect: true,
                    send_mode: SendMode::Ascii,
                    append_cr: false,
                    append_lf: true,
                    log_path: None,
                    log_enabled: false,
                },
            ],
        }
    }

    fn port(name: &str, serial: &str) -> PortInfo {
        PortInfo {
            name: name.into(),
            kind: PortKind::Usb,
            product: "Widget".into(),
            serial_number: serial.into(),
            manufacturer: "ACME".into(),
        }
    }

    #[test]
    fn nothing_saved_yet_is_a_fresh_start() {
        let storage = MemoryStorage::default();
        assert!(matches!(load(&storage), Loaded::Fresh));
    }

    #[test]
    fn an_empty_payload_is_a_fresh_start() {
        let mut storage = MemoryStorage::default();
        storage.set_string(KEY, "   ".to_owned());
        assert!(matches!(load(&storage), Loaded::Fresh));
    }

    #[test]
    fn state_round_trips_through_storage() {
        let mut storage = MemoryStorage::default();
        let original = sample_state();
        save(&mut storage, &original, None);

        let restored = match load(&storage) {
            Loaded::Restored(state) => state,
            _ => panic!("expected a restore"),
        };

        assert_eq!(restored.version, SCHEMA_VERSION);
        assert_eq!(restored.next_id, 7);
        assert_eq!(restored.tabs.len(), 2);

        let serial_tab = &restored.tabs[0];
        assert_eq!(serial_tab.id, TabId(3));
        assert_eq!(serial_tab.settings.serial.name, "COM3");
        assert_eq!(serial_tab.settings.serial.baud_rate, 9600);
        assert_eq!(serial_tab.settings.serial.usb_serial.as_deref(), Some("SN123"));
        assert_eq!(serial_tab.display_mode, DisplayMode::Hex);
        assert_eq!(serial_tab.max_bytes, 50_000);
        assert!(serial_tab.auto_reconnect);
        assert!(serial_tab.log_enabled);

        let ssh_tab = &restored.tabs[1];
        assert_eq!(ssh_tab.settings.ssh.host, "srv");
        assert_eq!(ssh_tab.settings.ssh.port, 2222);
        assert!(ssh_tab.auto_connect);

        // The layout came back with both tabs.
        let ids: Vec<_> = restored.dock.iter_all_tabs().map(|(_, id)| *id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&TabId(3)) && ids.contains(&TabId(5)));
    }

    #[test]
    fn no_secret_fields_appear_in_the_payload() {
        // The persistence property that matters most. Checked by field name rather than by
        // substring: `SshAuth::Password` is a *method* name and legitimately appears, so a naive
        // search for "password" matches something harmless and proves nothing.
        let mut storage = MemoryStorage::default();
        save(&mut storage, &sample_state(), None);
        let payload = storage.get_string(KEY).unwrap();

        for field in ["password:", "passphrase:", "credentials:", "secret:"] {
            assert!(
                !payload.to_lowercase().contains(field),
                "payload must not contain a `{field}` field:\n{payload}"
            );
        }
        // The auth *method* is expected, and is not a secret.
        assert!(payload.contains("Password"), "the auth method is still recorded");
    }

    #[test]
    fn the_saved_payload_survives_egui_infinities() {
        // Regression: a fresh DockState holds `Rect::NOTHING`, whose components are infinite.
        // JSON cannot represent those and silently turns them into `null`, which then fails to
        // load — so a saved layout would never come back.
        let mut storage = MemoryStorage::default();
        save(&mut storage, &sample_state(), None);
        let payload = storage.get_string(KEY).unwrap();
        assert!(payload.contains("inf"), "infinities should be present to test");
        assert!(
            matches!(load(&storage), Loaded::Restored(_)),
            "a payload containing infinities must round-trip"
        );
    }

    #[test]
    fn a_corrupt_payload_starts_fresh_and_is_handed_back() {
        let mut storage = MemoryStorage::default();
        storage.set_string(KEY, "{not json at all".to_owned());
        match load(&storage) {
            Loaded::Unreadable { reason, payload } => {
                assert!(reason.contains("could not be read"), "got {reason}");
                assert_eq!(payload, "{not json at all");
            }
            _ => panic!("expected an unreadable payload"),
        }
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_misread() {
        let mut storage = MemoryStorage::default();
        let mut state = sample_state();
        state.version = SCHEMA_VERSION + 5;
        save(&mut storage, &state, None);

        match load(&storage) {
            Loaded::Unreadable { reason, .. } => {
                assert!(reason.contains("newer version"), "got {reason}");
            }
            _ => panic!("a newer format must not be loaded"),
        }
    }

    #[test]
    fn an_unreadable_payload_is_set_aside_on_the_next_save() {
        let mut storage = MemoryStorage::default();
        storage.set_string(KEY, "garbage".to_owned());
        let Loaded::Unreadable { payload, .. } = load(&storage) else {
            panic!("expected unreadable");
        };

        save(&mut storage, &sample_state(), Some(&payload));
        assert_eq!(storage.get_string(BACKUP_KEY).as_deref(), Some("garbage"));
        // And the live key is now valid again.
        assert!(matches!(load(&storage), Loaded::Restored(_)));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Forward compatibility: a file written by an older build lacks fields added later, so
        // every optional field carries `#[serde(default)]`. Simulated by deleting them from a
        // real payload, which also proves the annotations are actually present.
        let mut storage = MemoryStorage::default();
        save(&mut storage, &sample_state(), None);
        let full = storage.get_string(KEY).unwrap();

        let stripped: String = full
            .lines()
            .filter(|line| {
                let field = line.trim_start();
                !(field.starts_with("max_bytes:")
                    || field.starts_with("font_size:")
                    || field.starts_with("enter_crlf:")
                    || field.starts_with("auto_connect:")
                    || field.starts_with("auto_reconnect:"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            stripped.len() < full.len(),
            "the test should actually have removed something"
        );
        storage.set_string(KEY, stripped);

        let state = match load(&storage) {
            Loaded::Restored(state) => state,
            other => panic!(
                "a file missing optional fields should still load: {}",
                match other {
                    Loaded::Unreadable { reason, .. } => reason,
                    _ => "fresh".to_owned(),
                }
            ),
        };
        let tab = &state.tabs[0];
        assert_eq!(tab.max_bytes, crate::term::DEFAULT_MAX_BYTES);
        assert_eq!(tab.font_size, 13.0);
        assert!(tab.enter_crlf, "enter_crlf should default to on");
        assert!(!tab.auto_connect, "auto-connect must default to off");
    }

    // ---- auto-connect policy ----

    #[test]
    fn serial_auto_connect_needs_the_device_to_be_present() {
        let mut settings = ConnectionSettings {
            kind: ConnectionKind::Serial,
            serial: SerialSettings {
                name: "COM3".into(),
                ..Default::default()
            },
            ssh: SshSettings::default(),
        };
        assert_eq!(may_auto_connect(&settings, &[]), AutoConnect::No("COM3 is not attached.".into()));
        assert_eq!(
            may_auto_connect(&settings, &[port("COM3", "")]),
            AutoConnect::Yes
        );
        settings.serial.name = "COM9".into();
        assert!(matches!(
            may_auto_connect(&settings, &[port("COM3", "")]),
            AutoConnect::No(_)
        ));
    }

    #[test]
    fn serial_auto_connect_refuses_a_different_device_on_the_same_port() {
        // The COM-renumbering hazard: the name matches but the hardware does not.
        let settings = ConnectionSettings {
            kind: ConnectionKind::Serial,
            serial: SerialSettings {
                name: "COM3".into(),
                usb_serial: Some("SN123".into()),
                ..Default::default()
            },
            ssh: SshSettings::default(),
        };
        match may_auto_connect(&settings, &[port("COM3", "SN999")]) {
            AutoConnect::No(reason) => {
                assert!(reason.contains("SN123"), "got {reason}");
                assert!(reason.contains("SN999"), "got {reason}");
            }
            AutoConnect::Yes => panic!("must not open a different device automatically"),
        }
        // The right device on that port is fine.
        assert_eq!(
            may_auto_connect(&settings, &[port("COM3", "SN123")]),
            AutoConnect::Yes
        );
    }

    #[test]
    fn serial_auto_connect_follows_a_device_that_moved_port() {
        let settings = ConnectionSettings {
            kind: ConnectionKind::Serial,
            serial: SerialSettings {
                name: "COM3".into(),
                usb_serial: Some("SN123".into()),
                ..Default::default()
            },
            ssh: SshSettings::default(),
        };
        // Same adapter, new port number.
        assert_eq!(
            may_auto_connect(&settings, &[port("COM14", "SN123")]),
            AutoConnect::Yes
        );
        // A different adapter is not a substitute.
        assert!(matches!(
            may_auto_connect(&settings, &[port("COM14", "SNOTHER")]),
            AutoConnect::No(_)
        ));
    }

    #[test]
    fn ssh_password_tabs_never_auto_connect() {
        // Nothing was saved to authenticate with, so trying would only produce a failure.
        let settings = ConnectionSettings {
            kind: ConnectionKind::Ssh,
            serial: SerialSettings::default(),
            ssh: SshSettings {
                host: "srv".into(),
                user: "phil".into(),
                auth: SshAuth::Password,
                ..Default::default()
            },
        };
        match may_auto_connect(&settings, &[]) {
            AutoConnect::No(reason) => assert!(reason.contains("Passwords are not saved")),
            AutoConnect::Yes => panic!("cannot dial without a password"),
        }
    }

    #[test]
    fn ssh_key_tabs_may_auto_connect() {
        let settings = ConnectionSettings {
            kind: ConnectionKind::Ssh,
            serial: SerialSettings::default(),
            ssh: SshSettings {
                host: "srv".into(),
                user: "phil".into(),
                auth: SshAuth::PublicKey,
                ..Default::default()
            },
        };
        assert_eq!(may_auto_connect(&settings, &[]), AutoConnect::Yes);
    }

    #[test]
    fn incomplete_settings_never_auto_connect() {
        assert!(matches!(
            may_auto_connect(&ConnectionSettings::default(), &[]),
            AutoConnect::No(_)
        ));
    }
}
