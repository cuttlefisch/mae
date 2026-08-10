//! `wait_until!` — the condition-wait that replaces `sleep(n)` in the daemon's
//! integration tests (issue #693).
//!
//! Deliberately its own module rather than living in `common/mod.rs`: that one
//! is the harness for the suites that spawn a REAL daemon over TCP, and the
//! files needing this macro (`collab_e2e.rs`, `collab_stress.rs`) drive an
//! in-process server over duplex pipes instead. Pulling `common` in just for a
//! macro would compile a pile of unrelated spawn helpers into those binaries and
//! blur what `common` is for.
//!
//! Also deliberately not duplicated per file: two copies of a synchronization
//! primitive drift, and the drift would be invisible until one of them hangs.

/// Poll a condition until it holds, or fail after `budget`.
///
/// A fixed `sleep(n)` is a bet that the machine is fast enough — simultaneously
/// too slow (every passing run pays the full duration) and too fast (a loaded
/// runner loses). Waiting on the property returns the moment it holds and only
/// spends the budget when something is genuinely wrong.
///
/// A macro rather than a function taking a closure: the conditions worth waiting
/// on call `&mut self` methods on live clients, and an `FnMut` returning a future
/// cannot hold that borrow across the call.
///
/// @ai-caution: [test-safety] The budget wraps the WHOLE wait via
/// `tokio::time::timeout`, not just the gaps between attempts. An earlier version
/// checked elapsed time only *after* evaluating the condition, so a condition that
/// itself blocked forever never reached the deadline and hung the run instead of
/// failing it. A timeout you can starve is not a timeout — do not "simplify" this
/// back into a bare loop.
#[macro_export]
macro_rules! wait_until {
    ($what:expr, $budget:expr, $cond:expr) => {{
        let __r = tokio::time::timeout($budget, async {
            loop {
                if $cond {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        if __r.is_err() {
            panic!("timed out after {:?} waiting for: {}", $budget, $what);
        }
    }};
}

/// Shared budget for these waits. Generous on purpose: it is a backstop for a
/// genuine hang, not a tuning knob — a correct run never approaches it.
#[allow(dead_code)]
pub const WAIT: std::time::Duration = std::time::Duration::from_secs(10);
