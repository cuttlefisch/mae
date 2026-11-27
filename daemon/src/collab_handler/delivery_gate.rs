//! Every KB document event a session receives is authorized when it is
//! delivered, not only when the session subscribed.
//!
//! A subscription is gated once, by the method that opens the document. Without
//! a check at delivery, a principal who lost read access afterwards kept
//! receiving the KB's changes on every session already open, until it
//! disconnected. Access can be lost in many ways: a member removed by the
//! owner, the operator or a signed op through `kb/collection_op`, a quorum
//! `kb/revoke`, a removal synced from a peer, a block, a rebind, a timeboxed
//! grant lapsing. Every one of them ends here, because this gate asks the one
//! access engine (`kb_access`) rather than tracking which method changed what.
//!
//! **Cost.** A decision is kept per session and KB until the KB's access
//! generation moves (`DocStore::kb_access_generation`: its collection document,
//! anchor or blocklist changed) or its membership's timebox horizon passes
//! (`DocStore::membership_valid_until`). A node edit moves neither, so typing
//! in a KB costs one map lookup per delivered event, not one access decision.
//!
//! **On a refusal** the session's documents of that KB are closed exactly as
//! renewal re-authorization closes them (`governor::reauthorize_open_docs`,
//! which shares these helpers): unsubscribed, released, and reported to the
//! client with `session/doc_closed` (`not_member`). The event is not delivered.
//!
//! @ai-caution: [authz] Never deliver a KB document event without
//! [`DeliveryGate::admits`], and never memoize a decision past a change to an
//! input of `kb_access` that the access generation does not cover. If
//! `kb_access` gains an input, move the generation when it changes.

use std::collections::{HashMap, HashSet};

use mae_mcp::broadcast::SharedBroadcaster;
use mae_sync::kb::Transport;

use super::{kb_access, now_unix, AccessDecision, KbOp};
use crate::doc_store::DocStore;

/// The KB a document belongs to: a collection or one of its nodes.
pub(crate) fn kb_of(doc: &str) -> Option<String> {
    match mae_sync::DocAddress::parse(doc) {
        Some(mae_sync::DocAddress::KbCollection { kb_id })
        | Some(mae_sync::DocAddress::KbNode { kb_id, .. }) => Some(kb_id),
        _ => None,
    }
}

/// Whether `principal` may read KB `kb_id` now. A decision that errors (the
/// collection is gone or unreadable) is a refusal: fail closed.
pub(crate) async fn may_read(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    transport: Transport,
) -> bool {
    matches!(
        kb_access(doc_store, kb_id, principal, KbOp::Read, transport).await,
        Ok(AccessDecision::Allow)
    )
}

/// Close `docs` for session `session_id`: unsubscribed from the broadcaster,
/// and, for each the session counted as open, forgotten and released.
pub(crate) async fn close_docs(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    docs: &[String],
) {
    for doc in docs {
        broadcaster
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unsubscribe_doc(session_id, doc);
        if session_docs.remove(doc) {
            let _ = doc_store.track_client_disconnect(doc).await;
        }
    }
}

/// One session's delivery-time access check.
pub(crate) struct DeliveryGate {
    principal: Option<String>,
    transport: Transport,
    /// KB → the access generation and timebox horizon its allow was decided
    /// under. Only allows are kept: a refusal closes the KB's documents.
    admitted: HashMap<String, (u64, u64)>,
    /// Documents this gate has closed and reported. An event already queued
    /// for one when it closed is dropped without a second notice.
    closed: HashSet<String>,
}

impl DeliveryGate {
    pub(crate) fn new(principal: Option<&str>, transport: Transport) -> Self {
        Self {
            principal: principal.map(str::to_string),
            transport,
            admitted: HashMap::new(),
            closed: HashSet::new(),
        }
    }

    /// Whether this session's principal may still read KB `kb_id`.
    pub(crate) async fn admits(&mut self, doc_store: &DocStore, kb_id: &str) -> bool {
        // Taken BEFORE the decision, so a change landing during it moves the
        // generation past the one recorded and the next event decides again.
        let generation = doc_store.kb_access_generation(kb_id);
        if let Some(&(g, until)) = self.admitted.get(kb_id) {
            if g == generation && now_unix() < until {
                return true;
            }
        }
        self.admitted.remove(kb_id);
        let principal = self.principal.as_deref();
        if !may_read(doc_store, kb_id, principal, self.transport).await {
            return false;
        }
        let until = doc_store.membership_valid_until(kb_id).await;
        self.admitted.insert(kb_id.to_string(), (generation, until));
        // Readable again (re-admitted): a later refusal reports afresh.
        self.closed.retain(|d| kb_of(d).as_deref() != Some(kb_id));
        true
    }

    /// The documents to close and report when KB `kb_id` is refused: every
    /// document of it the session holds, and `doc`, the one the refused event
    /// named, unless this gate has already closed and reported it.
    pub(crate) fn closing(
        &mut self,
        session_docs: &HashSet<String>,
        kb_id: &str,
        doc: &str,
    ) -> Vec<String> {
        let mut docs: Vec<String> = session_docs
            .iter()
            .filter(|d| kb_of(d).as_deref() == Some(kb_id))
            .cloned()
            .collect();
        if !self.closed.contains(doc) && !docs.iter().any(|d| d == doc) {
            docs.push(doc.to_string());
        }
        docs.sort();
        self.closed.extend(docs.iter().cloned());
        docs
    }
}
