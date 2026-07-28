//! End-to-end tests for the SSH client against a real in-process SSH server.
//!
//! Without these the SSH path would only be known to *compile*. russh has a server side, so
//! the whole thing is exercised for real over a loopback TCP connection: key exchange, host
//! key verification, password and public-key authentication, the PTY and shell requests, data
//! in both directions, and window resizing.
//!
//! Most importantly it tests the trust policy against a live handshake — that an unknown host
//! is refused with a usable fingerprint, that approving *that* fingerprint connects and
//! records the key, that a subsequent connection needs no approval, and that a substituted
//! key is refused as a change rather than quietly accepted.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use russh::keys::PrivateKey;
use russh::server::{self, Auth, Msg, Server as _, Session as ServerSession};
use russh::{Channel, ChannelId};

use super::ssh::{self, Credentials};
use crate::knownhosts::{self, Rejection, Trust};
use crate::session::ConnectionState;
use crate::settings::{SshAuth, SshSettings};

/// Fixed host key, so tests need no RNG and fingerprints are deterministic.
const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACAAVBqdkXYfu/7q7B8nxDpbqHQkTXyxzzHDNNqrdgLQdAAAAJArnd7BK53e
wQAAAAtzc2gtZWQyNTUxOQAAACAAVBqdkXYfu/7q7B8nxDpbqHQkTXyxzzHDNNqrdgLQdA
AAAEDLtfar2afxw9UR1kzjEVEcvkPJxo/o+Jm6WoM7n1XXuQBUGp2Rdh+7/ursHyfEOluo
dCRNfLHPMcM02qt2AtB0AAAADHVuaXRlcm0tdGVzdAE=
-----END OPENSSH PRIVATE KEY-----
";

/// A different key, used to simulate the host key changing under us.
const OTHER_KEY: &str =
    "AAAAC3NzaC1lZDI1NTE5AAAAIGIOya00NHtlVjWcc2n43OG86cbco7o/N0vC+N+QFrLV";

const USER: &str = "tester";
const PASSWORD: &str = "s3cret";
/// What the fake shell writes as soon as it starts.
const GREETING: &str = "welcome to the test shell\r\n";

/// What the server observed, so assertions can check the client's side of the protocol.
#[derive(Default)]
struct Observed {
    pty_requested: Option<(u32, u32)>,
    shell_requested: bool,
    received: Vec<u8>,
    window_changes: Vec<(u32, u32)>,
    auth_attempts: Vec<String>,
}

#[derive(Clone)]
struct TestServer {
    observed: Arc<Mutex<Observed>>,
}

impl server::Server for TestServer {
    type Handler = Self;

    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .auth_attempts
            .push(format!("password:{user}"));
        if user == USER && password == PASSWORD {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .auth_attempts
            .push(format!("publickey:{user}"));
        // Any key is accepted; the point is exercising the client's signing path.
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        _channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.observed.lock().unwrap().pty_requested = Some((col_width, row_height));
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.observed.lock().unwrap().shell_requested = true;
        // Write a greeting so the client has something to read.
        session.data(channel, bytes::Bytes::from_static(GREETING.as_bytes()))?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .received
            .extend_from_slice(data);
        // "DROP" severs the connection, standing in for a remote host going away.
        if data.windows(4).any(|w| w == b"DROP") {
            session.close(channel)?;
            return Ok(());
        }
        // Echo it back uppercased, so the client can prove the round trip.
        let echo = bytes::Bytes::from(data.to_ascii_uppercase());
        session.data(channel, echo)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut ServerSession,
    ) -> Result<(), Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .window_changes
            .push((col_width, row_height));
        Ok(())
    }
}

/// Start the test server on an ephemeral port. Returns the port and what it observes.
async fn start_server() -> (u16, Arc<Mutex<Observed>>) {
    let key = PrivateKey::from_openssh(HOST_KEY).expect("host key parses");
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });

    let observed = Arc::new(Mutex::new(Observed::default()));
    let mut server = TestServer {
        observed: Arc::clone(&observed),
    };

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    tokio::spawn(async move {
        let _ = server.run_on_address(config, ("127.0.0.1", port)).await;
    });

    // Wait for the listener to accept connections rather than sleeping blindly.
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (port, observed)
}

/// The host key's fingerprint, as the client will report it.
fn host_fingerprint() -> String {
    let key = PrivateKey::from_openssh(HOST_KEY).expect("host key parses");
    knownhosts::fingerprint(key.public_key())
}

fn temp_known_hosts(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("uniterm_ssh_e2e_{name}"));
    let _ = std::fs::remove_file(&path);
    path
}

fn password_settings(port: u16) -> SshSettings {
    SshSettings {
        host: "127.0.0.1".into(),
        port,
        user: USER.into(),
        auth: SshAuth::Password,
        key_path: None,
        term: "xterm-256color".into(),
        known_hosts: None,
    }
}

fn creds() -> Credentials {
    Credentials {
        password: PASSWORD.into(),
        passphrase: String::new(),
    }
}

#[tokio::test]
async fn an_unknown_host_is_refused_with_a_usable_fingerprint() {
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("unknown");

    let result = ssh::connect(
        password_settings(port),
        creds(),
        None,
        store.clone(),
        80,
        24,
    )
    .await;

    match result {
        Err(ssh::Error::HostKey(Rejection::Unknown {
            fingerprint,
            algorithm,
            ..
        })) => {
            assert_eq!(fingerprint, host_fingerprint());
            assert_eq!(algorithm, "ssh-ed25519");
        }
        other => panic!("expected an unknown-host rejection, got {other:?}"),
    }
    // Nothing was recorded by a refused connection.
    assert!(!store.exists() || std::fs::read_to_string(&store).unwrap().trim().is_empty());
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn approving_the_fingerprint_connects_and_records_the_key() {
    let (port, observed) = start_server().await;
    let store = temp_known_hosts("approve");

    let transport = ssh::connect(
        password_settings(port),
        creds(),
        Some(host_fingerprint()),
        store.clone(),
        100,
        40,
    )
    .await
    .expect("approved connection succeeds");

    // The PTY and shell were requested at the size we asked for. These are awaited rather
    // than asserted immediately: `request_pty` does not block for the server's reply, so the
    // server may not have processed it by the time `connect` returns.
    wait_for(&observed, |o| o.pty_requested.is_some() && o.shell_requested).await;
    {
        let observed = observed.lock().unwrap();
        assert_eq!(observed.pty_requested, Some((100, 40)));
        assert_eq!(observed.auth_attempts, vec![format!("password:{USER}")]);
    }

    // The key is now trusted for next time.
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    assert_eq!(
        knownhosts::check("127.0.0.1", port, key.public_key(), &store),
        Trust::Known
    );

    transport.close().await;
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn approving_a_different_fingerprint_does_not_authorise_the_real_one() {
    // Approval is bound to the exact key the user was shown.
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("wrongfp");

    let result = ssh::connect(
        password_settings(port),
        creds(),
        Some("SHA256:not-the-key-you-were-shown".to_owned()),
        store.clone(),
        80,
        24,
    )
    .await;

    assert!(
        matches!(result, Err(ssh::Error::HostKey(Rejection::Unknown { .. }))),
        "a mismatched approval must not let the connection through"
    );
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn a_known_host_connects_without_any_approval() {
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("known");

    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), &store).unwrap();

    let transport = ssh::connect(password_settings(port), creds(), None, store.clone(), 80, 24)
        .await
        .expect("a recorded host needs no prompt");
    transport.close().await;
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn a_substituted_host_key_is_refused_as_changed() {
    // The man-in-the-middle case: something else is recorded for this host and key type.
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("changed");

    let impostor = russh::keys::parse_public_key_base64(OTHER_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, &impostor, &store).unwrap();

    let result = ssh::connect(password_settings(port), creds(), None, store.clone(), 80, 24).await;
    match result {
        Err(ssh::Error::HostKey(rejection @ Rejection::Changed { .. })) => {
            assert!(
                !rejection.is_promptable(),
                "a changed key must never be promptable"
            );
            assert!(rejection.message().to_lowercase().contains("intercept"));
        }
        other => panic!("expected a changed-key rejection, got {other:?}"),
    }

    // Even offering the real fingerprint must not override a recorded mismatch.
    let result = ssh::connect(
        password_settings(port),
        creds(),
        Some(host_fingerprint()),
        store.clone(),
        80,
        24,
    )
    .await;
    assert!(
        matches!(result, Err(ssh::Error::HostKey(Rejection::Changed { .. }))),
        "approval must not be able to wave through a changed host key"
    );
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn a_wrong_password_is_reported_as_an_auth_failure() {
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("badpass");
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), &store).unwrap();

    let result = ssh::connect(
        password_settings(port),
        Credentials {
            password: "wrong".into(),
            passphrase: String::new(),
        },
        None,
        store.clone(),
        80,
        24,
    )
    .await;

    match result {
        Err(ssh::Error::Auth(message)) => {
            assert!(message.to_lowercase().contains("auth"), "got {message}");
        }
        other => panic!("expected an auth error, got {other:?}"),
    }
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn data_flows_both_ways_and_resize_reaches_the_server() {
    let (port, observed) = start_server().await;
    let store = temp_known_hosts("dataflow");
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), &store).unwrap();

    let mut transport =
        ssh::connect(password_settings(port), creds(), None, store.clone(), 80, 24)
            .await
            .expect("connect");

    // Wrap in the Transport enum, which is what the session loop actually drives.
    let mut transport_enum = super::transport::Transport::Ssh(transport);

    // The shell's greeting arrives.
    let greeting = read_until(&mut transport_enum, GREETING.len()).await;
    assert_eq!(String::from_utf8_lossy(&greeting), GREETING);

    // Send data; the server records it and echoes it uppercased.
    transport_enum.send(b"hello").await.expect("send");
    let echo = read_until(&mut transport_enum, 5).await;
    assert_eq!(&echo, b"HELLO", "the round trip must come back");
    assert_eq!(observed.lock().unwrap().received, b"hello");

    // Resizing tells the remote end, so full-screen programs reflow.
    transport_enum.resize(132, 50).await.expect("resize");
    wait_for(&observed, |o| !o.window_changes.is_empty()).await;
    assert_eq!(observed.lock().unwrap().window_changes, vec![(132, 50)]);

    transport = match transport_enum {
        super::transport::Transport::Ssh(ssh) => ssh,
        _ => unreachable!(),
    };
    transport.close().await;
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn public_key_authentication_works() {
    let (port, observed) = start_server().await;
    let store = temp_known_hosts("pubkey");
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), &store).unwrap();

    // Reuse the fixed key as a client key; the server accepts any.
    let key_file = std::env::temp_dir().join("uniterm_ssh_e2e_client_key");
    std::fs::write(&key_file, HOST_KEY).unwrap();

    let settings = SshSettings {
        auth: SshAuth::PublicKey,
        key_path: Some(key_file.clone()),
        ..password_settings(port)
    };

    let transport = ssh::connect(settings, Credentials::default(), None, store.clone(), 80, 24)
        .await
        .expect("public key auth succeeds");

    assert_eq!(
        observed.lock().unwrap().auth_attempts,
        vec![format!("publickey:{USER}")]
    );

    transport.close().await;
    let _ = std::fs::remove_file(&store);
    let _ = std::fs::remove_file(&key_file);
}

#[tokio::test]
async fn a_missing_key_file_is_reported_clearly() {
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("nokey");
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), &store).unwrap();

    let settings = SshSettings {
        auth: SshAuth::PublicKey,
        key_path: Some(PathBuf::from("this-file-does-not-exist")),
        ..password_settings(port)
    };

    match ssh::connect(settings, Credentials::default(), None, store.clone(), 80, 24).await {
        Err(ssh::Error::Auth(message)) => {
            assert!(message.contains("this-file-does-not-exist"), "got {message}");
        }
        other => panic!("expected an auth error naming the file, got {other:?}"),
    }
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn connecting_to_a_closed_port_is_reported_not_hung() {
    let store = temp_known_hosts("refused");
    // Port 1 on loopback: nothing listens there.
    let result = ssh::connect(password_settings(1), creds(), None, store.clone(), 80, 24).await;
    match result {
        Err(ssh::Error::Other(message)) => assert!(message.contains("127.0.0.1:1"), "got {message}"),
        other => panic!("expected a connect error, got {other:?}"),
    }
    let _ = std::fs::remove_file(&store);
}

// ---------------------------------------------------------------------------------------
// Reconnect, driving the real Session against the live server
// ---------------------------------------------------------------------------------------

/// Everything the whole `Session` needs, pointed at the test server with its key pre-trusted.
fn ssh_session(port: u16, store: &std::path::Path) -> crate::session::Session {
    let key = PrivateKey::from_openssh(HOST_KEY).unwrap();
    knownhosts::learn("127.0.0.1", port, key.public_key(), store).unwrap();

    let mut session = crate::session::Session::new(crate::settings::ConnectionSettings {
        kind: crate::settings::ConnectionKind::Ssh,
        serial: Default::default(),
        ssh: SshSettings {
            known_hosts: Some(store.to_path_buf()),
            ..password_settings(port)
        },
    });
    session.credentials = creds();
    session
}

/// Pump the session until `ready`, or fail. This is what the UI does each frame.
async fn poll_until(
    session: &mut crate::session::Session,
    ctx: &eframe::egui::Context,
    label: &str,
    ready: impl Fn(&crate::session::Session) -> bool,
) {
    let handle = tokio::runtime::Handle::current();
    for _ in 0..600 {
        session.poll(&handle, ctx);
        if ready(session) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for {label}; state={:?} error={:?}",
        session.state, session.last_error
    );
}

fn buffer_text(session: &crate::session::Session) -> String {
    let buffer = session.buffer.lock().unwrap();
    (0..buffer.line_count())
        .filter_map(|i| buffer.line(i))
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_dropped_connection_can_be_reconnected_and_keeps_the_terminal() {
    // The headline requirement, end to end against a real server: the remote host goes away,
    // the button re-establishes the session, and nothing already on screen is lost.
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("reconnect_e2e");
    let ctx = eframe::egui::Context::default();
    let handle = tokio::runtime::Handle::current();
    let mut session = ssh_session(port, &store);

    session.connect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the first connection", |s| s.is_connected()).await;
    poll_until(&mut session, &ctx, "the greeting", |s| {
        buffer_text(s).contains("welcome")
    })
    .await;
    assert!(!session.has_connected() || session.state == ConnectionState::Connected);

    // Make the server sever the connection.
    session.send(b"DROP\n".to_vec());
    poll_until(&mut session, &ctx, "the drop to be noticed", |s| {
        s.state == ConnectionState::Disconnected
    })
    .await;
    assert!(
        session.last_error.is_some(),
        "an unexpected drop must be reported"
    );
    assert!(
        buffer_text(&session).contains("welcome"),
        "the drop must not clear the terminal"
    );
    assert!(session.can_connect(), "the button must be usable again");

    // Reconnect.
    session.reconnect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the reconnection", |s| s.is_connected()).await;
    poll_until(&mut session, &ctx, "the second greeting", |s| {
        buffer_text(s).matches("welcome").count() >= 2
    })
    .await;

    let text = buffer_text(&session);
    assert!(
        text.contains("reconnected #1"),
        "the seam must be marked; got:\n{text}"
    );
    assert_eq!(
        text.matches("welcome").count(),
        2,
        "both sessions' output should be present; got:\n{text}"
    );

    session.disconnect();
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn reconnecting_twice_in_a_row_works() {
    // "Reset the button state so the user can try to reconnect multiple times."
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("reconnect_twice");
    let ctx = eframe::egui::Context::default();
    let handle = tokio::runtime::Handle::current();
    let mut session = ssh_session(port, &store);

    session.connect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the first connection", |s| s.is_connected()).await;

    for expected in 1..=3 {
        session.send(b"DROP\n".to_vec());
        poll_until(&mut session, &ctx, "a drop", |s| {
            s.state == ConnectionState::Disconnected
        })
        .await;
        session.reconnect(&handle, &ctx);
        poll_until(&mut session, &ctx, "a reconnection", |s| s.is_connected()).await;
        poll_until(&mut session, &ctx, "the divider", |s| {
            buffer_text(s).contains(&format!("reconnected #{expected}"))
        })
        .await;
    }

    session.disconnect();
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn a_reconnect_that_fails_leaves_the_button_usable() {
    // The server is gone for good; the attempt must fail cleanly and stay retryable rather
    // than wedging in Reconnecting.
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("reconnect_fail");
    let ctx = eframe::egui::Context::default();
    let handle = tokio::runtime::Handle::current();
    let mut session = ssh_session(port, &store);

    session.connect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the first connection", |s| s.is_connected()).await;
    session.disconnect();

    // Point at a dead port so every attempt fails.
    session.settings.ssh.port = 1;
    for _ in 0..3 {
        session.reconnect(&handle, &ctx);
        poll_until(&mut session, &ctx, "the failure", |s| {
            s.state == ConnectionState::Disconnected && s.last_error.is_some()
        })
        .await;
        assert!(
            session.can_connect(),
            "a failed reconnect must re-enable the button"
        );
    }
    let _ = std::fs::remove_file(&store);
}

#[tokio::test]
async fn auto_reconnect_recovers_without_being_asked() {
    let (port, _observed) = start_server().await;
    let store = temp_known_hosts("auto_reconnect");
    let ctx = eframe::egui::Context::default();
    let handle = tokio::runtime::Handle::current();
    let mut session = ssh_session(port, &store);
    session.auto_reconnect = true;

    session.connect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the first connection", |s| s.is_connected()).await;

    session.send(b"DROP\n".to_vec());
    // No manual reconnect: polling alone should bring it back once the backoff elapses.
    poll_until(&mut session, &ctx, "automatic recovery", |s| {
        buffer_text(s).contains("reconnected #1")
    })
    .await;
    assert!(session.is_connected());

    session.disconnect();
    let _ = std::fs::remove_file(&store);
}

/// Wait until the server has observed something, or fail the test.
async fn wait_for(observed: &Arc<Mutex<Observed>>, ready: impl Fn(&Observed) -> bool) {
    for _ in 0..500 {
        if ready(&observed.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the server never observed the expected request");
}

/// Read from the transport until at least `want` bytes have arrived.
async fn read_until(transport: &mut super::transport::Transport, want: usize) -> Vec<u8> {
    use super::transport::Incoming;
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while out.len() < want {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {want} bytes");
        match tokio::time::timeout(remaining, transport.recv()).await {
            Ok(Incoming::Data(data)) => out.extend_from_slice(&data),
            Ok(Incoming::Closed(reason)) => panic!("closed early: {reason:?}"),
            Err(_) => panic!("timed out waiting for {want} bytes; got {out:?}"),
        }
    }
    out
}

/// Typing into the terminal has to reach the remote shell — the whole point of an SSH tab.
///
/// This drives the real path end to end: a real egui frame renders the ANSI grid, a real click
/// focuses it, real key events are encoded exactly as `app.rs` encodes them, and the bytes are
/// asserted at a real SSH server. It exists because that path was broken in a way none of the
/// unit tests could see: the grid allocated its rect with `allocate_exact_size`, so
/// `request_focus` targeted an auto-generated id while the focus check looked at the caller's
/// id, and `TerminalResponse::focused` was never true. Every layer worked; the composition did
/// not.
#[tokio::test]
async fn typing_into_a_focused_terminal_reaches_the_server() {
    use crate::term::input;
    use eframe::egui;

    let (port, observed) = start_server().await;
    let store = temp_known_hosts("typing_e2e");
    let ctx = egui::Context::default();
    let handle = tokio::runtime::Handle::current();
    let mut session = ssh_session(port, &store);

    // ANSI mode is what an SSH tab defaults to, and is where the bug lived.
    assert_eq!(session.display_mode, crate::settings::DisplayMode::Ansi);

    session.connect(&handle, &ctx);
    poll_until(&mut session, &ctx, "the connection", |s| s.is_connected()).await;

    let view_id = egui::Id::new("typing-e2e-view");
    let inside = egui::Pos2::new(100.0, 100.0);

    // Render, click, and type — one closure per frame, mirroring `Viewer::ui`: draw the grid,
    // then transmit if it holds focus.
    let frame = |session: &mut crate::session::Session, events: Vec<egui::Event>| -> bool {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(800.0, 600.0),
            )),
            ..Default::default()
        };
        // `Viewer::ui` does this before drawing: ANSI mode builds its screen lazily.
        session.sync_emulator();
        let mut focused = false;
        let _ = ctx.run_ui(input, |ui| {
            let font_size = session.font_size;
            let emulator = session.emulator_mut().expect("ANSI mode has an emulator");
            focused = crate::term::render::grid_view(ui, view_id, emulator, font_size, 400.0)
                .focused;
            if focused {
                let events = ui.input(|i| i.events.clone());
                let modes = session.input_modes();
                let bytes = input::encode_events(&events, session.enter_crlf, modes);
                if !bytes.is_empty() && session.is_connected() {
                    session.send(bytes);
                }
            }
        });
        focused
    };

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    frame(&mut session, vec![]);
    frame(&mut session, vec![egui::Event::PointerMoved(inside)]);
    frame(&mut session, vec![click(inside, true)]);
    assert!(
        frame(&mut session, vec![click(inside, false)]),
        "clicking the ANSI grid must focus it, or typing is never transmitted"
    );
    frame(&mut session, vec![]);

    // Ordinary printable text.
    frame(&mut session, vec![egui::Event::Text("hi".into())]);
    // Return, and a control combination, which take the key-event path rather than Text.
    frame(
        &mut session,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }],
    );
    frame(
        &mut session,
        vec![egui::Event::Key {
            key: egui::Key::C,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }],
    );

    wait_for(&observed, |o| o.received.windows(2).any(|w| w == b"hi")).await;
    wait_for(&observed, |o| o.received.contains(&0x03)).await;
    let received = observed.lock().unwrap().received.clone();
    assert!(
        received.contains(&b'\r'),
        "Return must be transmitted; got {received:?}"
    );

    // And the round trip lands back on the emulator's screen, uppercased by the test server.
    // Hand-rolled rather than `poll_until` because the screen only advances when the frame
    // syncs the emulator, which needs `&mut`.
    let mut echoed = false;
    for _ in 0..600 {
        session.poll(&handle, &ctx);
        session.sync_emulator();
        if session
            .emulator()
            .map(|e| e.all_text().contains("HI"))
            .unwrap_or(false)
        {
            echoed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        echoed,
        "the echoed reply must reach the screen; got {:?}",
        session.emulator().map(|e| e.all_text())
    );
}
