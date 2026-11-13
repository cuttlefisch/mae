//! Start a real `mae-daemon` on TCP ports and wait until THAT daemon serves them.
//!
//! The one place the daemon e2e suites choose ports and decide readiness. Every
//! suite that spawns a serving daemon (`common`, `ws_common`, `oauth_e2e`,
//! `self_issued_token_e2e`, `token_mint_e2e`, `oauth_cert_reload_e2e`,
//! `remote_hub_query_layer_e2e`, `multi_tenant_process_isolation_e2e`,
//! `windows_remote_verify_smoke_test`) goes through [`spawn_ready`]. They used to
//! carry eight copies of "pick a free port, start the daemon, poll until a TCP
//! connect succeeds", and each copy had the same two defects:
//!
//! * **A free port can only be found, not reserved.** [`Ports::fresh`] binds `:0`,
//!   reads the number and releases it so the daemon can bind it. In between,
//!   anything on the machine can take it; under a parallel test run the likeliest
//!   taker is another test's outbound connection, whose local port comes from the
//!   same ephemeral range. The daemon does not exit when its bind fails: it logs
//!   the error, disables that one listener and keeps serving the rest.
//! * **A connect proves nothing about whose listener answered, or about the other
//!   listener.** The OAuth listener binds in a task spawned *after* the collab
//!   listener is up, so a harness that waited on collab alone handed the test a
//!   daemon whose OAuth port was not listening yet — or never would be.
//!
//! Both surfaced in CI as "connection refused" after the harness had declared the
//! daemon ready (`windows_remote_verify_smoke_test`, and the WebSocket suites
//! before PR #886). Readiness here is therefore proven, not inferred:
//!
//! 1. `daemon/status` answers on the daemon's own KB socket — a path inside the
//!    test's temp dir that no other process can serve — and reports the collab
//!    listener present or absent as configured. The daemon registers it only after
//!    a successful bind, so "present" means "bound".
//! 2. The OAuth port completes a TLS handshake pinned to the certificate this test
//!    generated, which no other process holds (plus, for the WebSocket suites, the
//!    ticket route answers `401`).
//! 3. The probe's own OAuth connection has been released (`oauth.active == 0`), so a
//!    test that counts connections or sets a connection cap starts from zero.
//!
//! A port lost to someone else appears in the daemon's log as a bind error; that
//! attempt is killed and the daemon is respawned on fresh ports. Anything else that
//! keeps it from becoming ready — it exits, or the wall-clock budget runs out —
//! fails the test with the tail of the daemon's own log attached.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How many times a daemon that lost a port to another process is respawned.
/// One loss is already rare; three in a row means something is holding ports
/// systematically, which a test should report rather than retry around.
const MAX_ATTEMPTS: usize = 3;

/// The ports one attempt hands to the daemon.
#[derive(Clone, Copy, Debug)]
pub struct Ports {
    pub collab: u16,
    pub oauth: u16,
}

impl Ports {
    /// Two distinct free ports. Both listeners are held at once, so the kernel
    /// cannot hand the same number out twice (two sequential `:0` binds can).
    pub fn fresh() -> Self {
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        Ports {
            collab: a.local_addr().unwrap().port(),
            oauth: b.local_addr().unwrap().port(),
        }
    }
    pub fn collab_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.collab))
    }
    pub fn oauth_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.oauth))
    }
}

/// The OAuth listener's identity: the certificate it must present.
#[derive(Clone)]
pub struct OauthProbe {
    pub cert_der: Vec<u8>,
    /// Also require `POST /ws/ticket` to answer `401` (the sync WebSocket is
    /// configured, which needs the collab service wired into the listener).
    pub ws_ticket: bool,
}

/// One attempt: the command to run and what THIS daemon must show to be ready.
pub struct Launch {
    /// The daemon command. Its stdout and stderr are redirected to `log`.
    pub cmd: tokio::process::Command,
    /// The daemon's KB socket, where `daemon/status` is asked.
    pub status_socket: PathBuf,
    /// Where the daemon's output goes; scanned for bind errors, quoted on failure.
    pub log: PathBuf,
    /// Whether the collab listener must be bound (`true`) or absent (`false`).
    pub collab: bool,
    /// When set, the OAuth listener on `Ports::oauth` must present this identity.
    pub oauth: Option<OauthProbe>,
}

/// A running daemon. Killed when dropped.
pub struct Process {
    child: tokio::process::Child,
    pub ports: Ports,
    log: PathBuf,
}

impl Process {
    /// Kill the daemon and reap it.
    pub async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    /// Whether the daemon has exited, and how.
    pub fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// The OS process, for tests that signal it directly.
    pub fn child_mut(&mut self) -> &mut tokio::process::Child {
        &mut self.child
    }

    /// The end of the daemon's own log, for a failure message.
    pub fn log_tail(&self) -> String {
        log_tail(&self.log)
    }
}

/// Spawn a daemon and wait until it is proven ready (see the module doc).
///
/// `launch` is called once per attempt with fresh [`Ports`]; it writes whatever
/// config those ports need and returns the command. A respawn after a lost port
/// calls it again, so nothing in it may assume a previous call's ports.
pub async fn spawn_ready(launch: impl FnMut(Ports) -> Launch) -> Process {
    spawn_ready_within(mae_mcp::ready::ready_timeout(), launch).await
}

/// [`spawn_ready`] with an explicit budget. Only the harness's own tests need
/// one: they prove a daemon that never becomes ready is NOT reported ready, and
/// should not wait the full default budget to do it.
pub async fn spawn_ready_within(
    budget: Duration,
    mut launch: impl FnMut(Ports) -> Launch,
) -> Process {
    let deadline = Instant::now() + budget;
    let mut lost = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        let ports = Ports::fresh();
        let Launch {
            mut cmd,
            status_socket,
            log,
            collab,
            oauth,
        } = launch(ports);
        let out = std::fs::File::create(&log).expect("create daemon log");
        let err = out.try_clone().expect("clone daemon log handle");
        let child = cmd
            .stdout(out)
            .stderr(err)
            .env("NO_COLOR", "1")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn mae-daemon");
        let mut proc = Process { child, ports, log };
        let want = Want {
            socket: &status_socket,
            collab,
            oauth: oauth.as_ref(),
            ports,
        };
        match wait_ready(&mut proc, &want, deadline).await {
            Outcome::Ready => return proc,
            Outcome::PortLost(line) => {
                lost.push(format!("{ports:?}: {line}"));
                proc.stop().await;
            }
            Outcome::Exited(status) => panic!(
                "mae-daemon exited ({status}) before becoming ready on {ports:?}\n\
                 --- daemon log ---\n{}",
                proc.log_tail()
            ),
            Outcome::TimedOut(what) => panic!(
                "{}\n--- daemon log ---\n{}",
                mae_mcp::ready::timeout_message(&format!("mae-daemon on {ports:?} ({what})")),
                proc.log_tail()
            ),
        }
    }
    panic!(
        "mae-daemon lost a port to another process on all {MAX_ATTEMPTS} attempts:\n{}",
        lost.join("\n")
    );
}

struct Want<'a> {
    socket: &'a Path,
    collab: bool,
    oauth: Option<&'a OauthProbe>,
    ports: Ports,
}

enum Outcome {
    Ready,
    PortLost(String),
    Exited(std::process::ExitStatus),
    /// Names the condition still unmet when the budget ran out.
    TimedOut(&'static str),
}

async fn wait_ready(proc: &mut Process, want: &Want<'_>, deadline: Instant) -> Outcome {
    // Set once the pinned handshake has succeeded; from then on only the
    // release of that probe connection is awaited, so the port is probed once.
    let mut oauth_seen = false;
    loop {
        if let Some(status) = proc.try_wait() {
            return Outcome::Exited(status);
        }
        if let Some(line) = bind_error(&proc.log) {
            return Outcome::PortLost(line);
        }
        let unmet = match ready_step(want, &mut oauth_seen).await {
            Ok(()) => return Outcome::Ready,
            Err(what) => what,
        };
        if Instant::now() >= deadline {
            return Outcome::TimedOut(unmet);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One readiness poll. `Err` names the first condition not yet met.
async fn ready_step(want: &Want<'_>, oauth_seen: &mut bool) -> Result<(), &'static str> {
    let status = try_status(want.socket)
        .await
        .ok_or("daemon/status on its KB socket")?;
    let conns = &status["connections"];
    if conns["collab"].is_object() != want.collab {
        return Err(if want.collab {
            "the collab listener bound"
        } else {
            "the collab listener absent"
        });
    }
    let Some(probe) = want.oauth else {
        return Ok(());
    };
    if !*oauth_seen {
        if !serves_this_daemon(want.ports.oauth_addr(), probe).await {
            return Err("the OAuth listener presenting this test's certificate");
        }
        *oauth_seen = true;
        return Err("the readiness probe's OAuth connection released");
    }
    match conns["oauth"]["active"].as_u64() {
        Some(0) => Ok(()),
        _ => Err("the readiness probe's OAuth connection released"),
    }
}

/// The first line of the daemon log reporting that a port was already taken.
/// The collab listener logs "collab address already in use"; the OAuth one
/// logs the io error ("Address already in use (os error 98)" on Linux, 48 on
/// macOS) under "OAuth listener failed to start".
fn bind_error(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines()
        .find(|l| l.to_ascii_lowercase().contains("already in use"))
        .map(str::to_string)
}

fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

/// The OAuth port answers with THIS daemon's certificate, and (when asked) the
/// ticket route exists.
async fn serves_this_daemon(addr: SocketAddr, probe: &OauthProbe) -> bool {
    let Ok(Ok(tcp)) = tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    else {
        return false;
    };
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
    let Ok(Ok(mut tls)) = tokio::time::timeout(
        Duration::from_millis(500),
        pinned_connector(&probe.cert_der).connect(name, tcp),
    )
    .await
    else {
        return false;
    };
    if !probe.ws_ticket {
        let _ = tls.shutdown().await;
        return true;
    }
    let req = "POST /ws/ticket HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    if tls.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_millis(500), async {
        while !head.ends_with(b"\r\n\r\n") {
            if tls.read(&mut b).await.unwrap_or(0) == 0 {
                break;
            }
            head.push(b[0]);
        }
    })
    .await;
    let _ = tls.shutdown().await;
    read.is_ok()
        && String::from_utf8_lossy(&head)
            .split_whitespace()
            .nth(1)
            .is_some_and(|code| code == "401")
}

/// A TLS client trusting ONLY `cert_der`.
pub fn pinned_connector(cert_der: &[u8]) -> tokio_rustls::TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der.to_vec().into()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

/// Write a fresh self-signed certificate for `127.0.0.1` (a distinct key per
/// call) and return its DER, which is what [`OauthProbe`] pins.
pub fn write_self_signed_cert(cert_path: &Path, key_path: &Path) -> Vec<u8> {
    let ck = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
        .expect("rcgen self-signed cert");
    std::fs::write(cert_path, ck.cert.pem()).unwrap();
    std::fs::write(key_path, ck.signing_key.serialize_pem()).unwrap();
    ck.cert.der().to_vec()
}

/// `daemon/status` over the KB socket, or `None` while the daemon is not
/// answering yet.
pub async fn try_status(socket: &Path) -> Option<Value> {
    let mut stream = tokio::net::UnixStream::connect(socket).await.ok()?;
    let (r, mut w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let body = serde_json::to_vec(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}}),
    )
    .ok()?;
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2))
        .await
        .ok()?;
    let msg = mae_mcp::read_message(&mut reader).await.ok()??;
    let v: Value = serde_json::from_str(&msg).ok()?;
    v.get("result").cloned()
}

/// `daemon/status` over the KB socket; panics if the daemon does not answer.
pub async fn daemon_status(socket: &Path) -> Value {
    use tokio::io::BufReader;
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .unwrap_or_else(|e| panic!("connect {}: {e}", socket.display()));
    let (r, mut w) = stream.split();
    let mut reader = BufReader::new(r);
    let body = serde_json::to_vec(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}}),
    )
    .unwrap();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(5))
        .await
        .expect("write daemon/status");
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .expect("read daemon/status")
        .expect("response before EOF");
    let v: Value = serde_json::from_str(&msg).expect("parse daemon/status");
    assert!(v.get("error").is_none(), "daemon/status failed: {v:?}");
    v["result"].clone()
}
