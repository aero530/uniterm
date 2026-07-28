//! SSH transport.
//!
//! Opens a connection, verifies the host key against the trust store, authenticates, and
//! requests an interactive shell on a PTY.
//!
//! # How the host key decision is made
//!
//! `check_server_key` runs inside russh's connect future, on a background task, and cannot
//! wait for someone to click a button. So the decision is made in two phases instead:
//!
//! 1. Connect. An unrecognised key is *refused* and reported back as
//!    [`crate::knownhosts::Rejection::Unknown`], carrying the fingerprint.
//! 2. The UI shows that fingerprint. If the user accepts, the connection is retried with
//!    `approved_fingerprint` set; the key is then recorded and accepted.
//!
//! Approval is bound to the exact fingerprint the user was shown, so saying yes cannot
//! blanket-trust a different key that arrives on the retry.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, Msg};
use russh::keys::ssh_key::PublicKey;
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::{Channel, ChannelReadHalf, ChannelWriteHalf, Disconnect};
use tracing::debug;

use crate::knownhosts::{self, Rejection, Trust};
use crate::settings::{SshAuth, SshSettings};

/// How often to send a keepalive, and how many may go unanswered before the connection is
/// declared dead.
///
/// Serial has `port_present` to notice an unplugged adapter; SSH has nothing equivalent, and
/// a silently dropped TCP connection is indistinguishable from an idle one without this.
/// Plan task 3's reconnect button has nothing to react to unless this is on.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_MAX: usize = 3;

/// Secrets for one connection attempt.
///
/// Held in memory for the lifetime of the process and never written to disk. Persisting these
/// is what plan tasks 4 and 5 must not do; an OS keychain is the only acceptable store.
#[derive(Clone, Default)]
pub struct Credentials {
    pub password: String,
    /// Passphrase for an encrypted private key.
    pub passphrase: String,
}

impl Credentials {
    /// Whether enough has been supplied for the selected auth method.
    ///
    /// A blank key passphrase is fine — unencrypted keys are normal — but a blank password is
    /// almost always a forgotten field rather than a passwordless account.
    pub fn satisfies(&self, auth: SshAuth) -> Result<(), &'static str> {
        match auth {
            SshAuth::Password if self.password.is_empty() => Err("Enter the password first."),
            _ => Ok(()),
        }
    }
}

/// Why a connection attempt failed.
#[derive(Debug)]
pub enum Error {
    /// The host key was not accepted. May be promptable.
    HostKey(Rejection),
    /// Authentication was refused.
    Auth(String),
    /// Anything else: DNS, connect, key file, channel setup.
    Other(String),
}

impl Error {
    pub fn message(&self) -> String {
        match self {
            Self::HostKey(rejection) => rejection.message(),
            Self::Auth(m) | Self::Other(m) => m.clone(),
        }
    }
}

/// Records the host key verdict so the caller can explain a refusal.
#[derive(Default)]
struct Verdict {
    rejection: Option<Rejection>,
    /// Set when an approved-but-unrecorded key should be written to the store on success.
    learn: Option<PublicKey>,
}

/// Applies the trust policy during the handshake.
struct Verifier {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    approved_fingerprint: Option<String>,
    verdict: Arc<Mutex<Verdict>>,
}

impl client::Handler for Verifier {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let algorithm = key.algorithm().to_string();
        let fingerprint = knownhosts::fingerprint(key);

        match knownhosts::check(&self.host, self.port, key, &self.known_hosts) {
            Trust::Known => {
                debug!("host key for {}:{} is known", self.host, self.port);
                Ok(true)
            }
            // Never accepted, and never promptable.
            Trust::Changed { line } => {
                if let Ok(mut verdict) = self.verdict.lock() {
                    verdict.rejection = Some(Rejection::Changed {
                        host: self.host.clone(),
                        port: self.port,
                        line,
                        algorithm,
                        fingerprint,
                    });
                }
                Ok(false)
            }
            Trust::Unknown => {
                // Accept only if the user approved this exact key.
                if self.approved_fingerprint.as_deref() == Some(fingerprint.as_str()) {
                    if let Ok(mut verdict) = self.verdict.lock() {
                        verdict.learn = Some(key.clone());
                    }
                    Ok(true)
                } else {
                    if let Ok(mut verdict) = self.verdict.lock() {
                        verdict.rejection = Some(Rejection::Unknown {
                            host: self.host.clone(),
                            port: self.port,
                            algorithm,
                            fingerprint,
                        });
                    }
                    Ok(false)
                }
            }
        }
    }
}

/// A live SSH shell.
///
/// `Debug` is manual: none of the russh handles implement it, but tests format
/// `Result<SshTransport, Error>` when an expectation fails.
pub struct SshTransport {
    read: ChannelReadHalf,
    write: ChannelWriteHalf<Msg>,
    /// Kept alive: dropping the handle tears down the connection.
    handle: Handle<Verifier>,
}

impl std::fmt::Debug for SshTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SshTransport")
    }
}

impl SshTransport {
    pub fn read_half(&mut self) -> &mut ChannelReadHalf {
        &mut self.read
    }

    pub async fn send(&mut self, data: &[u8]) -> Result<(), String> {
        self.write
            .data_bytes(data.to_vec())
            .await
            .map_err(|e| format!("SSH write failed: {e}"))
    }

    /// Tell the remote end the terminal changed size, so full-screen programs reflow.
    pub async fn resize(&mut self, columns: u16, rows: u16) -> Result<(), String> {
        self.write
            .window_change(columns as u32, rows as u32, 0, 0)
            .await
            .map_err(|e| format!("SSH window change failed: {e}"))
    }

    /// Best-effort clean shutdown.
    pub async fn close(self) {
        let _ = self.write.eof().await;
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "closed by user", "")
            .await;
    }
}

/// Connect, authenticate and start a shell.
pub async fn connect(
    settings: SshSettings,
    credentials: Credentials,
    approved_fingerprint: Option<String>,
    known_hosts: PathBuf,
    columns: u16,
    rows: u16,
) -> Result<SshTransport, Error> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(KEEPALIVE_INTERVAL),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    });

    let verdict = Arc::new(Mutex::new(Verdict::default()));
    let verifier = Verifier {
        host: settings.host.clone(),
        port: settings.port,
        known_hosts: known_hosts.clone(),
        approved_fingerprint,
        verdict: Arc::clone(&verdict),
    };

    let mut handle = match client::connect(
        config,
        (settings.host.as_str(), settings.port),
        verifier,
    )
    .await
    {
        Ok(handle) => handle,
        Err(e) => {
            // A refused host key surfaces here as a generic handshake failure, so the
            // recorded verdict is what actually explains it.
            if let Some(rejection) = verdict.lock().ok().and_then(|v| v.rejection.clone()) {
                return Err(Error::HostKey(rejection));
            }
            return Err(Error::Other(format!(
                "Could not connect to {}:{}: {e}",
                settings.host, settings.port
            )));
        }
    };

    // The handshake succeeded, so an approved key can now be recorded.
    let to_learn = verdict.lock().ok().and_then(|v| v.learn.clone());
    if let Some(key) = to_learn {
        if let Err(e) = knownhosts::learn(&settings.host, settings.port, &key, &known_hosts) {
            // Not fatal: the connection is up, the user will just be asked again next time.
            tracing::warn!("could not record host key: {e}");
        }
    }

    authenticate(&mut handle, &settings, &credentials).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| Error::Other(format!("Could not open a session channel: {e}")))?;

    start_shell(&channel, &settings, columns, rows).await?;

    let (read, write) = channel.split();
    Ok(SshTransport {
        read,
        write,
        handle,
    })
}

async fn authenticate(
    handle: &mut Handle<Verifier>,
    settings: &SshSettings,
    credentials: &Credentials,
) -> Result<(), Error> {
    let result = match settings.auth {
        SshAuth::Password => handle
            .authenticate_password(settings.user.clone(), credentials.password.clone())
            .await
            .map_err(|e| Error::Auth(format!("Password authentication failed: {e}")))?,

        SshAuth::PublicKey => {
            let path = settings
                .key_path
                .as_ref()
                .ok_or_else(|| Error::Auth("No private key file selected.".to_owned()))?;

            let passphrase = if credentials.passphrase.is_empty() {
                None
            } else {
                Some(credentials.passphrase.as_str())
            };
            let key = load_secret_key(path, passphrase).map_err(|e| {
                Error::Auth(format!(
                    "Could not load {}: {e}. An encrypted key needs its passphrase.",
                    path.display()
                ))
            })?;

            // RSA keys must be signed with a hash the server actually accepts.
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| Error::Auth(format!("Could not negotiate a signature hash: {e}")))?
                .flatten();

            handle
                .authenticate_publickey(
                    settings.user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(|e| Error::Auth(format!("Key authentication failed: {e}")))?
        }
    };

    if result.success() {
        Ok(())
    } else {
        Err(Error::Auth(format!(
            "{} authentication was rejected by {}.",
            settings.auth.label(),
            settings.host
        )))
    }
}

/// Request a PTY and start a shell on it.
///
/// `want_reply` is set, but russh does not block for the reply: a refusal arrives later as a
/// channel failure message rather than an error here. A server that refuses a PTY therefore
/// yields a line-mode shell instead of an error — rare enough to accept, and visible to the
/// user as a shell that behaves oddly rather than as silent data loss.
async fn start_shell(
    channel: &Channel<Msg>,
    settings: &SshSettings,
    columns: u16,
    rows: u16,
) -> Result<(), Error> {
    channel
        .request_pty(
            true,
            &settings.term,
            columns as u32,
            rows as u32,
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| Error::Other(format!("Could not allocate a PTY: {e}")))?;

    channel
        .request_shell(true)
        .await
        .map_err(|e| Error::Other(format!("Could not start a shell: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_auth_needs_a_password() {
        assert!(Credentials::default().satisfies(SshAuth::Password).is_err());
        let creds = Credentials {
            password: "hunter2".into(),
            ..Default::default()
        };
        assert!(creds.satisfies(SshAuth::Password).is_ok());
    }

    #[test]
    fn key_auth_accepts_a_blank_passphrase() {
        // Unencrypted keys are normal, so a blank passphrase must not block the attempt.
        assert!(Credentials::default().satisfies(SshAuth::PublicKey).is_ok());
    }

    #[test]
    fn host_key_errors_report_the_rejection_message() {
        let error = Error::HostKey(Rejection::Unknown {
            host: "srv".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
        });
        let message = error.message();
        assert!(message.contains("srv:22"));
        assert!(message.contains("SHA256:abc"));
    }

    #[test]
    fn auth_and_other_errors_pass_their_message_through() {
        assert_eq!(Error::Auth("nope".into()).message(), "nope");
        assert_eq!(Error::Other("boom".into()).message(), "boom");
    }
}
