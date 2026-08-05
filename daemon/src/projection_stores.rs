//! The production [`ProjectionStores`] implementation (ADR-029).
//!
//! Bridges `projector`'s per-KB store lookup to this binary crate's `DaemonState` +
//! [`crate::handler::resolve_kb_store`] — the same crate-boundary pattern
//! `collab_handler::kb_lease::DaemonLeaseFence` and `artifact_store` already use.
//!
//! Deliberately holds **no store map of its own**. `DaemonState.instance_stores` plus
//! the registry are already the single source of truth for "which Cozo store backs this
//! KB"; a second copy here would be a cache that silently goes stale the moment an
//! instance is registered, unregistered, or re-opened (principle #8).

use std::sync::Arc;

use mae_kb::CozoKbStore;
use tokio::sync::Mutex;

use mae_daemon::projector::ProjectionStores;

use crate::handler::{resolve_kb_store, DaemonState};

/// Resolves a `kb_id` to its Cozo projection store through `DaemonState`.
pub struct DaemonProjectionStores {
    state: Arc<Mutex<DaemonState>>,
}

impl DaemonProjectionStores {
    pub fn new(state: Arc<Mutex<DaemonState>>) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl ProjectionStores for DaemonProjectionStores {
    /// @ai-caution: [daemon-locking] Take the lock, clone the `Arc`, drop it — ADR-054's
    /// snapshot-then-drop idiom. The returned store is used for a synchronous CozoDB
    /// call by the caller; holding `DaemonState` across that call would serialise every
    /// projection behind every other daemon request, which is the exact contention
    /// ADR-054 removed from the query path.
    async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
        let st = self.state.lock().await;
        resolve_kb_store(&st, kb_id)
            .ok_or_else(|| format!("no cozo store registered for KB '{kb_id}'"))
    }
}
