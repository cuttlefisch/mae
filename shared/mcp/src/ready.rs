//! Wait for something to become ready, bounded by wall-clock rather than by a
//! fixed iteration count.
//!
//! @ai-caution: [test-timing] E2E tests must not spell readiness as
//! `for _ in 0..50 { sleep(100ms); check() }`. That is a **fixed budget for
//! something whose duration depends on how loaded the machine is** — it never
//! expires locally and expires under CI contention, so the failure lands on
//! whichever PR happened to share a runner with a heavy job. Three separate
//! MAE tests failed that way in two days (#769); a fourth did the same thing in
//! bash (#762).
//!
//! MAE already learned this twice and did not carry it to the test harness:
//!
//! * **#484** replaced `db.rs`'s 400-*attempt* busy-retry budget with a
//!   wall-clock deadline, recording that *"a per-attempt count is an indirect,
//!   hardware-dependent proxy for 'how long can I wait' — it silently
//!   under-budgets on a slower/more contended CI runner than whatever machine
//!   last tuned the number."*
//! * **#494** created `mae_kb::watch::wait_for` and warned in its own doc
//!   comment that hand-rolled copies of the same loop *"silently drift"*. That
//!   helper is **sync and watcher-scoped**, so the async readiness sites here
//!   could not use it.
//!
//! Raising `0..50` to `0..200` is deliberately **not** the fix: it trades a fast
//! failure for a slow one and leaves the next contributor guessing a number.

use std::future::Future;
use std::time::{Duration, Instant};

/// Default budget. Generous on purpose — on a healthy machine the condition is
/// met in well under a second, so this only sets how long a *genuinely broken*
/// case takes to report, and a too-tight budget is the failure mode being fixed.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long between polls. Short enough that a fast machine is not penalised.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Env var overriding [`DEFAULT_READY_TIMEOUT`], in seconds. Lets CI give
/// itself a longer budget than a laptop **without editing every call site** —
/// which is what made the per-site iteration counts drift apart.
pub const READY_TIMEOUT_ENV: &str = "MAE_E2E_READY_TIMEOUT_SECS";

/// The budget in force, honouring [`READY_TIMEOUT_ENV`].
pub fn ready_timeout() -> Duration {
    std::env::var(READY_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_READY_TIMEOUT)
}

/// Poll `cond` until it yields `Some`, or the budget elapses.
///
/// Returns the value on success. On timeout returns `None` — callers should
/// `.expect()` with a message naming *what* was being waited for, so a failure
/// reads as "the daemon never came up" rather than as an unexplained panic.
pub async fn wait_for_some<T, F, Fut>(mut cond: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = Instant::now() + ready_timeout();
    loop {
        if let Some(v) = cond().await {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Poll `cond` until it yields `true`, or the budget elapses.
pub async fn wait_until<F, Fut>(mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    wait_for_some(|| {
        let f = cond();
        async move { f.await.then_some(()) }
    })
    .await
    .is_some()
}

/// Poll `cond` until it returns `true`, or the budget elapses — **blocking**.
///
/// The sibling of [`wait_until`] for tests that are deliberately *not* async.
/// `daemon/tests/remote_hub_query_layer_e2e.rs` is the motivating case: it
/// drives a blocking `RemoteHubQueryLayer` client, so its module doc records
/// that it uses plain `#[test]` rather than calling a blocking client from
/// inside a tokio runtime. Without this variant that file would have to keep
/// its own hand-rolled iteration count, which is exactly the per-site drift
/// this module exists to remove — same budget, same env override, same
/// [`timeout_message`], one mechanism (CLAUDE.md principle #8).
pub fn wait_until_blocking<F>(mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + ready_timeout();
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The message a timed-out wait should panic with — names the thing, the budget,
/// and the override, so the next person does not have to read this module.
pub fn timeout_message(what: &str) -> String {
    format!(
        "{what} did not become ready within {:?} (override with {READY_TIMEOUT_ENV}=<seconds>)",
        ready_timeout()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_as_soon_as_the_condition_holds() {
        let mut n = 0;
        let started = Instant::now();
        let ok = wait_until(|| {
            n += 1;
            async move { n >= 2 }
        })
        .await;
        assert!(ok);
        // Must not burn the whole budget when the condition is met early — the
        // point is a bounded wait, not a fixed one.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_blocking_variant_returns_as_soon_as_the_condition_holds() {
        let mut n = 0;
        let started = Instant::now();
        let ok = wait_until_blocking(|| {
            n += 1;
            n >= 2
        });
        assert!(ok);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// Everything that depends on [`READY_TIMEOUT_ENV`] lives in ONE test.
    ///
    /// `set_var` is process-wide, and `cargo test` runs a binary's tests in
    /// parallel threads — so N separate tests each setting and clearing this
    /// variable are N racing writers to one cell, and whichever reads while a
    /// sibling holds a different value fails for reasons unrelated to the code
    /// under test. That is precisely the "passes locally, fails on a loaded
    /// runner" shape this whole module exists to eliminate; reproducing it in
    /// the module's own tests would be self-refuting. Serialised by
    /// construction instead of by a lock, because holding a lock across an
    /// `.await` is its own hazard.
    #[test]
    fn the_budget_is_configurable_and_a_never_true_condition_times_out() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        // The override is read at call time, not baked in at first use.
        std::env::set_var(READY_TIMEOUT_ENV, "7");
        assert_eq!(ready_timeout(), Duration::from_secs(7));

        // A never-true condition must terminate on the budget, not hang —
        // async and blocking alike, since a hand-rolled site that got this
        // wrong would hang a whole CI job rather than fail it.
        std::env::set_var(READY_TIMEOUT_ENV, "1");
        let started = Instant::now();
        let ok = rt.block_on(wait_until(|| async { false }));
        assert!(!ok, "a never-true async condition must time out, not hang");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "async wait ignored its budget, took {:?}",
            started.elapsed()
        );

        let started = Instant::now();
        let ok = wait_until_blocking(|| false);
        assert!(
            !ok,
            "a never-true blocking condition must time out, not hang"
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "blocking wait ignored its budget, took {:?}",
            started.elapsed()
        );

        // An unparseable value must fall back to the default rather than
        // silently producing a zero budget (which would make every wait fail
        // on its first poll).
        std::env::set_var(READY_TIMEOUT_ENV, "not-a-number");
        assert_eq!(ready_timeout(), DEFAULT_READY_TIMEOUT);

        std::env::remove_var(READY_TIMEOUT_ENV);
        assert_eq!(ready_timeout(), DEFAULT_READY_TIMEOUT);
    }

    #[test]
    fn the_timeout_message_names_the_thing_and_the_override() {
        let m = timeout_message("PSK mae-daemon");
        assert!(m.contains("PSK mae-daemon"), "{m}");
        assert!(m.contains(READY_TIMEOUT_ENV), "{m}");
    }
}
