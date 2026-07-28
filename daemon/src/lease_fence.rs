//! ADR-061 Phase D2: the `LeaseFence` trait — a write-time re-check of the
//! ADR-033 advisory lease, consulted immediately before a bulk operation
//! commits its results.
//!
//! Lives in this small, standalone module (not inside `enrichment` or
//! `collab_handler`) because of a real crate-boundary constraint: `enrichment`
//! is BINARY-crate-only (`main.rs`'s `pub mod enrichment;`), while
//! `collab_handler` (whose `DaemonLeaseFence` is the production
//! implementation) lives in the LIBRARY crate (`lib.rs`) that the binary
//! depends on — a library can never implement a trait defined in its
//! downstream binary. Defining the trait here, in the library, lets both
//! sides depend on it correctly: `collab_handler::kb_lease::DaemonLeaseFence`
//! implements it directly (same crate); `enrichment.rs` (binary) references
//! it as `mae_daemon::lease_fence::LeaseFence`.

use async_trait::async_trait;

/// `Err` ⇒ someone else was granted the lease since the caller's sweep
/// started (its TTL lapsed and lost the race, or a higher-fingerprint peer
/// preempted it) — the caller must discard its pending write, not commit it.
#[async_trait]
pub trait LeaseFence: Send + Sync {
    async fn check(&self) -> Result<(), String>;
}

/// The default fence for a KB with no lease to coordinate — a KB that isn't
/// currently collab-shared (the common case for a single-daemon topology).
pub struct NoFence;

#[async_trait]
impl LeaseFence for NoFence {
    async fn check(&self) -> Result<(), String> {
        Ok(())
    }
}
