//! ADR-116 D4: verifying a KB's whole history, one implementation for the
//! `kb/history.verify` method and the operator's `mae-daemon kb history
//! verify`, so the two cannot disagree.
//!
//! Every node's chain is checked (with its content, under the KB's history
//! key when the daemon holds it), and the chains are checked against the
//! latest signed head and, when given, a head kept outside the daemon. A head
//! counts only if it is signed with this daemon's key: one that verifies
//! under any other key was not written here.

use std::collections::BTreeMap;

use mae_sync::kb::history::{
    kb_head, public_key_of, verify_against_head, verify_chain, Hash, HeadError, HistoryEntry,
    SignedHead, VerifyError,
};
use serde_json::{json, Value};

use crate::doc_store::DocStore;
use crate::storage::StorageError;

/// How a KB's chains stand against one head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadCheck {
    /// There is no head to check against, and why.
    Unchecked(&'static str),
    Ok {
        at: i64,
        digest: Hash,
    },
    Failed {
        at: i64,
        digest: Hash,
        node_id: Option<String>,
        seq: Option<u64>,
        broken: &'static str,
        reason: String,
    },
}

impl HeadCheck {
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    pub fn json(&self) -> Value {
        match self {
            Self::Unchecked(why) => json!({ "checked": false, "reason": why }),
            Self::Ok { at, digest } => json!({
                "checked": true, "ok": true,
                "head": { "at": at, "digest": hex::encode(digest) },
            }),
            Self::Failed {
                at,
                digest,
                node_id,
                seq,
                broken,
                reason,
            } => json!({
                "checked": true, "ok": false,
                "head": { "at": at, "digest": hex::encode(digest) },
                "broken": broken, "node_id": node_id, "seq": seq, "reason": reason,
            }),
        }
    }
}

/// A whole KB's verification.
#[derive(Debug)]
pub struct KbReport {
    pub kb_id: String,
    pub nodes: usize,
    pub versions: usize,
    /// Each node whose chain fails on its own, with where.
    pub failures: Vec<(String, VerifyError)>,
    /// The digest of the KB's current tips.
    pub head: Hash,
    /// Against the latest head in the daemon's log.
    pub signed: HeadCheck,
    /// Against a head kept outside the daemon, when one is given.
    pub against: Option<HeadCheck>,
}

impl KbReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
            && !self.signed.is_failure()
            && !self.against.as_ref().is_some_and(HeadCheck::is_failure)
    }
}

/// Check `head` against `chains`, as this daemon (`own`, its public key)
/// signed it.
fn check(
    head: &SignedHead,
    own: Option<[u8; 32]>,
    kb_id: &str,
    chains: &BTreeMap<String, Vec<HistoryEntry>>,
    key: Option<&mae_sync::kb::history::HistoryKey>,
) -> HeadCheck {
    let (at, digest) = (head.at, head.digest);
    let failed = |node_id, seq, broken, reason: &str| HeadCheck::Failed {
        at,
        digest,
        node_id,
        seq,
        broken,
        reason: reason.to_string(),
    };
    if own != Some(head.signer) {
        return failed(
            None,
            None,
            "head",
            "the head is not signed with this daemon's key",
        );
    }
    match verify_against_head(head, kb_id, chains, key) {
        Ok(()) => HeadCheck::Ok { at, digest },
        Err(e) => {
            let broken = e.broken().as_str();
            let reason = match &e {
                HeadError::Signature => "the head's signature does not verify",
                HeadError::Kb => "the head is another KB's",
                HeadError::Chain { .. } => "a chain fails on its own",
                HeadError::Tip { .. } => "a version the head names is gone or different",
            };
            let (node_id, seq) = match e {
                HeadError::Tip { node_id, seq } => (Some(node_id), Some(seq)),
                HeadError::Chain { node_id, err } => (Some(node_id), Some(err.seq)),
                _ => (None, None),
            };
            failed(node_id, seq, broken, reason)
        }
    }
}

/// Verify every node of `kb_id` and its heads.
pub async fn verify_kb(
    doc_store: &DocStore,
    kb_id: &str,
    against: Option<&SignedHead>,
) -> Result<KbReport, StorageError> {
    let key = doc_store.history_key(kb_id).await?;
    let mut chains = BTreeMap::new();
    let (mut versions, mut failures) = (0, Vec::new());
    for node in doc_store.history_nodes(kb_id).await? {
        let h = doc_store.history_list(kb_id, &node).await?;
        versions += h.len();
        if let Err(e) = verify_chain(kb_id, &node, &h, None, key.as_ref()) {
            failures.push((node.clone(), e));
        }
        chains.insert(node, h);
    }
    let tips = chains
        .iter()
        .filter_map(|(n, h)| h.last().map(|t| (n.as_str(), t.seq, t.entry_hash)));
    let head = kb_head(kb_id, tips).map_err(|e| StorageError::Sqlite(format!("head: {e}")))?;
    let own = doc_store.signer().map(|s| public_key_of(&s.secret_bytes()));
    let signed = match doc_store.history_latest_head(kb_id).await? {
        None => HeadCheck::Unchecked("no signed head yet"),
        Some(h) => check(&h, own, kb_id, &chains, key.as_ref()),
    };
    let against = against.map(|h| check(h, own, kb_id, &chains, key.as_ref()));
    Ok(KbReport {
        kb_id: kb_id.to_string(),
        nodes: chains.len(),
        versions,
        failures,
        head,
        signed,
        against,
    })
}
