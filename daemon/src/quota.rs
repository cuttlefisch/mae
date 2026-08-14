//! Per-request quota seam for the collab/OAuth listeners (ADR-060 Phase C, #456).
//!
//! ## Why this is a trait and not a direct call
//!
//! The quota mechanism itself — `TenantRegistry`, the cost-weighted points window,
//! the tenant-scoped connection cap — lives in the **binary** crate (`main.rs`'s
//! `mod tenant` / `mod conn_limit`), because it is built from `daemon.toml` config
//! types that also live there. `collab_handler` lives in the **library** crate, and
//! a library cannot reach into the binary that links it.
//!
//! `tenant.rs` originally recorded that follow-on wiring "only needs a call site,
//! not a signature change". That is not true across this crate boundary: the
//! listener needs a seam it can be *handed* an implementation through.
//!
//! So this mirrors [`crate::artifact_store::ArtifactStore`] exactly — the same
//! problem already solved the same way: a narrow trait in the library, an
//! implementation in the binary, and a no-op default for callers that have no
//! quota to enforce.

/// A tenant's claim on a connection slot for the duration of one request.
///
/// Type-erased on purpose: the real guard is the binary crate's `ConnGuard`, which
/// this crate cannot name. All the library needs is that *something* is released
/// when the value drops, which `Drop` gives it regardless of the concrete type.
#[derive(Default)]
pub struct QuotaLease(
    // Never read, and must not be: the value exists solely so its `Drop` runs when
    // the lease goes out of scope at the end of the request. Reading it would mean
    // naming the binary crate's `ConnGuard`, which is the thing this seam exists to
    // avoid.
    #[allow(dead_code)] Option<Box<dyn Send>>,
);

impl QuotaLease {
    /// Wrap a guard whose `Drop` releases the slot.
    pub fn held(guard: Box<dyn Send>) -> Self {
        Self(Some(guard))
    }

    /// A lease that holds nothing — an admitted request against an unconfigured
    /// tenant, which takes no slot.
    pub fn none() -> Self {
        Self(None)
    }
}

/// Charges one collab/OAuth request against its tenant's budget.
pub trait QuotaCharger: Send + Sync {
    /// `Ok` admits the request and carries the lease to hold for its duration;
    /// `Err` carries the message to return to the caller.
    ///
    /// A principal resolving to no configured tenant MUST be admitted — that is
    /// ADR-060 Phase A's zero-config-zero-behaviour-change contract, and it is what
    /// keeps a single-user daemon unaffected by any of this.
    fn charge(&self, principal: Option<&str>, method: &str) -> Result<QuotaLease, String>;
}

/// Admits everything. The default for surfaces with no tenant configuration, and
/// for tests that are not exercising quotas — the counterpart of
/// [`crate::artifact_store::NoArtifactStore`].
pub struct NoQuota;

impl QuotaCharger for NoQuota {
    fn charge(&self, _principal: Option<&str>, _method: &str) -> Result<QuotaLease, String> {
        Ok(QuotaLease::none())
    }
}

/// Charge a request, shaping a refusal into the JSON-RPC error the caller should
/// return. Both listeners need exactly this, so it lives here once rather than
/// being spelled out at each door (CLAUDE.md principle #8).
///
/// `Ok` carries the lease to hold for the request's duration.
pub fn charge_or_reject(
    quota: &dyn QuotaCharger,
    principal: Option<&str>,
    method: &str,
    id: serde_json::Value,
) -> Result<QuotaLease, mae_mcp::protocol::JsonRpcResponse> {
    quota.charge(principal, method).map_err(|msg| {
        mae_mcp::protocol::JsonRpcResponse::error(
            id,
            mae_mcp::protocol::McpError::internal_error(msg),
        )
    })
}
