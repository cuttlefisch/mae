//! Shared connection-count limiter (ADR-054), extracted from the collab TCP
//! listener's inline `#342` fix so the same behavior can be reused by the KB
//! Unix-socket and P2P mesh accept loops without a third hand-copied
//! implementation (CLAUDE.md principle #8 — two inline copies were
//! defensible, a third tips the balance).
//!
//! Behavior-identical to the pattern it replaces: `std::sync::atomic::AtomicUsize`
//! with `Relaxed` ordering (a connection-count cap doesn't need stronger
//! ordering — it's an approximate, best-effort admission check, not a
//! correctness-critical invariant), `max == 0` means unlimited, and the
//! returned guard decrements on every exit path (normal return, early
//! `return`, or a panic unwinding through the spawned task) via `Drop`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Caps the number of concurrently active connections a listener has
/// accepted (authenticated or not — this bounds raw socket/task
/// accumulation, independent of whatever auth happens afterward).
#[derive(Clone)]
pub struct ConnLimiter {
    active: Arc<AtomicUsize>,
    max: usize,
}

impl ConnLimiter {
    /// `max == 0` means unlimited (no cap enforced, `try_acquire` always succeeds).
    pub fn new(max: usize) -> Self {
        ConnLimiter {
            active: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    /// Attempt to admit one more connection. `None` if already at capacity
    /// (the caller should reject/drop the connection without spawning
    /// anything for it); `Some(guard)` otherwise — the count is already
    /// incremented, and stays incremented until the guard is dropped.
    pub fn try_acquire(&self) -> Option<ConnGuard> {
        if self.max > 0 && self.active.load(Ordering::Relaxed) >= self.max {
            return None;
        }
        self.active.fetch_add(1, Ordering::Relaxed);
        Some(ConnGuard(Arc::clone(&self.active)))
    }

    /// Current in-flight connection count. Diagnostic/logging use only — the
    /// admission decision itself lives in `try_acquire`, not a separate
    /// check-then-act pair, so there's no TOCTOU window in normal use.
    pub fn current(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

/// RAII handle for one admitted connection. Decrements the shared counter on
/// drop regardless of how the holding task ends (normal return, early
/// `return`, or a panic unwinding through it) — the count can never leak.
pub struct ConnGuard(Arc<AtomicUsize>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_when_max_is_zero() {
        let limiter = ConnLimiter::new(0);
        let guards: Vec<_> = (0..100).map(|_| limiter.try_acquire()).collect();
        assert!(guards.iter().all(|g| g.is_some()));
    }

    #[test]
    fn rejects_at_capacity_and_admits_again_after_a_guard_drops() {
        let limiter = ConnLimiter::new(2);
        let g1 = limiter.try_acquire().expect("1st admitted");
        let g2 = limiter.try_acquire().expect("2nd admitted");
        assert!(
            limiter.try_acquire().is_none(),
            "3rd must be rejected at cap"
        );
        drop(g1);
        assert!(
            limiter.try_acquire().is_some(),
            "freed slot must be reusable"
        );
        drop(g2);
    }

    #[test]
    fn guard_decrements_even_when_dropped_via_a_panic_unwind() {
        let limiter = ConnLimiter::new(1);
        let limiter_clone = limiter.clone();
        let result = std::panic::catch_unwind(move || {
            let _guard = limiter_clone.try_acquire().expect("admitted");
            assert_eq!(limiter_clone.current(), 1);
            panic!("simulated task panic while holding a connection guard");
        });
        assert!(result.is_err(), "the panic should have propagated");
        assert_eq!(
            limiter.current(),
            0,
            "the guard's Drop impl must still run during unwind, decrementing the count"
        );
        assert!(
            limiter.try_acquire().is_some(),
            "capacity must be available again after the panicking holder unwound"
        );
    }

    #[test]
    fn current_reflects_live_count() {
        let limiter = ConnLimiter::new(5);
        assert_eq!(limiter.current(), 0);
        let _g1 = limiter.try_acquire().unwrap();
        let _g2 = limiter.try_acquire().unwrap();
        assert_eq!(limiter.current(), 2);
    }
}
