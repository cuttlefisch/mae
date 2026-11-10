//! A typed refusal: what the access engine and the write gates return, so the
//! handler that turns one into a JSON-RPC error keeps the REASON as well as the
//! message.
//!
//! Before this, every gate returned `String` and every handler wrapped it in
//! `McpError::internal_error`, so a client could tell "not a member" from "stale
//! epoch" only by parsing English. The message text is unchanged — clients log
//! it — and `kind` rides alongside it into `error.data.kind` (see
//! `mae_mcp::protocol::ERROR_KINDS` and `docs/DAEMON_ADMIN.md`).
//!
//! `Deref<Target = str>`, `Display` and `From<Refusal> for String` keep every
//! caller that only formats or logs the reason compiling unchanged — including
//! the dialer in the binary crate, which logs relay rejections.

use super::*;

/// Why a request was refused. Maps one-to-one onto a `data.kind` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalKind {
    NotMember,
    /// A member whose role is below `required`.
    RoleForbidden {
        required: SyncRole,
    },
    /// The KB's transport policy does not expose it on this transport.
    TransportForbidden {
        transport: Transport,
    },
    /// ADR-067: a `QueryOnly` member asked to replicate.
    ReplicationRestricted,
    KbNotFound,
    CollectionUnavailable,
    NodeNotInKb,
    /// ADR-023. `caller_epoch` is known only when the op names its epoch (a
    /// signed content op); a bare CRDT update carries only a client id.
    EpochFenced {
        current_epoch: u64,
        caller_epoch: Option<u64>,
    },
    SignatureInvalid,
    SignatureRequired,
    RebirthUnverified,
    /// A KB document named on a raw path that cannot authorize one.
    DocUnavailable,
    /// A node document whose address names a different KB from the request's
    /// `kb_id`. Both come from the request, so this reveals nothing.
    DocKbMismatch,
    /// A caller-supplied parameter is malformed (`param` names it).
    InvalidParams {
        param: &'static str,
    },
    /// The daemon failed; not a decision about the caller.
    Internal,
}

/// A refusal reason plus the human message that has always accompanied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub message: String,
}

impl Refusal {
    pub fn new(kind: RefusalKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_member(message: impl Into<String>) -> Self {
        Self::new(RefusalKind::NotMember, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(RefusalKind::Internal, message)
    }

    /// The same reason with a different message — for a caller that adds
    /// context to a refusal it passes on.
    pub fn with_message(self, message: impl Into<String>) -> Self {
        Self::new(self.kind, message)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::ops::Deref for Refusal {
    type Target = str;
    fn deref(&self) -> &str {
        &self.message
    }
}

impl From<Refusal> for String {
    fn from(r: Refusal) -> String {
        r.message
    }
}

fn transport_name(t: Transport) -> &'static str {
    match t {
        Transport::Hub => "hub",
        Transport::P2p => "p2p",
    }
}

impl From<Refusal> for McpError {
    fn from(r: Refusal) -> McpError {
        let m = r.message;
        match r.kind {
            RefusalKind::NotMember => McpError::not_member(m),
            RefusalKind::RoleForbidden { required } => {
                McpError::role_forbidden(m, Some(required.as_str()))
            }
            RefusalKind::TransportForbidden { transport } => {
                McpError::transport_forbidden(m, transport_name(transport))
            }
            RefusalKind::ReplicationRestricted => McpError::replication_restricted(m),
            RefusalKind::KbNotFound => McpError::kb_not_found(m),
            RefusalKind::CollectionUnavailable => McpError::collection_unavailable(m),
            RefusalKind::NodeNotInKb => McpError::node_not_in_kb(m),
            RefusalKind::EpochFenced {
                current_epoch,
                caller_epoch,
            } => McpError::epoch_fenced(m, current_epoch, caller_epoch),
            RefusalKind::SignatureInvalid => McpError::signature_invalid(m),
            RefusalKind::SignatureRequired => McpError::signature_required(m),
            RefusalKind::RebirthUnverified => McpError::rebirth_unverified(m),
            RefusalKind::DocUnavailable => McpError::doc_unavailable(m),
            RefusalKind::DocKbMismatch => McpError::doc_kb_mismatch(m),
            RefusalKind::InvalidParams { param } => McpError::invalid_params(param, m),
            RefusalKind::Internal => McpError::internal_fault(m),
        }
    }
}

/// Require `Allow` from an access decision. `AllowAutoJoin` / `Pending` exist
/// only for `KbOp::Join` — the gate never returns them for another op — so on
/// any other op they are refused as the non-member outcomes they are, with the
/// message the call site has always used.
pub(crate) fn require_allow(
    decision: Result<AccessDecision, Refusal>,
    fallback: impl FnOnce() -> String,
) -> Result<(), Refusal> {
    match decision {
        Ok(AccessDecision::Allow) => Ok(()),
        Ok(AccessDecision::Deny(r)) | Err(r) => Err(r),
        Ok(AccessDecision::AllowAutoJoin | AccessDecision::Pending) => {
            Err(Refusal::not_member(fallback()))
        }
    }
}

/// The storage-error → JSON-RPC mapping for a handler that has nothing to
/// add: a size refusal is `quota_exceeded`, a missing durable doc is
/// `doc_not_found`, anything else `internal`. The message is the error's own.
pub(crate) fn storage_error(e: crate::storage::StorageError) -> McpError {
    match e {
        crate::storage::StorageError::TooLarge { size, limit, .. } => {
            McpError::document_too_large(e.to_string(), size, limit)
        }
        crate::storage::StorageError::DurableDocMissing(_) => {
            McpError::doc_not_found(e.to_string())
        }
        e => McpError::internal_fault(e.to_string()),
    }
}
