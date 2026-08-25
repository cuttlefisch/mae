//! Whether an unsigned KB content op is accepted, and how often one was.
//!
//! ADR-036 signs content ops. The **mesh** has always required a signature
//! (`Transport::P2p`); the **hub** accepts unsigned ops as a migration
//! accommodation, whose own comment in `verify_relayed_content_op` reads
//! *"hub migration: accept legacy unsigned"*.
//!
//! @ai-caution: [security] That accommodation had **no config flag, no deadline
//! and no metric**, so no operator could ever establish that it was safe to
//! close — which makes a migration path permanent by default. This module adds
//! the two things that make the decision falsifiable: a lever, and the evidence
//! to pull it on.
//!
//! Process-global rather than threaded through the request path, deliberately:
//! this is a daemon-wide policy fixed at startup from `[collab.auth]`, not a
//! per-request value, and threading it would touch every signature between
//! `handle_doc_request_inner` and `verify_relayed_content_op` for a value that
//! never varies within a run. The counter is monotonic, so parallel tests
//! observing it cannot corrupt each other; `set_require_signed` is startup-only
//! and tests that exercise it should restore the prior value.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use mae_sync::kb::Transport;

static REQUIRE_SIGNED: AtomicBool = AtomicBool::new(false);
static UNSIGNED_ACCEPTED: AtomicU64 = AtomicU64::new(0);

/// Apply `[collab.auth] require_signed_content_ops`. Called once at startup.
pub fn set_require_signed(v: bool) {
    REQUIRE_SIGNED.store(v, Ordering::Relaxed);
}

/// Whether this transport must reject an unsigned content op.
///
/// The mesh always must — a relaying peer is untrusted, and an unsigned op
/// there has no author to check membership against. The hub must only when the
/// operator has flipped the flag.
pub fn require_signed(transport: Transport) -> bool {
    matches!(transport, Transport::P2p) || REQUIRE_SIGNED.load(Ordering::Relaxed)
}

/// Record that an unsigned content op was accepted on the hub.
///
/// This is the exit criterion. A deployment whose count has stayed at zero
/// across a representative period can set `require_signed_content_ops = true`
/// and know it will break nothing; a non-zero count names a client still to be
/// upgraded. Without it, "is the migration over?" is unanswerable.
pub fn note_unsigned_accepted() {
    UNSIGNED_ACCEPTED.fetch_add(1, Ordering::Relaxed);
}

/// How many unsigned content ops have been accepted since this process started.
pub fn unsigned_accepted() -> u64 {
    UNSIGNED_ACCEPTED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mesh_requires_a_signature_regardless_of_configuration() {
        let prior = REQUIRE_SIGNED.load(Ordering::Relaxed);
        set_require_signed(false);
        assert!(
            require_signed(Transport::P2p),
            "a relaying peer is untrusted; the mesh must never accept unsigned, \
             whatever the hub is configured to do"
        );
        set_require_signed(prior);
    }

    #[test]
    fn the_counter_is_the_exit_criterion_and_actually_counts() {
        let before = unsigned_accepted();
        note_unsigned_accepted();
        note_unsigned_accepted();
        assert_eq!(
            unsigned_accepted() - before,
            2,
            "a counter that does not move cannot answer 'is the migration over?'"
        );
    }
}
