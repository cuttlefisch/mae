//! What a transport imposes on a collab session beyond its identity
//! (ADR-099 Amendment 1, B3/B4): an authentication deadline, a cap on open
//! documents, and re-authorization of every open document on demand.
//!
//! The WebSocket route on the HTTPS listener is the first transport that needs
//! these, and it reaches the SAME session loop and dispatch every other
//! transport uses (`run_session`). That is the point: every `sync/*` and `kb/*`
//! method keeps its one authorization path, so per-frame authorization is
//! inherited rather than re-implemented. This module adds only what the shared
//! loop cannot know on its own — when the credential behind the session
//! expires, and how many documents the transport lets one session hold. A
//! session with no governor (mTLS, PSK, the P2P mesh) behaves exactly as before.
//!
//! @ai-caution: [authz] [`SessionGovernor::admit`] runs BEFORE dispatch and the
//! deadline check there is the authoritative "no frame is applied after the
//! deadline" (B3). The transport's own timer only closes the socket; frames
//! already queued inside the session when it fires are stopped here, not there.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mae_mcp::broadcast::SharedBroadcaster;
use mae_mcp::protocol::{JsonRpcResponse, McpError};
use mae_sync::kb::Transport;
use tokio::sync::mpsc;

use super::{kb_access, AccessDecision, KbOp};
use crate::doc_store::DocStore;

/// A request from the transport to the session loop it governs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControl {
    /// The session's credential was renewed: re-authorize every open document
    /// and close, per document, any the principal can no longer read (B3).
    Reauthorize,
}

/// The session-side half: owned by `run_session`.
pub struct SessionGovernor {
    max_open_docs: usize,
    deadline: Arc<AtomicU64>,
    control: mpsc::Receiver<SessionControl>,
}

/// The transport-side half: moves the deadline and asks for re-authorization.
#[derive(Clone)]
pub struct GovernorHandle {
    deadline: Arc<AtomicU64>,
    control: mpsc::Sender<SessionControl>,
}

/// Build a governor pair. `deadline_unix` is the credential's expiry (unix
/// seconds); `max_open_docs` of 0 means unlimited.
pub fn governor(max_open_docs: usize, deadline_unix: u64) -> (SessionGovernor, GovernorHandle) {
    let deadline = Arc::new(AtomicU64::new(deadline_unix));
    let (tx, rx) = mpsc::channel(4);
    (
        SessionGovernor {
            max_open_docs,
            deadline: Arc::clone(&deadline),
            control: rx,
        },
        GovernorHandle {
            deadline,
            control: tx,
        },
    )
}

impl GovernorHandle {
    /// Move the deadline (a successful renewal, B3).
    pub fn set_deadline(&self, deadline_unix: u64) {
        self.deadline.store(deadline_unix, Ordering::SeqCst);
    }

    /// The current deadline, unix seconds.
    pub fn deadline(&self) -> u64 {
        self.deadline.load(Ordering::SeqCst)
    }

    /// Ask the session to re-authorize its open documents. `false` when the
    /// session has already ended.
    pub async fn reauthorize(&self) -> bool {
        self.control.send(SessionControl::Reauthorize).await.is_ok()
    }
}

/// What the session loop does with one inbound message.
pub(crate) enum Admission {
    /// Dispatch it as usual.
    Dispatch,
    /// Do not dispatch; answer with this (requests only — a notification gets
    /// nothing).
    Refuse(Box<JsonRpcResponse>),
    /// The credential has expired: apply nothing, end the session.
    Expired,
}

impl SessionGovernor {
    fn expired(&self, now_unix: u64) -> bool {
        now_unix >= self.deadline.load(Ordering::SeqCst)
    }

    /// Decide whether `msg` may be dispatched (B3's deadline, B4's cap).
    ///
    /// The cap counts documents the session already holds (`open`). A request
    /// that names a document it does not hold is refused once it holds
    /// `max_open_docs`, before anything runs — so the refusal is independent of
    /// the named document's KB and discloses nothing about it.
    pub(crate) fn admit(&self, msg: &str, open: &HashSet<String>, now_unix: u64) -> Admission {
        if self.expired(now_unix) {
            return Admission::Expired;
        }
        if self.max_open_docs == 0 || open.len() < self.max_open_docs {
            return Admission::Dispatch;
        }
        let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(msg)
        else {
            return Admission::Dispatch;
        };
        // `kb/query.*` reads through the store and subscribes the session to
        // nothing, so it never counts against the cap.
        let method = obj.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method.starts_with("kb/query.") {
            return Admission::Dispatch;
        }
        let params = obj.get("params").cloned().unwrap_or_default();
        match requested_doc(&params) {
            Some(doc) if !open.contains(&doc) => {
                let id = obj.get("id").cloned().unwrap_or_default();
                Admission::Refuse(Box::new(JsonRpcResponse::error(
                    id,
                    McpError::too_many_open_docs(
                        format!(
                            "this session already holds its {} open documents",
                            self.max_open_docs
                        ),
                        self.max_open_docs,
                    ),
                )))
            }
            _ => Admission::Dispatch,
        }
    }

    /// The next control request, or never when the transport is gone.
    pub(crate) async fn next_control(gov: &mut Option<SessionGovernor>) -> SessionControl {
        match gov {
            Some(g) => match g.control.recv().await {
                Some(c) => c,
                None => std::future::pending().await,
            },
            None => std::future::pending().await,
        }
    }
}

/// The document a request names, from the envelope's own addressing (ADR-099
/// D2): `params.doc` on `sync/*`/`docs/*`, a node on `kb/*` with `node_id`, the
/// collection on any other `kb/*` with a `kb_id`. `None` names no document.
fn requested_doc(params: &serde_json::Value) -> Option<String> {
    if let Some(doc) = params.get("doc").and_then(|d| d.as_str()) {
        return Some(doc.to_string());
    }
    let kb_id = params.get("kb_id").and_then(|k| k.as_str())?;
    Some(match params.get("node_id").and_then(|n| n.as_str()) {
        Some(node_id) => mae_sync::kb_node_doc_name(kb_id, node_id),
        None => mae_sync::DocAddress::KbCollection {
            kb_id: kb_id.to_string(),
        }
        .to_doc_name(),
    })
}

/// Re-authorize every open KB document for `principal` (B3), closing each one
/// it can no longer read: unsubscribed, dropped from `session_docs`, and its
/// connection count released. Returns the closed `(doc, kb_id)` pairs for the
/// caller to report. Non-KB documents carry no KB gate and are kept.
///
/// One `kb_access` decision per KB, not per document — a joined KB holds a
/// document per node. A decision that errors (the collection is gone or
/// unreadable) closes the KB's documents: fail closed.
pub(crate) async fn reauthorize_open_docs(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    (session_id, principal, transport): (u64, Option<&str>, Transport),
    session_docs: &mut HashSet<String>,
) -> Vec<(String, String)> {
    let mut decided: HashMap<String, bool> = HashMap::new();
    let mut lost = Vec::new();
    for doc in session_docs.iter() {
        let kb_id = match mae_sync::DocAddress::parse(doc) {
            Some(mae_sync::DocAddress::KbCollection { kb_id })
            | Some(mae_sync::DocAddress::KbNode { kb_id, .. }) => kb_id,
            _ => continue,
        };
        if !decided.contains_key(&kb_id) {
            let allowed = matches!(
                kb_access(doc_store, &kb_id, principal, KbOp::Read, transport).await,
                Ok(AccessDecision::Allow)
            );
            decided.insert(kb_id.clone(), allowed);
        }
        if !decided[&kb_id] {
            lost.push((doc.clone(), kb_id));
        }
    }
    for (doc, _) in &lost {
        session_docs.remove(doc);
        broadcaster
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unsubscribe_doc(session_id, doc);
        let _ = doc_store.track_client_disconnect(doc).await;
    }
    lost
}

/// The notification closing one document for a session (B3/B4).
/// `doc` is the envelope's document name; `kb_id` is present for a KB
/// document and `node_id` for a node document, read from the address itself.
fn doc_closed(doc: &str, reason: &str) -> serde_json::Value {
    let mut params = serde_json::json!({ "doc": doc, "reason": reason });
    match mae_sync::DocAddress::parse(doc) {
        Some(mae_sync::DocAddress::KbNode { kb_id, node_id }) => {
            params["kb_id"] = kb_id.into();
            params["node_id"] = node_id.into();
        }
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => params["kb_id"] = kb_id.into(),
        _ => {}
    }
    serde_json::json!({ "jsonrpc": "2.0", "method": "session/doc_closed", "params": params })
}

/// Serialize `value` and write it as one framed message. `Err` means the
/// client is gone and the session should end.
pub(crate) async fn write_json<W, T>(
    writer: &mut W,
    value: &T,
    timeout: std::time::Duration,
) -> Result<(), ()>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let body = serde_json::to_vec(value).map_err(|_| ())?;
    mae_mcp::write_framed(writer, &body, timeout)
        .await
        .map_err(|_| ())
}

/// Handle one [`SessionControl`] inside the session loop: re-authorize and
/// tell the client, per document, which it lost (`not_member`, B3). `Err`
/// means the client is gone.
pub(crate) async fn apply_control<W: tokio::io::AsyncWrite + Unpin>(
    control: SessionControl,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    who: (u64, Option<&str>, Transport),
    session_docs: &mut HashSet<String>,
    (writer, timeout): (&mut W, std::time::Duration),
) -> Result<(), ()> {
    match control {
        SessionControl::Reauthorize => {
            let lost = reauthorize_open_docs(doc_store, broadcaster, who, session_docs).await;
            for (doc, kb_id) in &lost {
                tracing::info!(session = who.0, doc = %doc, kb_id = %kb_id,
                    "re-authorization: document closed (not_member)");
                write_json(writer, &doc_closed(doc, "not_member"), timeout).await?;
            }
            Ok(())
        }
    }
}
