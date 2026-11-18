//! Whether an unsigned KB content op is accepted, and how often one was.
//!
//! ADR-036 signs content ops. The **mesh** has always required a signature
//! (`Transport::P2p`); the **hub** accepts unsigned ops until the operator sets
//! `[collab.auth] require_signed_content_ops` (ADR-036 Amendment 1 F6). The
//! switch applies to every write path that carries a content op — `sync/update`
//! and `kb/node_update`, over every transport that reaches them — because each
//! of them asks this one policy.
//!
//! @ai-caution: [security] The accommodation had **no config flag, no deadline
//! and no metric** until this module, so no operator could establish that it
//! was safe to close. The counter is the exit criterion: turn the switch on
//! once it stays at zero over the pilot.
//!
//! Owned by the `DocStore`, not a process global: every handler that verifies
//! an op already holds the store, and a global flipped by one test changed the
//! policy under every other test running in the same process.

use std::sync::atomic::{AtomicU64, Ordering};

use mae_sync::kb::Transport;

/// The daemon's content-op signing policy and its migration counter.
#[derive(Debug, Default)]
pub struct ContentOpPolicy {
    require_signed: bool,
    unsigned_accepted: AtomicU64,
}

impl ContentOpPolicy {
    /// `[collab.auth] require_signed_content_ops`, fixed at startup.
    pub fn new(require_signed: bool) -> Self {
        Self {
            require_signed,
            unsigned_accepted: AtomicU64::new(0),
        }
    }

    /// Whether this transport must refuse an unsigned content op.
    ///
    /// The mesh always must: a relaying peer is untrusted, and an unsigned op
    /// there has no author to check membership against. The hub must when the
    /// operator has turned the switch on.
    pub fn require_signed(&self, transport: Transport) -> bool {
        matches!(transport, Transport::P2p) || self.require_signed
    }

    /// Record that an unsigned content op was accepted on the hub.
    pub fn note_unsigned_accepted(&self) {
        self.unsigned_accepted.fetch_add(1, Ordering::Relaxed);
    }

    /// How many unsigned content ops this store has accepted since it opened.
    pub fn unsigned_accepted(&self) -> u64 {
        self.unsigned_accepted.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mesh_requires_a_signature_regardless_of_configuration() {
        let off = ContentOpPolicy::new(false);
        assert!(
            off.require_signed(Transport::P2p),
            "a relaying peer is untrusted; the mesh must never accept unsigned, \
             whatever the hub is configured to do"
        );
        assert!(
            !off.require_signed(Transport::Hub),
            "the hub follows the switch"
        );
        assert!(ContentOpPolicy::new(true).require_signed(Transport::Hub));
    }

    #[test]
    fn the_counter_is_the_exit_criterion_and_actually_counts() {
        let p = ContentOpPolicy::default();
        p.note_unsigned_accepted();
        p.note_unsigned_accepted();
        assert_eq!(
            p.unsigned_accepted(),
            2,
            "a counter that does not move cannot answer 'is the migration over?'"
        );
    }
}
