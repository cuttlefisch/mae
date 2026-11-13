//! The life of one upgraded socket (ADR-099 Amendment 1, B2/B3/B4).
//!
//! The seam to the shared session loop is an in-memory byte pipe: the session
//! runs `collab_handler::handle_client_governed` on one end, exactly as the
//! collab listener runs it on a TCP stream, and this driver sits on the other.
//! An inbound text frame is written to the pipe as one `Content-Length` frame
//! (the length is the frame's own, so a frame can never smuggle a second
//! message); every message the session writes is read back with the same
//! `mae_mcp::read_message` and sent as one text frame. Nothing here parses a
//! `sync/*` or `kb/*` message beyond its method name.
//!
//! What the driver owns: the first frame (`session/hello`, B2), the renewal
//! protocol (`session/renew_needed` / `session/renew`, B3), the close codes
//! (`4401 token_expired`, `4403 principal_changed`, `1009` from the frame cap),
//! and the write timeout that drops a client that stops reading (D4).
//!
//! @ai-caution: [security] `session/*` frames never reach the session loop; a
//! renewal token is validated here and never logged.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use mae_daemon::collab_handler::governor::{governor, GovernorHandle};
use mae_daemon::oauth_self_issue::{self, ValidatedPrincipal};
use mae_mcp::identity::PeerIdentity;
use mae_mcp::protocol::McpError;
use serde_json::{json, Value};
use tokio::io::{BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio_websockets::{CloseCode, Limits, Message, ServerBuilder, WebSocketStream};

use super::upgrade::SessionAuth;
use super::{now_unix, WsSettings, PROTOCOL_V1};
use crate::conn_limit::ConnGuard;
use crate::oauth::Listener;

type Socket = WebSocketStream<TokioIo<Upgraded>>;

/// A client that does not read for this long is dropped, not waited on (D4,
/// the same bound the collab session uses for its own writes).
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Buffer of the pipe between this driver and the session loop.
const PIPE_BYTES: usize = 1 << 20;
/// B3: a missed renewal deadline.
const CLOSE_TOKEN_EXPIRED: u16 = 4401;
/// B3: a renewal carrying a different identity.
const CLOSE_PRINCIPAL_CHANGED: u16 = 4403;
/// RFC 6455 "unsupported data": the envelope is JSON text.
const CLOSE_UNSUPPORTED_DATA: u16 = 1003;

enum Flow {
    Continue,
    Close(u16, &'static str),
    Stop,
}

struct Driver {
    ws: Socket,
    to_session: WriteHalf<DuplexStream>,
    handle: GovernorHandle,
    auth: SessionAuth,
    listener: Arc<Listener>,
    renew_lead: Duration,
    renew_sent: bool,
    ping_every: Option<Duration>,
    /// Most bytes [`Driver::linger`] discards before giving up on a client.
    linger_bytes: usize,
}

/// Run one socket until it closes. Holds `conn` — the TCP connection's one
/// `ConnLimiter` slot — until the session loop behind it has really ended.
pub(super) async fn run(
    on_upgrade: OnUpgrade,
    auth: SessionAuth,
    listener: Arc<Listener>,
    conn: Arc<ConnGuard>,
) {
    let upgraded = match on_upgrade.await {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!(error = %e, "ws: upgrade did not complete");
            return;
        }
    };
    let state = listener
        .ws
        .as_ref()
        .expect("a socket exists only when ws is Some");
    let settings = state.settings.clone();
    let limits = Limits::default().max_payload_len(Some(settings.max_frame_bytes));
    let ws = ServerBuilder::new()
        .limits(limits)
        .serve(TokioIo::new(upgraded));
    let (gov, handle) = governor(settings.max_open_docs, auth.deadline);
    let (client_end, session_end) = tokio::io::duplex(PIPE_BYTES);
    let (from_session, to_session) = tokio::io::split(client_end);
    let principal = auth.principal.clone();
    let mut driver = Driver {
        ws,
        to_session,
        handle,
        auth,
        listener: Arc::clone(&listener),
        renew_lead: settings.renew_lead,
        renew_sent: false,
        ping_every: settings.ping_every,
        // One frame over the cap may be mid-flight when the daemon closes,
        // with a frame queued behind it: two caps' worth.
        linger_bytes: settings.max_frame_bytes.saturating_mul(2),
    };
    tracing::info!(principal = %principal, "ws: session opened");
    // B2: the first frame the daemon sends, before the session can write.
    if let Flow::Continue = driver.send_json(hello(&driver.auth, &settings)).await {
        let (r, w) = tokio::io::split(session_end);
        let mut session = tokio::spawn(mae_daemon::collab_handler::handle_client_governed(
            BufReader::new(r),
            w,
            PeerIdentity::bearer(&principal),
            state.deps.clone(),
            gov,
        ));
        let (out_tx, out_rx) = mpsc::channel(32);
        let pump = tokio::spawn(pump(from_session, out_tx));
        driver.drive(out_rx).await;
        // Closing both ends of the pipe ends the session loop; wait for it, so
        // the slot is released only once the session is really gone.
        drop(driver);
        pump.abort();
        let _ = pump.await;
        // A session still running after the grace period (stuck in a slow
        // store call) is aborted, so it never outlives the slot released below.
        if tokio::time::timeout(WRITE_TIMEOUT * 2, &mut session)
            .await
            .is_err()
        {
            session.abort();
            let _ = session.await;
        }
    }
    tracing::info!(principal = %principal, "ws: session closed");
    drop(conn);
}

/// Every message the session writes, as one string each, until it ends.
async fn pump(from_session: ReadHalf<DuplexStream>, out: mpsc::Sender<String>) {
    let mut reader = BufReader::new(from_session);
    while let Ok(Some(msg)) = mae_mcp::read_message(&mut reader).await {
        if out.send(msg).await.is_err() {
            break;
        }
    }
}

/// `auth_deadline` on the wire: RFC 3339, UTC, whole seconds.
fn rfc3339(unix: u64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(unix)).to_string()
}

fn hello(auth: &SessionAuth, settings: &WsSettings) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/hello",
        "params": {
            "protocol": PROTOCOL_V1,
            "principal": auth.principal,
            "auth_deadline": rfc3339(auth.deadline),
            "limits": {
                "max_frame_bytes": settings.max_frame_bytes,
                "max_open_docs": settings.max_open_docs,
            },
        },
    })
}

/// The tokio instant of a unix-seconds time, to the precision `SystemTime` has.
fn at_unix(unix: u64) -> tokio::time::Instant {
    let target = UNIX_EPOCH + Duration::from_secs(unix);
    let wait = target.duration_since(SystemTime::now()).unwrap_or_default();
    tokio::time::Instant::now() + wait
}

impl Driver {
    async fn drive(&mut self, mut out: mpsc::Receiver<String>) {
        // ADR-111 D6: a ping on a fixed period keeps a quiet session alive
        // through a proxy's idle timeout. The first tick is one period out.
        let period = self.ping_every.unwrap_or(Duration::from_secs(3600));
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let deadline = self.handle.deadline();
            let renew_at = at_unix(deadline.saturating_sub(self.renew_lead.as_secs()));
            let deadline_at = at_unix(deadline);
            let flow = tokio::select! {
                frame = self.ws.next() => self.on_frame(frame).await,
                body = out.recv() => match body {
                    Some(b) => self.send_text(b).await,
                    None => self.session_ended(),
                },
                _ = tokio::time::sleep_until(renew_at), if !self.renew_sent => {
                    self.renew_needed().await
                }
                _ = tokio::time::sleep_until(deadline_at) => {
                    Flow::Close(CLOSE_TOKEN_EXPIRED, "token_expired")
                }
                _ = ping.tick(), if self.ping_every.is_some() => self.send_ping().await,
            };
            match flow {
                Flow::Continue => {}
                Flow::Stop => return,
                Flow::Close(code, reason) => return self.close(code, reason).await,
            }
        }
    }

    /// The session loop ended on its own. If that was the deadline (the loop
    /// stops at it too, B3), say so with the deadline's close code.
    fn session_ended(&self) -> Flow {
        if now_unix() >= self.handle.deadline() {
            Flow::Close(CLOSE_TOKEN_EXPIRED, "token_expired")
        } else {
            Flow::Stop
        }
    }

    async fn on_frame(&mut self, frame: Option<Result<Message, tokio_websockets::Error>>) -> Flow {
        match frame {
            None => Flow::Stop,
            Some(Err(e)) => {
                // A frame over the cap arrives here, with its `1009` close
                // already queued by the codec: flush it out, then linger so
                // the client can read it.
                tracing::debug!(error = %e, "ws: read error");
                let _ = tokio::time::timeout(WRITE_TIMEOUT, self.ws.flush()).await;
                self.linger().await;
                Flow::Stop
            }
            Some(Ok(m)) if m.is_text() => {
                let text = m.as_text().unwrap_or_default().to_string();
                self.on_text(text).await
            }
            Some(Ok(m)) if m.is_binary() => {
                Flow::Close(CLOSE_UNSUPPORTED_DATA, "binary frames are not supported")
            }
            Some(Ok(m)) if m.is_close() => {
                let _ = tokio::time::timeout(WRITE_TIMEOUT, self.ws.flush()).await;
                Flow::Stop
            }
            Some(Ok(_)) => Flow::Continue,
        }
    }

    async fn on_text(&mut self, text: String) -> Flow {
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");
        if method.starts_with("session/") {
            let id = parsed.get("id").cloned().unwrap_or_default();
            let params = parsed.get("params").cloned().unwrap_or_default();
            return match method {
                "session/renew" => self.renew(id, &params).await,
                other => {
                    let msg = format!("unknown session method '{other}'");
                    self.send_error(id, McpError::method_not_found(msg)).await
                }
            };
        }
        // Everything else is the envelope: hand it to the session loop, which
        // authorizes it exactly as it would from any other transport.
        match mae_mcp::write_framed(&mut self.to_session, text.as_bytes(), WRITE_TIMEOUT).await {
            Ok(()) => Flow::Continue,
            Err(_) => Flow::Stop,
        }
    }

    async fn renew_needed(&mut self) -> Flow {
        self.renew_sent = true;
        let deadline = self.handle.deadline();
        self.send_json(json!({
            "jsonrpc": "2.0",
            "method": "session/renew_needed",
            "params": { "auth_deadline": rfc3339(deadline) },
        }))
        .await
    }

    /// B3: validate the new token, refuse a changed identity, move the
    /// deadline, and re-authorize every open document.
    async fn renew(&mut self, id: Value, params: &Value) -> Flow {
        let token = params.get("token").and_then(Value::as_str).unwrap_or("");
        let vp = match self.validate_renewal(token) {
            Ok(vp) => vp,
            Err(e) => return self.send_error(id, e).await,
        };
        // Issuer AND subject, as the token carries them — before any mapping.
        if vp.issuer != self.auth.issuer || vp.subject != self.auth.subject {
            tracing::warn!(principal = %self.auth.principal, "ws: renewal changed the principal; closing");
            return Flow::Close(CLOSE_PRINCIPAL_CHANGED, "principal_changed");
        }
        self.handle.set_deadline(vp.expires_at);
        self.renew_sent = false;
        let flow = self
            .send_json(
                json!({"jsonrpc": "2.0", "id": id, "result": {"auth_deadline": rfc3339(vp.expires_at)}}),
            )
            .await;
        self.handle.reauthorize().await;
        flow
    }

    /// The renewal token, validated by the same self-issued validator the
    /// upgrade reaches through `oauth::authenticate` (B6: nothing else).
    fn validate_renewal(&self, token: &str) -> Result<ValidatedPrincipal, McpError> {
        let kid = jsonwebtoken::decode_header(token).ok().and_then(|h| h.kid);
        let self_issue = self.listener.self_issue.as_ref();
        let si = match (kid.as_deref(), self_issue) {
            (Some(oauth_self_issue::SELF_ISSUED_KID), Some(si)) => si,
            _ => {
                return Err(McpError::issuer_not_supported(
                    "only this daemon's own tokens are accepted on this route (ADR-099 B6)".into(),
                ))
            }
        };
        let pubkey = si.identity.public().to_bytes();
        let vp = oauth_self_issue::validate_self_issued_token(token, &pubkey, &si.audience)
            .map_err(|e| McpError::invalid_token(format!("{e:?}")))?;
        if vp.expires_at <= now_unix() {
            return Err(McpError::invalid_token("Expired".into()));
        }
        Ok(vp)
    }

    async fn send_error(&mut self, id: Value, error: McpError) -> Flow {
        let resp = mae_mcp::protocol::JsonRpcResponse::error(id, error);
        self.send_json(serde_json::to_value(resp).unwrap_or_default())
            .await
    }

    async fn send_ping(&mut self) -> Flow {
        match tokio::time::timeout(WRITE_TIMEOUT, self.ws.send(Message::ping(Vec::new()))).await {
            Ok(Ok(())) => Flow::Continue,
            _ => Flow::Stop,
        }
    }

    async fn send_json(&mut self, value: Value) -> Flow {
        self.send_text(value.to_string()).await
    }

    async fn send_text(&mut self, body: String) -> Flow {
        match tokio::time::timeout(WRITE_TIMEOUT, self.ws.send(Message::text(body))).await {
            Ok(Ok(())) => Flow::Continue,
            _ => {
                tracing::warn!(principal = %self.auth.principal, "ws: client not reading; dropping");
                Flow::Stop
            }
        }
    }

    async fn close(&mut self, code: u16, reason: &'static str) {
        let code = CloseCode::try_from(code).ok();
        let send = self.ws.send(Message::close(code, reason));
        let _ = tokio::time::timeout(WRITE_TIMEOUT, send).await;
        let _ = tokio::time::timeout(WRITE_TIMEOUT, self.ws.flush()).await;
        self.linger().await;
    }

    /// After the daemon's own close frame: half-close, then read and discard
    /// what the client is still sending until it closes too, bounded by
    /// [`WRITE_TIMEOUT`] and `linger_bytes`. Closing a TCP connection with
    /// unread input makes the kernel answer with a reset, and a reset discards
    /// the close frame still in flight: the client would see an abnormal
    /// closure (1006) instead of the code it was sent — exactly what happens
    /// to a frame over the cap, whose unread remainder is the input.
    async fn linger(&mut self) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let budget = self.linger_bytes;
        let io = self.ws.get_mut();
        let drain = async {
            let _ = io.shutdown().await;
            let mut buf = vec![0u8; 16 * 1024];
            let mut seen = 0usize;
            while seen <= budget {
                match io.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen += n,
                }
            }
        };
        let _ = tokio::time::timeout(WRITE_TIMEOUT, drain).await;
    }
}
