//! Shared harness for the sync-WebSocket e2e suites (`ws_upgrade_e2e.rs`,
//! `ws_session_e2e.rs`; ADR-099 Amendment 1).
//!
//! A real `mae-daemon` binary, a real TLS listener, real self-issued tokens
//! signed with the daemon's own identity, and a real WebSocket client. The
//! upgrade request is written by hand rather than through a client library so a
//! refusal's status, headers and body can be compared byte for byte — the
//! "identical refusal" properties are about exactly those bytes.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mae_daemon::oauth_self_issue::mint_self_token;
use mae_mcp::identity::Identity;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::client::TlsStream;
use tokio_websockets::{Message, WebSocketStream};

pub const AUDIENCE: &str = "https://127.0.0.1/mcp";
pub const ORIGIN_A: &str = "https://app.example";
pub const ORIGIN_B: &str = "https://other.example";
const WAIT: Duration = Duration::from_secs(15);

/// The `[oauth] ws_*` knobs a test varies.
#[derive(Clone)]
pub struct Opts {
    pub ws_enabled: bool,
    pub renew_lead_secs: u64,
    pub max_open_docs: usize,
    pub max_frame_bytes: Option<usize>,
    pub tenant_budget: Option<(String, u32)>,
    /// `ws_ping_secs`; `None` leaves the daemon's default.
    pub ping_secs: Option<u64>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            ws_enabled: true,
            renew_lead_secs: 60,
            max_open_docs: 1024,
            max_frame_bytes: None,
            tenant_budget: None,
            ping_secs: None,
        }
    }
}

pub struct Daemon {
    proc: Option<crate::daemon_harness::Process>,
    tmp: tempfile::TempDir,
    pub addr: SocketAddr,
    pub identity: Arc<Identity>,
    pub socket: PathBuf,
    cert_der: Vec<u8>,
    config_path: PathBuf,
    opts: Opts,
}

fn config_toml(dir: &Path, ports: crate::daemon_harness::Ports, opts: &Opts) -> String {
    let id = dir.join("identity");
    let tenant = opts
        .tenant_budget
        .as_ref()
        .map(|(p, b)| format!("[[tenant]]\nname = \"t\"\nprincipals = [\"{p}\"]\n[tenant.quota]\nbudget_per_minute = {b}\n"))
        .unwrap_or_default();
    let ping = opts
        .ping_secs
        .map(|n| format!("ws_ping_secs = {n}\n"))
        .unwrap_or_default();
    let frame = opts
        .max_frame_bytes
        .map(|n| format!("ws_max_frame_bytes = {n}\n"))
        .unwrap_or_default()
        + ping.as_str();
    format!(
        "[collab]\nenabled = true\nbind = \"127.0.0.1:{collab}\"\n\
         [collab.auth]\nmode = \"key\"\nidentity_dir = \"{id}\"\nauthorized_keys = \"{id}/authorized_keys\"\n\
         [oauth]\nenabled = true\nbind = \"127.0.0.1:{oauth}\"\ncanonical_resource_uri = \"{AUDIENCE}\"\n\
         cert_path = \"{d}/oauth.crt\"\nkey_path = \"{d}/oauth.key\"\nkb_query_enabled = true\n\
         self_issued_tokens_enabled = true\nmax_connections = 0\n\
         ws_enabled = {ws}\nws_allowed_origins = [\"{ORIGIN_A}\", \"{ORIGIN_B}\"]\n\
         ws_renew_lead_secs = {lead}\nws_max_open_docs = {docs}\n{frame}\n{tenant}",
        collab = ports.collab,
        oauth = ports.oauth,
        id = id.display(),
        d = dir.display(),
        ws = opts.ws_enabled,
        lead = opts.renew_lead_secs,
        docs = opts.max_open_docs,
    )
}

/// Spawn a daemon with the sync WebSocket configured by `opts`.
pub async fn spawn(opts: Opts) -> Daemon {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let cert_der = crate::daemon_harness::write_self_signed_cert(
        &dir.join("oauth.crt"),
        &dir.join("oauth.key"),
    );
    let identity = Arc::new(Identity::generate("daemon-under-test"));
    identity.save(&dir.join("identity")).unwrap();
    let client = Identity::generate("throwaway-client").public().to_line();
    std::fs::write(dir.join("identity/authorized_keys"), format!("{client}\n")).unwrap();
    let config_path = dir.join("daemon.toml");
    let mut d = Daemon {
        proc: None,
        // Set by `start`, from the ports the daemon actually came up on.
        addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        socket: dir.join("mae-daemon.sock"),
        identity,
        cert_der,
        config_path,
        tmp,
        opts,
    };
    d.start().await;
    d
}

impl Daemon {
    /// Start the daemon and wait until THIS daemon serves its port, on fresh
    /// ports each time. The port choice, the readiness proof (a TLS handshake
    /// pinned to this daemon's certificate, `/ws/ticket` answering when ws is
    /// on) and the respawn after a lost port are `daemon_harness`'s, shared with
    /// every daemon suite; this harness was where they were first worked out
    /// (PR #886), after a probe that merely connected reached another test's
    /// daemon on a reused port.
    async fn start(&mut self) {
        let dir = self.tmp.path().to_path_buf();
        let probe = crate::daemon_harness::OauthProbe {
            cert_der: self.cert_der.clone(),
            ws_ticket: self.opts.ws_enabled,
        };
        let (config_path, socket, opts) = (&self.config_path, &self.socket, &self.opts);
        let proc = crate::daemon_harness::spawn_ready(|ports| {
            std::fs::write(config_path, config_toml(&dir, ports, opts)).unwrap();
            let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"));
            cmd.args([
                "--config",
                config_path.to_str().unwrap(),
                "--data-dir",
                dir.to_str().unwrap(),
            ])
            .env("XDG_RUNTIME_DIR", &dir);
            crate::daemon_harness::Launch {
                cmd,
                status_socket: socket.clone(),
                log: dir.join("daemon.log"),
                collab: true,
                oauth: Some(probe.clone()),
            }
        })
        .await;
        self.addr = proc.ports.oauth_addr();
        self.proc = Some(proc);
    }

    /// Kill and start the same daemon again: same config, identity and data
    /// (on fresh ports: a released port is no more ours than any other).
    pub async fn restart(&mut self) {
        if let Some(mut p) = self.proc.take() {
            p.stop().await;
        }
        self.start().await;
    }

    /// A self-issued token for `sub`, valid `ttl` seconds.
    pub fn token(&self, sub: &str, ttl: u64) -> String {
        mint_self_token(&self.identity, sub, AUDIENCE, ttl).expect("mint")
    }

    async fn tls(&self) -> TlsStream<tokio::net::TcpStream> {
        let tcp = tokio::net::TcpStream::connect(self.addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
        self.connector()
            .connect(name, tcp)
            .await
            .expect("TLS handshake")
    }

    /// A TLS client trusting ONLY this daemon's certificate.
    fn connector(&self) -> tokio_rustls::TlsConnector {
        crate::daemon_harness::pinned_connector(&self.cert_der)
    }
}

/// An HTTP response read off the wire, or a live socket after `101`.
pub struct Reply {
    pub status: u16,
    pub head: String,
    pub body: String,
    pub ws: Option<Ws>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_default()
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    }
    /// Status line, headers minus `date`, and body: what two refusals must
    /// share to be "identical".
    pub fn fingerprint(&self) -> String {
        let head: Vec<&str> = self
            .head
            .lines()
            .filter(|l| !l.to_ascii_lowercase().starts_with("date:"))
            .collect();
        format!("{}\n{}", head.join("\n"), self.body)
    }
}

async fn read_head(s: &mut TlsStream<tokio::net::TcpStream>) -> String {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).await.unwrap_or(0) == 0 {
            break;
        }
        head.push(b[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// Send one raw HTTP request (`GET` upgrade or otherwise) with `headers`.
pub async fn request(
    d: &Daemon,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    upgrade: bool,
) -> Reply {
    request_on(d.tls().await, method, path, headers, upgrade).await
}

/// `GET /ws` like [`upgrade`], over a client socket whose kernel send buffer
/// is `send_buffer` bytes: a large frame then cannot all be buffered, so the
/// client is still writing it when the daemon reacts — what a test needs to
/// see how the daemon closes on a client mid-write.
pub async fn upgrade_small_send_buffer(
    d: &Daemon,
    protocols: &str,
    extra: &[(&str, &str)],
    send_buffer: u32,
) -> Reply {
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_send_buffer_size(send_buffer).unwrap();
    let tcp = sock.connect(d.addr).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
    let tls = d
        .connector()
        .connect(name, tcp)
        .await
        .expect("TLS handshake");
    let mut h: Vec<(&str, &str)> = extra.to_vec();
    h.push(("Sec-WebSocket-Protocol", protocols));
    request_on(tls, "GET", "/ws", &h, true).await
}

async fn request_on(
    mut s: TlsStream<tokio::net::TcpStream>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    upgrade: bool,
) -> Reply {
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    if upgrade {
        req.push_str("Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n");
        req.push_str("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n");
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Content-Length: 0\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let head = tokio::time::timeout(WAIT, read_head(&mut s))
        .await
        .expect("response head");
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let mut reply = Reply {
        status,
        head,
        body: String::new(),
        ws: None,
    };
    if status == 101 {
        let inner = tokio_websockets::ClientBuilder::new().take_over(s);
        reply.ws = Some(Ws {
            inner,
            log: Vec::new(),
        });
        return reply;
    }
    let len: usize = reply
        .header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.unwrap();
    reply.body = String::from_utf8_lossy(&body).into_owned();
    reply
}

/// `GET /ws` with a subprotocol list and extra headers.
pub async fn upgrade(d: &Daemon, protocols: &str, extra: &[(&str, &str)]) -> Reply {
    let mut h: Vec<(&str, &str)> = extra.to_vec();
    if !protocols.is_empty() {
        h.push(("Sec-WebSocket-Protocol", protocols));
    }
    request(d, "GET", "/ws", &h, true).await
}

/// `GET /ws` with a bearer header and `mae.v1`, expected to open.
pub async fn connect_bearer(d: &Daemon, token: &str) -> Ws {
    let auth = format!("Bearer {token}");
    let r = upgrade(d, "mae.v1", &[("Authorization", &auth)]).await;
    assert_eq!(r.status, 101, "bearer upgrade refused: {}", r.body);
    r.ws.unwrap()
}

/// `POST /ws/ticket` with `token`, from `origin` if any.
pub async fn mint_ticket(d: &Daemon, token: &str, origin: Option<&str>) -> Reply {
    let auth = format!("Bearer {token}");
    let mut h = vec![("Authorization", auth.as_str())];
    if let Some(o) = origin {
        h.push(("Origin", o));
    }
    request(d, "POST", "/ws/ticket", &h, false).await
}

/// What the client saw next.
#[derive(Debug)]
pub enum Event {
    Text(Value),
    Close(u16, String),
    End,
}

/// A WebSocket client that keeps every raw text frame it receives (`log`) —
/// the leak oracle scans those bytes, not parsed values.
pub struct Ws {
    inner: WebSocketStream<TlsStream<tokio::net::TcpStream>>,
    pub log: Vec<String>,
}

impl Ws {
    pub async fn send(&mut self, v: Value) {
        self.send_raw(v.to_string()).await;
    }

    pub async fn send_raw(&mut self, text: String) {
        use futures_util::SinkExt;
        self.inner
            .send(Message::text(text))
            .await
            .expect("send frame");
    }

    pub async fn send_binary(&mut self, bytes: Vec<u8>) {
        use futures_util::SinkExt;
        self.inner
            .send(Message::binary(bytes))
            .await
            .expect("send frame");
    }

    /// Wait for a ping from the daemon, skipping any other frame.
    pub async fn pinged(&mut self, within: std::time::Duration) -> bool {
        use futures_util::StreamExt;
        let until = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(until, self.inner.next()).await {
                Err(_) | Ok(None) | Ok(Some(Err(_))) => return false,
                Ok(Some(Ok(m))) if m.is_ping() => return true,
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    pub async fn next(&mut self) -> Event {
        use futures_util::StreamExt;
        let frame = loop {
            match tokio::time::timeout(WAIT, self.inner.next()).await {
                Ok(Some(Ok(m))) if m.is_ping() || m.is_pong() => continue,
                other => break other,
            }
        };
        match frame {
            Err(_) => panic!("no frame within {WAIT:?}"),
            Ok(None) | Ok(Some(Err(_))) => Event::End,
            Ok(Some(Ok(m))) if m.is_close() => {
                let (code, reason) = m
                    .as_close()
                    .map(|(c, r)| (u16::from(c), r.to_string()))
                    .unwrap_or((0, String::new()));
                Event::Close(code, reason)
            }
            Ok(Some(Ok(m))) => {
                let text = m.as_text().unwrap_or_default().to_string();
                self.log.push(text.clone());
                Event::Text(serde_json::from_str(&text).unwrap_or_default())
            }
        }
    }

    /// The next text frame matching `pred`, skipping others. Panics on close.
    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            match self.next().await {
                Event::Text(v) if pred(&v) => return v,
                Event::Text(_) => {}
                other => panic!("socket ended while waiting: {other:?}"),
            }
        }
    }

    /// Send a request and return its response.
    pub async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        self.until(|v| v["id"] == id && v.get("method").is_none())
            .await
    }

    /// The close the socket ends with, skipping text frames.
    pub async fn closed(&mut self) -> (u16, String) {
        loop {
            match self.next().await {
                Event::Close(c, r) => return (c, r),
                Event::End => return (0, String::new()),
                Event::Text(_) => {}
            }
        }
    }
}

/// `(status, error, kind)` of an HTTP refusal: the listener's body
/// convention plus the agreed `kind`.
pub fn refusal(r: &Reply) -> (u16, String, String) {
    let j = r.json();
    let field = |k: &str| j[k].as_str().unwrap_or("").to_string();
    (r.status, field("error"), field("kind"))
}

/// An `auth_deadline` (RFC 3339, UTC) as unix seconds.
pub fn deadline_unix(v: &Value) -> u64 {
    let s = v.as_str().expect("auth_deadline is an RFC 3339 string");
    humantime::parse_rfc3339(s)
        .expect("valid RFC 3339")
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The `data.kind` of a JSON-RPC error response.
pub fn kind(resp: &Value) -> String {
    resp["error"]["data"]["kind"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// A `kb/share` of `kb_id` owned by the sharer, with `nodes` as `(id, title, body)`.
pub fn kb_share(kb_id: &str, nodes: &[(&str, &str, &str)]) -> Value {
    use mae_sync::encoding::update_to_base64;
    let mut coll = mae_sync::kb::KbCollectionDoc::new(kb_id, "");
    let mut node_states = Vec::new();
    for (id, title, body) in nodes {
        coll.add_node(id, title);
        let doc = mae_sync::kb::KbNodeDoc::new(id, title, body, &[]);
        node_states.push(json!({"id": id, "state": update_to_base64(&doc.encode())}));
    }
    json!({"kb_id": kb_id, "name": kb_id, "collection_state": update_to_base64(&coll.encode_state()), "nodes": node_states})
}

/// `daemon/status` over the KB socket.
#[allow(unused_imports)]
pub use crate::daemon_harness::daemon_status;
