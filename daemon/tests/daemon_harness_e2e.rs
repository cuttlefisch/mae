//! The shared spawn harness's own falsifying tests (`daemon_harness`).
//!
//! Each test takes one of the daemon's ports away on the first attempt, the
//! way another process can between the harness finding a free port and the
//! daemon binding it, and holds it bound but not listening — what another
//! test's outbound connection looks like from the port's side. A connect to
//! such a port is refused, which is the CI symptom this harness exists for.
//! The daemon does not exit when its bind fails, so:
//!
//! - with its log readable, the harness must notice the loss and respawn on
//!   fresh ports, returning a daemon that serves both;
//! - with its log silenced (`RUST_LOG=off`), the loss is invisible, and the
//!   readiness proof itself must refuse to call the daemon ready. A harness
//!   that only connected, or waited on one listener, would return it.

mod daemon_harness;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon_harness::{Launch, OauthProbe, Ports, Process};

#[derive(Clone, Copy)]
enum Steal {
    Collab,
    Oauth,
}

struct Attempts {
    ports: Mutex<Vec<Ports>>,
    /// The sockets holding stolen ports, kept open for the whole test.
    held: Mutex<Vec<tokio::net::TcpSocket>>,
}

/// A launcher that steals `steal` on its first call. `quiet` silences the
/// daemon's log, hiding the bind error from the harness.
fn launcher(
    root: PathBuf,
    steal: Steal,
    quiet: bool,
    attempts: Arc<Attempts>,
) -> impl FnMut(Ports) -> Launch + Send {
    let (cert, key) = (root.join("oauth.crt"), root.join("oauth.key"));
    let cert_der = daemon_harness::write_self_signed_cert(&cert, &key);
    move |ports| {
        if attempts.ports.lock().unwrap().is_empty() {
            let port = match steal {
                Steal::Collab => ports.collab,
                Steal::Oauth => ports.oauth,
            };
            let thief = tokio::net::TcpSocket::new_v4().unwrap();
            thief
                .bind(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
                .unwrap();
            attempts.held.lock().unwrap().push(thief);
        }
        attempts.ports.lock().unwrap().push(ports);
        let config = format!(
            "[collab]\nenabled = true\nbind = \"127.0.0.1:{c}\"\n[collab.auth]\nmode = \"none\"\n\
             [oauth]\nenabled = true\nbind = \"127.0.0.1:{o}\"\n\
             canonical_resource_uri = \"https://127.0.0.1/mcp\"\n\
             jwks_url = \"http://127.0.0.1:1/unused\"\n\
             cert_path = \"{cert}\"\nkey_path = \"{key}\"\n",
            c = ports.collab,
            o = ports.oauth,
            cert = cert.display(),
            key = key.display(),
        );
        let config_path = root.join("daemon.toml");
        std::fs::write(&config_path, config).unwrap();
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
        cmd.args([
            "--config",
            config_path.to_str().unwrap(),
            "--data-dir",
            root.to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", &root);
        if quiet {
            cmd.env("RUST_LOG", "off");
        }
        Launch {
            cmd,
            status_socket: root.join("mae-daemon.sock"),
            log: root.join("daemon.log"),
            collab: true,
            oauth: Some(OauthProbe {
                cert_der: cert_der.clone(),
                ws_ticket: false,
            }),
        }
    }
}

fn new_attempts() -> Arc<Attempts> {
    Arc::new(Attempts {
        ports: Mutex::new(Vec::new()),
        held: Mutex::new(Vec::new()),
    })
}

fn stolen(attempts: &Attempts, steal: Steal) -> u16 {
    let first = attempts.ports.lock().unwrap()[0];
    match steal {
        Steal::Collab => first.collab,
        Steal::Oauth => first.oauth,
    }
}

/// Both listeners answer, and neither sits on the port that was taken.
async fn assert_serves_both(proc: &Process, stolen: u16) {
    assert_ne!(proc.ports.collab, stolen);
    assert_ne!(proc.ports.oauth, stolen);
    tokio::net::TcpStream::connect(proc.ports.collab_addr())
        .await
        .unwrap_or_else(|e| panic!("collab not served after respawn: {e}"));
    tokio::net::TcpStream::connect(proc.ports.oauth_addr())
        .await
        .unwrap_or_else(|e| panic!("OAuth not served after respawn: {e}"));
}

async fn respawns_after_losing(steal: Steal) {
    let tmp = tempfile::tempdir().unwrap();
    let attempts = new_attempts();
    let proc = daemon_harness::spawn_ready(launcher(
        tmp.path().to_path_buf(),
        steal,
        false,
        Arc::clone(&attempts),
    ))
    .await;
    assert_eq!(
        attempts.ports.lock().unwrap().len(),
        2,
        "one lost attempt, one good one"
    );
    assert_serves_both(&proc, stolen(&attempts, steal)).await;
}

/// With the bind error hidden, the daemon is up but missing a listener; the
/// harness must time out rather than hand it to a test.
async fn refuses_a_daemon_missing(steal: Steal, root: &Path) {
    let attempts = new_attempts();
    let launch = launcher(root.to_path_buf(), steal, true, Arc::clone(&attempts));
    let spawned = tokio::spawn(async move {
        daemon_harness::spawn_ready_within(Duration::from_secs(5), launch).await
    })
    .await;
    match spawned {
        Err(e) if e.is_panic() => {
            let msg = e.into_panic();
            let msg = msg.downcast_ref::<String>().cloned().unwrap_or_default();
            assert!(msg.contains("did not become ready"), "{msg}");
        }
        Err(e) => panic!("unexpected join error: {e}"),
        Ok(proc) => panic!(
            "the harness called a daemon ready although its port {} was taken (now on {:?})",
            stolen(&attempts, steal),
            proc.ports
        ),
    }
    assert_eq!(
        attempts.ports.lock().unwrap().len(),
        1,
        "no visible loss, no respawn"
    );
}

#[tokio::test]
async fn a_daemon_that_loses_its_oauth_port_is_respawned_on_fresh_ones() {
    respawns_after_losing(Steal::Oauth).await;
}

#[tokio::test]
async fn a_daemon_that_loses_its_collab_port_is_respawned_on_fresh_ones() {
    respawns_after_losing(Steal::Collab).await;
}

#[tokio::test]
async fn a_daemon_without_its_oauth_listener_is_never_reported_ready() {
    let tmp = tempfile::tempdir().unwrap();
    refuses_a_daemon_missing(Steal::Oauth, tmp.path()).await;
}

#[tokio::test]
async fn a_daemon_without_its_collab_listener_is_never_reported_ready() {
    let tmp = tempfile::tempdir().unwrap();
    refuses_a_daemon_missing(Steal::Collab, tmp.path()).await;
}
