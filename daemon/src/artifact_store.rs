//! ADR-034 (ADR-061 Phase D3): the `ArtifactStore` trait — bridges the
//! `kb/fetch_artifact` collab RPC (in `collab_handler`, a LIBRARY-crate
//! module) to the local KB content store's embedding cache
//! (`CozoKbStore::get_cached_embedding`, reached via `DaemonState`/
//! `resolve_kb_store`, both BINARY-crate-only — `main.rs`'s `mod handler;`
//! is private and not declared in `lib.rs` at all).
//!
//! Same crate-boundary rationale as `lease_fence.rs`: a library can never
//! implement a trait defined in its own downstream binary, so this small,
//! standalone trait lives in the library, and the binary crate
//! (`daemon/src/handler.rs`) provides the real implementation wrapping
//! `Arc<tokio::sync::Mutex<DaemonState>>` + `resolve_kb_store`.

use async_trait::async_trait;

/// Looks up a cached embedding vector for a given KB + content hash + model +
/// chunk version. `Ok(None)` means the KB is known locally but nothing is
/// cached for this exact key yet (not an error — the requester should
/// recompute). `Err` means the KB itself isn't known/available to this
/// daemon at all.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn get_cached_embedding(
        &self,
        kb_id: &str,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, String>;
}

/// The default store for a daemon build/test configuration with no local KB
/// content store wired in at all — every lookup reports the KB unknown. Real
/// production wiring always uses `handler::DaemonArtifactStore` instead; this
/// exists so call sites that genuinely have nothing to serve (e.g. a
/// P2P-only relay node with no local replica) don't need a special case.
pub struct NoArtifactStore;

#[async_trait]
impl ArtifactStore for NoArtifactStore {
    async fn get_cached_embedding(
        &self,
        kb_id: &str,
        _content_hash: &str,
        _model: &str,
        _chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, String> {
        Err(format!(
            "KB '{kb_id}' has no local content store on this daemon"
        ))
    }
}
