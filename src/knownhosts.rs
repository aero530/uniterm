//! Host key trust store.
//!
//! Verifying the server's host key is what stops an SSH connection being silently
//! intercepted. `russh` deliberately rejects every key by default and leaves the policy to
//! the application, so this module supplies it.
//!
//! The store is the standard `~/.ssh/known_hosts`, so entries interoperate with OpenSSH:
//! a host trusted here is trusted by `ssh`, and vice versa.
//!
//! Three outcomes, deliberately handled differently:
//!
//! * [`Trust::Known`] — the key matches what was recorded. Connect.
//! * [`Trust::Unknown`] — nothing recorded for this host and key type. Ask the user, showing
//!   the fingerprint (trust on first use). Only on an explicit yes is the key recorded.
//! * [`Trust::Changed`] — a key of the *same type* is recorded and it is different. This is
//!   what a man-in-the-middle looks like, so it is a hard failure that no prompt can wave
//!   through; the user has to resolve it in `known_hosts` by hand.
//!
//! A host legitimately may serve several key types, so a new *type* for a known host reports
//! `Unknown` rather than `Changed`. That matches OpenSSH.

use std::path::{Path, PathBuf};

use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::ssh_key::PublicKey;
use russh::keys::HashAlg;
use tracing::warn;

/// Result of checking a server key against the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Recorded and matching.
    Known,
    /// Not recorded. Safe to prompt.
    Unknown,
    /// Recorded differently for the same key type. Refuse.
    Changed { line: usize },
}

/// The user's `known_hosts` file.
pub fn default_path() -> Option<PathBuf> {
    dirs_home().map(|home| home.join(".ssh").join("known_hosts"))
}

/// Home directory, without pulling in a crate for it.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Strip the comment from a key.
///
/// `ssh_key::PublicKey`'s equality includes the trailing comment, and russh's comparison uses
/// it, so a key carrying `user@host` would be reported as *changed* against the same key
/// recorded without one. A comment is metadata, not identity, and mistaking it for tampering
/// would train users to click through the one warning that must never be clicked through.
fn without_comment(key: &PublicKey) -> PublicKey {
    PublicKey::new(key.key_data().clone(), "")
}

/// Check a server key.
///
/// A store that does not exist yet counts as [`Trust::Unknown`] — the first connection to
/// anything is a first use.
pub fn check(host: &str, port: u16, key: &PublicKey, path: &Path) -> Trust {
    if !path.exists() {
        return Trust::Unknown;
    }
    let key = &without_comment(key);
    match check_known_hosts_path(host, port, key, path) {
        Ok(true) => Trust::Known,
        Ok(false) => Trust::Unknown,
        Err(russh::keys::Error::KeyChanged { line }) => Trust::Changed { line },
        Err(e) => {
            // An unreadable or malformed store must not be treated as "trusted". Falling
            // back to Unknown means the user is asked rather than silently accepted.
            warn!("could not read {}: {e}", path.display());
            Trust::Unknown
        }
    }
}

/// Record a key, creating the store if needed.
pub fn learn(host: &str, port: u16, key: &PublicKey, path: &Path) -> Result<(), String> {
    learn_known_hosts_path(host, port, &without_comment(key), path)
        .map_err(|e| format!("Could not write {}: {e}", path.display()))
}

/// `SHA256:...`, the form `ssh` prints and users compare against.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// What to tell the user when a key was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// First time seeing this host. Offer to trust it.
    Unknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
    },
    /// The recorded key changed. Refuse and explain.
    Changed {
        host: String,
        port: u16,
        line: usize,
        algorithm: String,
        fingerprint: String,
    },
}

impl Rejection {
    /// Message shown in the tab.
    pub fn message(&self) -> String {
        match self {
            Self::Unknown {
                host,
                port,
                algorithm,
                fingerprint,
            } => format!(
                "Unknown host {host}:{port}. Key type {algorithm}, fingerprint {fingerprint}. \
                 Verify this out of band before trusting it."
            ),
            Self::Changed {
                host,
                port,
                line,
                algorithm,
                fingerprint,
            } => format!(
                "HOST KEY CHANGED for {host}:{port}. The {algorithm} key recorded at line {line} \
                 of known_hosts does not match the key the server offered ({fingerprint}). This \
                 can mean the connection is being intercepted. If you know the host was \
                 legitimately rekeyed, remove line {line} from known_hosts by hand."
            ),
        }
    }

    /// Whether the user may be offered a way through this.
    ///
    /// Only an unknown host is promptable. A changed key is never click-throughable — that is
    /// the entire point of recording it.
    pub fn is_promptable(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed key material, so tests need no RNG and are deterministic.
    const ED25519_A: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIAgJufBWRCob7ytiVrAnEW7PvL432B7cXzJLziOZ59id";
    const ED25519_B: &str =
        "AAAAC3NzaC1lZDI1NTE5AAAAIGIOya00NHtlVjWcc2n43OG86cbco7o/N0vC+N+QFrLV";
    const RSA: &str = "AAAAB3NzaC1yc2EAAAADAQABAAABAQDjYrrF6O38JPZyYF4bfC1AbfmkI0s0y7qDtMtqQQD3mfjpD5R7P/VZDjEb6GjbfNs0DKfcaGUfaiOVblgTE4wGZwzqE0Rk0n51lwhGj2rWCg+eEUvjzsWVwoTlNhHAsXCB16PiApwHktl++ZriTytIdJLoRJ/8AQthXfirHiYrykAoxgqryQdwez6RVpR1O9dPr0VQjogrTljfWOcWOOfAhvJyx0Ph5cBwecaQoRRglPxBkID74zMhdyfHGDRnBZWfI4Q6EGw/Da4ZeGz87tf4GK2GP/K6A337mpTo6R1+ZLOrwZSBY491hAzc8hwqhxXjioMQyxeBRe7H/8V1nlrn";

    fn key(base64: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(base64).expect("valid test key")
    }

    /// A temporary store path unique to each test.
    fn store(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("uniterm_kh_{name}"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_missing_store_is_unknown_not_an_error() {
        let path = store("missing");
        assert_eq!(check("example.com", 22, &key(ED25519_A), &path), Trust::Unknown);
    }

    #[test]
    fn learning_makes_a_host_known() {
        let path = store("learn");
        let k = key(ED25519_A);
        assert_eq!(check("example.com", 22, &k, &path), Trust::Unknown);
        learn("example.com", 22, &k, &path).unwrap();
        assert_eq!(check("example.com", 22, &k, &path), Trust::Known);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_changed_key_is_reported_as_changed() {
        // The security-critical case: same host, same key type, different key.
        let path = store("changed");
        learn("example.com", 22, &key(ED25519_A), &path).unwrap();
        match check("example.com", 22, &key(ED25519_B), &path) {
            Trust::Changed { .. } => {}
            other => panic!("expected Changed, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_new_key_type_for_a_known_host_is_unknown() {
        // Hosts legitimately serve several key types, so this must not read as tampering.
        let path = store("algo");
        learn("example.com", 22, &key(ED25519_A), &path).unwrap();
        assert_eq!(check("example.com", 22, &key(RSA), &path), Trust::Unknown);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn port_is_part_of_the_identity() {
        let path = store("port");
        let k = key(ED25519_A);
        learn("example.com", 22, &k, &path).unwrap();
        assert_eq!(check("example.com", 22, &k, &path), Trust::Known);
        assert_eq!(check("example.com", 2222, &k, &path), Trust::Unknown);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn host_is_part_of_the_identity() {
        let path = store("host");
        let k = key(ED25519_A);
        learn("example.com", 22, &k, &path).unwrap();
        assert_eq!(check("elsewhere.com", 22, &k, &path), Trust::Unknown);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_key_comment_is_not_mistaken_for_a_changed_key() {
        // Comments are metadata, not identity. Reporting one as tampering would teach users
        // to click through the warning that must never be clicked through.
        let path = store("comment");
        let mut commented = key(ED25519_A);
        commented.set_comment("someone@somewhere");

        learn("example.com", 22, &key(ED25519_A), &path).unwrap();
        assert_eq!(check("example.com", 22, &commented, &path), Trust::Known);

        // And the other way round: learning a commented key still matches the bare one.
        let path = store("comment2");
        learn("example.com", 22, &commented, &path).unwrap();
        assert_eq!(check("example.com", 22, &key(ED25519_A), &path), Trust::Known);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_malformed_store_does_not_become_trusted() {
        let path = store("malformed");
        std::fs::write(&path, "this is not a known_hosts file\n@@@@\n").unwrap();
        // Must never be Known.
        assert_ne!(check("example.com", 22, &key(ED25519_A), &path), Trust::Known);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fingerprints_are_sha256_and_key_specific() {
        let a = fingerprint(&key(ED25519_A));
        let b = fingerprint(&key(ED25519_B));
        assert!(a.starts_with("SHA256:"), "got {a}");
        assert_ne!(a, b);
        // Stable across calls.
        assert_eq!(a, fingerprint(&key(ED25519_A)));
    }

    #[test]
    fn only_unknown_hosts_are_promptable() {
        let unknown = Rejection::Unknown {
            host: "h".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:x".into(),
        };
        let changed = Rejection::Changed {
            host: "h".into(),
            port: 22,
            line: 3,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:y".into(),
        };
        assert!(unknown.is_promptable());
        assert!(
            !changed.is_promptable(),
            "a changed host key must never be click-throughable"
        );
    }

    #[test]
    fn messages_carry_the_detail_a_user_needs() {
        let changed = Rejection::Changed {
            host: "srv".into(),
            port: 2222,
            line: 7,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:zzz".into(),
        };
        let msg = changed.message();
        assert!(msg.contains("srv:2222"));
        assert!(msg.contains("line 7"));
        assert!(msg.contains("SHA256:zzz"));
        assert!(msg.to_lowercase().contains("intercept"));
    }

    #[test]
    fn learning_creates_missing_parent_directories() {
        let dir = std::env::temp_dir().join("uniterm_kh_nested").join("deeper");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("uniterm_kh_nested"));
        let path = dir.join("known_hosts");
        learn("example.com", 22, &key(ED25519_A), &path).unwrap();
        assert_eq!(check("example.com", 22, &key(ED25519_A), &path), Trust::Known);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("uniterm_kh_nested"));
    }
}
