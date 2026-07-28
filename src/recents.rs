//! Recently used connections.
//!
//! A capped, most-recent-first list of connections that have actually worked, so reopening one
//! takes a click instead of retyping a host or hunting for a baud rate.
//!
//! Two decisions worth knowing about:
//!
//! * **Only successful connections are recorded.** A list full of things that never connected is
//!   noise, and would helpfully offer to repeat your typos.
//! * **Entries hold [`ConnectionSettings`], which has no field for a password or passphrase.**
//!   A one-click reopen is exactly where it would be tempting to store one; the type makes it
//!   impossible instead of merely discouraged.

use serde::{Deserialize, Serialize};

use crate::settings::ConnectionSettings;

/// How many unpinned entries to keep.
pub const MAX_ENTRIES: usize = 20;
/// Hard ceiling including pinned entries, so the list cannot grow without bound.
const HARD_CAP: usize = 100;

/// One remembered connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub settings: ConnectionSettings,
    /// Seconds since the Unix epoch, for "3 hours ago".
    #[serde(default)]
    pub last_used: u64,
    #[serde(default)]
    pub uses: u32,
    /// Pinned entries sort first and are never evicted.
    #[serde(default)]
    pub pinned: bool,
}

impl Recent {
    pub fn identity(&self) -> String {
        self.settings.identity()
    }
}

/// The list, most recent first with pinned entries ahead of the rest.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Recents {
    entries: Vec<Recent>,
}

impl Recents {
    pub fn from_entries(entries: Vec<Recent>) -> Self {
        let mut recents = Self { entries };
        recents.sort();
        recents.evict();
        recents
    }

    pub fn entries(&self) -> &[Recent] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Note that a connection succeeded.
    ///
    /// An existing entry for the same identity is updated rather than duplicated, keeping its
    /// pinned flag and use count.
    pub fn record(&mut self, settings: &ConnectionSettings, now: u64) {
        let identity = settings.identity();
        match self.entries.iter_mut().find(|e| e.identity() == identity) {
            Some(existing) => {
                existing.last_used = now;
                existing.uses = existing.uses.saturating_add(1);
                // Settings may have been edited since (a changed key path, say); keep the
                // newest, since that is what worked.
                existing.settings = settings.clone();
            }
            None => self.entries.push(Recent {
                settings: settings.clone(),
                last_used: now,
                uses: 1,
                pinned: false,
            }),
        }
        self.sort();
        self.evict();
    }

    pub fn remove(&mut self, identity: &str) {
        self.entries.retain(|e| e.identity() != identity);
    }

    /// Forget everything unpinned. Pinned entries are the ones the user said to keep.
    pub fn clear_unpinned(&mut self) {
        self.entries.retain(|e| e.pinned);
    }

    pub fn toggle_pin(&mut self, identity: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.identity() == identity) {
            entry.pinned = !entry.pinned;
        }
        self.sort();
    }

    /// Pinned first, then most recently used.
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.last_used.cmp(&a.last_used))
        });
    }

    /// Drop the oldest unpinned entries once over the cap.
    ///
    /// Relies on [`Self::sort`] having run: entries are newest-first, so keeping the first
    /// `MAX_ENTRIES` unpinned ones drops exactly the stalest.
    fn evict(&mut self) {
        let mut kept = 0;
        self.entries.retain(|entry| {
            if entry.pinned {
                return true;
            }
            kept += 1;
            kept <= MAX_ENTRIES
        });
        // Pinning is user-driven, but bound it anyway.
        self.entries.truncate(HARD_CAP);
    }
}

/// Seconds since the Unix epoch.
pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "just now", "5 min ago", "3 hours ago", "2 days ago".
///
/// Relative rather than absolute deliberately: `std` cannot convert to local time, and "14:02
/// UTC" is worse than useless for judging how recent something is.
pub fn relative_time(now: u64, then: u64) -> String {
    if then == 0 {
        return "unknown".to_owned();
    }
    let seconds = now.saturating_sub(then);
    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3_599 => {
            let minutes = seconds / 60;
            format!("{minutes} min ago")
        }
        3_600..=86_399 => {
            let hours = seconds / 3_600;
            plural(hours, "hour")
        }
        _ => {
            let days = seconds / 86_400;
            plural(days, "day")
        }
    }
}

fn plural(count: u64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ConnectionKind, SerialSettings, SshAuth, SshSettings};

    fn serial(name: &str, baud: u32) -> ConnectionSettings {
        ConnectionSettings {
            kind: ConnectionKind::Serial,
            serial: SerialSettings {
                name: name.into(),
                baud_rate: baud,
                ..Default::default()
            },
            ssh: SshSettings::default(),
        }
    }

    fn ssh(user: &str, host: &str, port: u16) -> ConnectionSettings {
        ConnectionSettings {
            kind: ConnectionKind::Ssh,
            serial: SerialSettings::default(),
            ssh: SshSettings {
                host: host.into(),
                user: user.into(),
                port,
                ..Default::default()
            },
        }
    }

    #[test]
    fn recording_the_same_connection_twice_updates_rather_than_duplicates() {
        let mut recents = Recents::default();
        recents.record(&serial("COM3", 115_200), 1_000);
        recents.record(&serial("COM3", 115_200), 2_000);
        assert_eq!(recents.len(), 1);
        assert_eq!(recents.entries()[0].uses, 2);
        assert_eq!(recents.entries()[0].last_used, 2_000);
    }

    #[test]
    fn serial_identity_includes_the_line_parameters() {
        // The plan's call: for a serial tool the baud rate is part of what you are remembering.
        let mut recents = Recents::default();
        recents.record(&serial("COM3", 9_600), 1_000);
        recents.record(&serial("COM3", 115_200), 2_000);
        assert_eq!(recents.len(), 2, "different baud rates are different entries");
    }

    #[test]
    fn ssh_identity_ignores_the_auth_method() {
        // How you get in is not what you are connecting to.
        let mut with_password = ssh("phil", "srv", 22);
        with_password.ssh.auth = SshAuth::Password;
        let mut with_key = ssh("phil", "srv", 22);
        with_key.ssh.auth = SshAuth::PublicKey;

        let mut recents = Recents::default();
        recents.record(&with_password, 1_000);
        recents.record(&with_key, 2_000);
        assert_eq!(recents.len(), 1, "same destination, one entry");
        // The most recent settings win, so reopening uses the method that last worked.
        assert_eq!(recents.entries()[0].settings.ssh.auth, SshAuth::PublicKey);
    }

    #[test]
    fn ssh_identity_distinguishes_user_host_and_port() {
        let mut recents = Recents::default();
        recents.record(&ssh("phil", "srv", 22), 1);
        recents.record(&ssh("root", "srv", 22), 2);
        recents.record(&ssh("phil", "other", 22), 3);
        recents.record(&ssh("phil", "srv", 2222), 4);
        assert_eq!(recents.len(), 4);
    }

    #[test]
    fn serial_and_ssh_never_collide() {
        let mut recents = Recents::default();
        recents.record(&serial("COM3", 9_600), 1);
        recents.record(&ssh("phil", "COM3", 9_600), 2);
        assert_eq!(recents.len(), 2);
    }

    #[test]
    fn entries_are_most_recent_first() {
        let mut recents = Recents::default();
        recents.record(&serial("COM1", 9_600), 100);
        recents.record(&serial("COM2", 9_600), 300);
        recents.record(&serial("COM3", 9_600), 200);
        let names: Vec<_> = recents
            .entries()
            .iter()
            .map(|e| e.settings.serial.name.clone())
            .collect();
        assert_eq!(names, vec!["COM2", "COM3", "COM1"]);
    }

    #[test]
    fn the_list_is_capped() {
        let mut recents = Recents::default();
        for i in 0..(MAX_ENTRIES as u32 + 10) {
            recents.record(&serial(&format!("COM{i}"), 9_600), i as u64 + 1);
        }
        assert_eq!(recents.len(), MAX_ENTRIES);
        // The newest survived and the oldest did not.
        let names: Vec<_> = recents
            .entries()
            .iter()
            .map(|e| e.settings.serial.name.clone())
            .collect();
        assert!(names.contains(&format!("COM{}", MAX_ENTRIES as u32 + 9)));
        assert!(!names.contains(&"COM0".to_owned()));
    }

    #[test]
    fn pinned_entries_are_never_evicted() {
        let mut recents = Recents::default();
        let keeper = serial("COM-KEEP", 9_600);
        recents.record(&keeper, 1);
        recents.toggle_pin(&keeper.identity());
        assert!(recents.entries()[0].pinned);

        // Flood the list well past the cap.
        for i in 0..(MAX_ENTRIES as u32 + 30) {
            recents.record(&serial(&format!("COM{i}"), 9_600), i as u64 + 100);
        }
        let names: Vec<_> = recents
            .entries()
            .iter()
            .map(|e| e.settings.serial.name.clone())
            .collect();
        assert!(
            names.contains(&"COM-KEEP".to_owned()),
            "a pinned entry must survive eviction"
        );
        assert_eq!(
            recents.entries().iter().filter(|e| !e.pinned).count(),
            MAX_ENTRIES,
            "unpinned entries are still capped"
        );
    }

    #[test]
    fn pinned_entries_sort_first_even_when_older() {
        let mut recents = Recents::default();
        let old = serial("COM-OLD", 9_600);
        recents.record(&old, 1);
        recents.record(&serial("COM-NEW", 9_600), 9_999);
        recents.toggle_pin(&old.identity());
        assert_eq!(recents.entries()[0].settings.serial.name, "COM-OLD");
    }

    #[test]
    fn unpinning_restores_recency_order() {
        let mut recents = Recents::default();
        let old = serial("COM-OLD", 9_600);
        recents.record(&old, 1);
        recents.record(&serial("COM-NEW", 9_600), 9_999);
        recents.toggle_pin(&old.identity());
        recents.toggle_pin(&old.identity());
        assert_eq!(recents.entries()[0].settings.serial.name, "COM-NEW");
    }

    #[test]
    fn removing_and_clearing_work() {
        let mut recents = Recents::default();
        let a = serial("COM1", 9_600);
        let b = serial("COM2", 9_600);
        recents.record(&a, 1);
        recents.record(&b, 2);
        recents.remove(&a.identity());
        assert_eq!(recents.len(), 1);
        assert_eq!(recents.entries()[0].settings.serial.name, "COM2");

        recents.clear_unpinned();
        assert!(recents.is_empty());
    }

    #[test]
    fn clearing_keeps_pinned_entries() {
        let mut recents = Recents::default();
        let keeper = serial("COM-KEEP", 9_600);
        recents.record(&keeper, 1);
        recents.record(&serial("COM-GO", 9_600), 2);
        recents.toggle_pin(&keeper.identity());
        recents.clear_unpinned();
        assert_eq!(recents.len(), 1);
        assert_eq!(recents.entries()[0].settings.serial.name, "COM-KEEP");
    }

    #[test]
    fn no_credentials_can_be_stored_in_an_entry() {
        // Structural: the settings type has nowhere to put one. Asserted so a refactor that
        // adds one has to confront this test.
        let mut recents = Recents::default();
        recents.record(&ssh("phil", "srv", 22), 1);
        let encoded = serde_json::to_string(&recents).expect("serialise");
        for field in ["password", "passphrase", "credential", "secret"] {
            assert!(
                !encoded.to_lowercase().contains(&format!("{field}\":")),
                "recents must not carry a {field} field: {encoded}"
            );
        }
    }

    #[test]
    fn from_entries_sorts_and_caps_untrusted_input() {
        // Loaded from disk, so it may be out of order or over the cap.
        let entries: Vec<Recent> = (0..(MAX_ENTRIES + 5))
            .map(|i| Recent {
                settings: serial(&format!("COM{i}"), 9_600),
                last_used: i as u64,
                uses: 1,
                pinned: false,
            })
            .collect();
        let recents = Recents::from_entries(entries);
        assert_eq!(recents.len(), MAX_ENTRIES);
        // Highest timestamp first.
        assert_eq!(
            recents.entries()[0].settings.serial.name,
            format!("COM{}", MAX_ENTRIES + 4)
        );
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 30), "just now");
        assert_eq!(relative_time(now, now - 60), "1 min ago");
        assert_eq!(relative_time(now, now - 600), "10 min ago");
        assert_eq!(relative_time(now, now - 3_600), "1 hour ago");
        assert_eq!(relative_time(now, now - 7_200), "2 hours ago");
        assert_eq!(relative_time(now, now - 86_400), "1 day ago");
        assert_eq!(relative_time(now, now - 172_800), "2 days ago");
    }

    #[test]
    fn relative_time_handles_missing_and_future_timestamps() {
        assert_eq!(relative_time(1_000, 0), "unknown");
        // A clock that went backwards must not underflow.
        assert_eq!(relative_time(100, 500), "just now");
    }

    #[test]
    fn descriptions_are_useful_for_both_kinds() {
        assert_eq!(serial("COM3", 115_200).description(), "COM3 · 115200 baud");
        let mut s = ssh("phil", "srv", 2222);
        s.ssh.auth = SshAuth::PublicKey;
        assert_eq!(s.description(), "phil@srv:2222 · Private key");
    }
}
