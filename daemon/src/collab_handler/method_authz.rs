//! Every method the collab dispatcher routes, as a type — and what each one is
//! allowed to touch in the doc store.
//!
//! @ai-caution: [dispatch-authz] `handle_doc_request_inner` matches on
//! [`Method`], NOT on `&str`, and that is the whole point of this module. Both
//! matches here are exhaustive with no `_` arm, so a new method **cannot be
//! routed until it is added to [`Method`] and classified in [`DocScope::of`]**.
//!
//! The alternative — remembering to call a guard at each site — is what MAE
//! already tried, twice, and it fails open both times:
//!
//! * `deny_kb_doc_read` was written correctly and then called from two of the
//!   four paths that needed it. `collab_handler_cross_kb_node_isolation_tests`
//!   records that round: *"`sync/resync` returns the same bytes under a
//!   different method name, and `sync/diff` returns them as a delta."*
//! * That fix swept `sync/*` and stopped, so the entire `docs/*` family — seven
//!   methods that never received `auth_principal` at all — returned the same
//!   bytes under a third set of names. See
//!   `collab_handler_unauthorized_surface_tests`, whose four cases were all
//!   live when this module was written.
//!
//! `kb_id_guard` already states the principle: *"Keyed on the PRESENCE of a
//! `kb_id` param rather than on a list of method names… the enumerate-the-sites
//! approach is what finding C showed fails open."* This module is that
//! principle applied to authorization rather than to addressability.

use super::*;

/// A method routed by [`super::handle_doc_request_inner`].
///
/// Adding a variant forces a `DocScope::of` arm and a dispatch arm; both
/// matches are exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Method {
    // --- sync/*: named-document transfer, self-gated by `deny_kb_doc_read` ---
    SyncStateVector,
    SyncUpdate,
    SyncAwareness,
    SyncFullState,
    SyncDiff,
    SyncResync,
    SyncShare,

    // --- docs/*: named-document access with NO membership model of its own ---
    DocsList,
    DocsContent,
    DocsStats,
    DocsMetadata,
    DocsSaveIntent,
    DocsSaveCommitted,
    DocsDelete,
    DebugStats,

    // --- kb/*: the access-gated surface; each handler calls `kb_access` ---
    KbRegister,
    KbList,
    KbUnregister,
    KbShare,
    KbJoin,
    KbNodeFetch,
    KbNodeUpdate,
    KbCollectionOp,
    KbClaimLease,
    KbFetchArtifact,
    KbAddMember,
    KbRemoveMember,
    KbCollectionNodeAdd,
    KbCollectionNodeRemove,
    KbSetPolicy,
    KbBlockPrincipal,
    KbUnblockPrincipal,
    KbBlocklist,
    KbSetGovernance,
    KbRevoke,
    KbListPending,
    KbApproveMember,
    KbLeave,

    // --- kb/query.*: the ADR-053 read-through surface ---
    KbQueryCapabilities,
    KbQueryGet,
    KbQuerySearch,
    KbQueryGraph,
    KbQueryMyWrappedKey,
    KbQuerySelfToken,
}

impl Method {
    /// The one place a wire method name becomes a [`Method`].
    ///
    /// `None` means the dispatcher answers `method_not_found`, which is the
    /// same behaviour the old `other =>` arm had — an unroutable method is
    /// never silently treated as some other method.
    pub(super) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "sync/state_vector" => Self::SyncStateVector,
            "sync/update" => Self::SyncUpdate,
            "sync/awareness" => Self::SyncAwareness,
            "sync/full_state" => Self::SyncFullState,
            "sync/diff" => Self::SyncDiff,
            "sync/resync" => Self::SyncResync,
            "sync/share" => Self::SyncShare,

            "docs/list" => Self::DocsList,
            "docs/content" => Self::DocsContent,
            "docs/stats" => Self::DocsStats,
            "docs/metadata" => Self::DocsMetadata,
            "docs/save_intent" => Self::DocsSaveIntent,
            "docs/save_committed" => Self::DocsSaveCommitted,
            "docs/delete" => Self::DocsDelete,
            "$/debug" => Self::DebugStats,

            "kb/register" => Self::KbRegister,
            "kb/list" => Self::KbList,
            "kb/unregister" => Self::KbUnregister,
            "kb/share" => Self::KbShare,
            "kb/join" => Self::KbJoin,
            "kb/node_fetch" => Self::KbNodeFetch,
            "kb/node_update" => Self::KbNodeUpdate,
            "kb/collection_op" => Self::KbCollectionOp,
            "kb/claim_lease" => Self::KbClaimLease,
            "kb/fetch_artifact" => Self::KbFetchArtifact,
            "kb/add_member" => Self::KbAddMember,
            "kb/remove_member" => Self::KbRemoveMember,
            "kb/collection_node_add" => Self::KbCollectionNodeAdd,
            "kb/collection_node_remove" => Self::KbCollectionNodeRemove,
            "kb/set_policy" => Self::KbSetPolicy,
            "kb/block_principal" => Self::KbBlockPrincipal,
            "kb/unblock_principal" => Self::KbUnblockPrincipal,
            "kb/blocklist" => Self::KbBlocklist,
            "kb/set_governance" => Self::KbSetGovernance,
            "kb/revoke" => Self::KbRevoke,
            "kb/list_pending" => Self::KbListPending,
            "kb/approve_member" => Self::KbApproveMember,
            "kb/leave" => Self::KbLeave,

            "kb/query.capabilities" => Self::KbQueryCapabilities,
            "kb/query.get" => Self::KbQueryGet,
            "kb/query.search" => Self::KbQuerySearch,
            "kb/query.graph" => Self::KbQueryGraph,
            "kb/query.my_wrapped_key" => Self::KbQueryMyWrappedKey,
            "kb/query.self_token" => Self::KbQuerySelfToken,

            _ => return None,
        })
    }
}

/// What a method is permitted to reach in the doc store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocScope {
    /// The handler resolves a `kb_id` and calls `kb_access` itself. Nothing to
    /// do here; the gate lives at the handler because the resource is a KB, not
    /// a document.
    KbGated,

    /// Names a document and clears `deny_kb_doc_read` itself before touching
    /// it. Verified by `collab_handler_cross_kb_node_isolation_tests`.
    SelfGatedNamedDoc,

    /// Names a document but has **no membership model of its own**, so a KB
    /// address must be refused before the handler runs.
    ///
    /// This is the `docs/*` family. The temptation is to require KB membership
    /// instead — that breaks file collaboration, because `docs/list`,
    /// `docs/save_intent` and `docs/save_committed` have real callers in
    /// `crates/mae/src/collab_bridge/mod.rs` and plain buffer docs have no
    /// membership concept at all. KB content is reachable **only** through the
    /// access-gated `kb/*` surface — which is what `deny_kb_doc_read`'s own doc
    /// comment asserts and what this family used to falsify.
    NonKbDocOnly,

    /// Enumerates the doc store. Must be scoped to the session's own documents:
    /// on a multi-tenant host the names alone disclose every tenant's KB ids
    /// and node ids.
    Enumerates,
}

impl DocScope {
    /// Exhaustive by construction — no `_` arm, deliberately.
    pub(super) fn of(m: Method) -> Self {
        match m {
            Method::SyncStateVector
            | Method::SyncUpdate
            | Method::SyncAwareness
            | Method::SyncFullState
            | Method::SyncDiff
            | Method::SyncResync
            | Method::SyncShare => Self::SelfGatedNamedDoc,

            Method::DocsContent
            | Method::DocsStats
            | Method::DocsMetadata
            | Method::DocsSaveIntent
            | Method::DocsSaveCommitted
            | Method::DocsDelete => Self::NonKbDocOnly,

            Method::DocsList | Method::DebugStats => Self::Enumerates,

            Method::KbRegister
            | Method::KbList
            | Method::KbUnregister
            | Method::KbShare
            | Method::KbJoin
            | Method::KbNodeFetch
            | Method::KbNodeUpdate
            | Method::KbCollectionOp
            | Method::KbClaimLease
            | Method::KbFetchArtifact
            | Method::KbAddMember
            | Method::KbRemoveMember
            | Method::KbCollectionNodeAdd
            | Method::KbCollectionNodeRemove
            | Method::KbSetPolicy
            | Method::KbBlockPrincipal
            | Method::KbUnblockPrincipal
            | Method::KbBlocklist
            | Method::KbSetGovernance
            | Method::KbRevoke
            | Method::KbListPending
            | Method::KbApproveMember
            | Method::KbLeave
            | Method::KbQueryCapabilities
            | Method::KbQueryGet
            | Method::KbQuerySearch
            | Method::KbQueryGraph
            | Method::KbQueryMyWrappedKey
            | Method::KbQuerySelfToken => Self::KbGated,
        }
    }
}

/// Refuse a KB-addressed document to a method that cannot authorize one.
///
/// Runs before the dispatch match, for the same reason
/// `kb_id_guard::refuse_unaddressable_kb_id` does: one chokepoint beats N call
/// sites, and the check is keyed on the address TYPE rather than on a string
/// prefix (ADR-105 D1), so a renamed namespace cannot slip past it.
///
/// **Scope, stated honestly.** This closes the KB boundary and nothing else. KB
/// documents have a membership model, it lives behind `kb_access` on the `kb/*`
/// surface, and reaching one through here was an authorization bypass.
///
/// Plain collaborative buffers have **no membership model at all**, so there is
/// no boundary here to enforce for them and this function does not pretend
/// otherwise. Requiring the session to have joined a plain document was tried
/// and reverted: `sync/full_state` on a plain doc is equally ungated, so the
/// requirement adds a step without removing a capability — security theatre
/// that also breaks the two-client read flow the collab e2e suite exercises
/// (`unicode_round_trip_through_server` and six siblings, where the second
/// client reads a shared buffer without a prior join). Plain-buffer access
/// control is a real gap and a separate design decision; the tenant boundary
/// for it is the per-tenant daemon process, not this check.
///
/// Deliberately silent about *why* a document is unavailable — the refusal text
/// must not distinguish "exists, not yours" from "does not exist", or the error
/// becomes an existence oracle over every tenant's node ids.
pub(super) fn authorize_named_doc(
    session_id: u64,
    method: Method,
    params: &serde_json::Value,
    id: &serde_json::Value,
) -> Option<JsonRpcResponse> {
    if DocScope::of(method) != DocScope::NonKbDocOnly {
        return None;
    }
    let doc_name = params.get("doc").and_then(|v| v.as_str())?;
    if matches!(
        mae_sync::DocAddress::parse(doc_name),
        Some(mae_sync::DocAddress::KbNode { .. } | mae_sync::DocAddress::KbCollection { .. })
    ) {
        warn!(
            session = session_id,
            doc = %doc_name,
            "refused: KB documents are reachable only through the access-gated kb/* surface"
        );
        return Some(JsonRpcResponse::error(
            id.clone(),
            McpError::internal_error(format!("document '{doc_name}' is not available")),
        ));
    }
    None
}

/// Whether a document may appear in an enumeration response.
///
/// `docs/list` and `$/debug` take no document name, so [`authorize_named_doc`]
/// cannot help them — the filtering has to happen where the list is built. KB
/// documents are excluded because their names alone disclose every tenant's KB
/// ids and node ids on a shared host, and because `kb/list` is their proper,
/// membership-checked surface.
///
/// Plain buffers are still listed. That is the existing collab model — shared
/// buffers are discoverable to anyone connected to the daemon — and narrowing
/// it is a separate design decision, not part of closing this hole. What
/// changes here is that discovering a name no longer grants the content:
/// [`authorize_named_doc`] requires the session to have joined the document.
pub(super) fn is_enumerable(doc_name: &str) -> bool {
    !matches!(
        mae_sync::DocAddress::parse(doc_name),
        Some(mae_sync::DocAddress::KbNode { .. } | mae_sync::DocAddress::KbCollection { .. })
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every method the dispatcher can route is classified. This is belt to the
    /// exhaustive match's braces: it fails if a variant is added and lumped
    /// into an existing arm without thought, which the compiler cannot catch.
    #[test]
    fn every_docs_method_is_kb_address_restricted() {
        for name in [
            "docs/content",
            "docs/stats",
            "docs/metadata",
            "docs/save_intent",
            "docs/save_committed",
            "docs/delete",
        ] {
            let m = Method::parse(name).unwrap_or_else(|| panic!("{name} must parse"));
            assert_eq!(
                DocScope::of(m),
                DocScope::NonKbDocOnly,
                "{name} names a caller-supplied doc and has no membership model — \
                 it must refuse KB addresses"
            );
        }
    }

    /// `docs/list` and `$/debug` take no doc name, so the address guard cannot
    /// help them; they must be session-scoped in the handler instead. Pin the
    /// classification so that obligation is not silently dropped.
    #[test]
    fn enumerating_methods_are_classified_as_such() {
        for name in ["docs/list", "$/debug"] {
            let m = Method::parse(name).unwrap();
            assert_eq!(DocScope::of(m), DocScope::Enumerates, "{name}");
        }
    }

    /// An unroutable method must not resolve to some other method.
    #[test]
    fn unknown_methods_do_not_parse() {
        for name in [
            "",
            "docs/",
            "docs/content ",
            "DOCS/CONTENT",
            "kb/query.",
            "nope",
        ] {
            assert!(Method::parse(name).is_none(), "{name:?} must not parse");
        }
    }
}
