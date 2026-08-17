//! Watchdog thread — detects main-thread stalls via heartbeat monitoring.
//!
//! Runs on a standalone OS thread (not tokio) so it remains responsive even
//! when the async runtime is blocked. Checks a shared `AtomicU64` heartbeat
//! counter every 2 seconds. If the counter hasn't advanced after 3 checks (6s),
//! it dumps thread state to the log. After prolonged stalls (>10s), sets a
//! recovery flag that the main thread can check to cancel pending AI work.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tracing::{error, warn};

/// Alert emitted when the watchdog detects a problem.
#[derive(Debug)]
#[allow(dead_code)]
pub enum WatchdogAlert {
    /// Main thread hasn't incremented the heartbeat for `stall_count` checks.
    MainThreadStall {
        stall_count: u32,
        thread_info: Vec<ThreadDump>,
    },
}

/// Per-thread state snapshot from /proc/self/task.
#[derive(Debug, Clone)]
pub struct ThreadDump {
    pub name: String,
    pub id: u64,
    pub state: String,
}

/// Shared watchdog state, accessible from the main thread for introspection.
pub struct WatchdogState {
    pub heartbeat: Arc<AtomicU64>,
    /// Number of consecutive stalls detected (0 = healthy).
    pub stall_count: Arc<AtomicU64>,
    /// Set by watchdog after prolonged stall (>10s). Main thread checks this
    /// on wake to cancel pending AI work and force a redraw.
    pub stall_recovery: Arc<AtomicBool>,
}

impl WatchdogState {
    pub fn new() -> Self {
        WatchdogState {
            heartbeat: Arc::new(AtomicU64::new(0)),
            stall_count: Arc::new(AtomicU64::new(0)),
            stall_recovery: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Increment the heartbeat — call this each event loop tick.
    #[allow(dead_code)]
    pub fn tick(&self) {
        self.heartbeat.fetch_add(1, Ordering::Relaxed);
    }

    /// Check and clear the stall recovery flag. Returns true if recovery
    /// was requested (main thread should cancel pending AI work and redraw).
    #[allow(dead_code)]
    pub fn check_recovery(&self) -> bool {
        self.stall_recovery.swap(false, Ordering::Relaxed)
    }
}

/// Spawn the watchdog thread. Returns the shared state for heartbeat ticking.
pub fn spawn_watchdog() -> WatchdogState {
    let state = WatchdogState::new();
    let heartbeat = state.heartbeat.clone();
    let stall_count = state.stall_count.clone();
    let stall_recovery = state.stall_recovery.clone();

    match thread::Builder::new()
        .name("mae-watchdog".into())
        .spawn(move || {
            watchdog_loop(heartbeat, stall_count, stall_recovery);
        }) {
        Ok(_) => {}
        Err(e) => {
            error!("failed to spawn watchdog thread: {e} — stall detection disabled");
        }
    }

    state
}

/// How long the watchdog waits between heartbeat samples.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Consecutive missed samples before reporting a stall (6s).
const ALERT_THRESHOLD: u32 = 3;
/// Consecutive missed samples before dumping a backtrace and asking the main
/// thread to recover (10s).
const BACKTRACE_THRESHOLD: u32 = 5;
/// Consecutive samples with NO heartbeat ever seen before reporting that
/// bootstrap has not reached the event loop.
///
/// Deliberately far above the stall thresholds, because this measures a
/// different thing: not "the loop stopped" but "the loop has not started". A
/// debug-build boot legitimately takes ~16s — opening the primary CozoDB store
/// alone accounts for most of it — against ~1.7s for a release build, so
/// anything under a wide margin here is normal startup, not a fault. 60s.
const BOOTSTRAP_ALERT_THRESHOLD: u32 = 30;

/// What the watchdog should do with one heartbeat sample.
///
/// A pure decision so the rule is testable without running the thread: the loop
/// below only sleeps, samples, and executes what this returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogAction {
    /// Nothing to report.
    Quiet,
    /// The event loop has not started yet and bootstrap is taking unusually
    /// long. Reported, but NEVER escalated to recovery — see `decide`.
    BootstrapSlow { seconds: u32 },
    /// The heartbeat stopped advancing; report it with a thread dump.
    Stalled { seconds: u32 },
    /// Prolonged stall — dump a backtrace and set the recovery flag.
    StalledProlonged { seconds: u32 },
    /// The heartbeat advanced again after a reported stall.
    Recovered { seconds: u32 },
}

/// Decide what a single sample means.
///
/// **The watchdog arms on the first heartbeat.** `heartbeat == 0` means the
/// event loop has never ticked, which during startup is the normal state, not a
/// fault: the main thread is still in bootstrap and there is no loop to be
/// stalled. Treating that as a stall was a category error — it fired on EVERY
/// debug boot (~16s, dominated by the primary CozoDB store open) and set the
/// recovery flag, whose only effect is to cancel pending AI work on the next
/// event-loop wake. There is no pending AI work before the loop starts, so the
/// flag self-cleared harmlessly and the whole sequence was pure noise — noise
/// that a real stall would then have been indistinguishable from.
///
/// Bootstrap is still watched, at its own threshold and under its own name, so
/// a genuine hang before the loop starts is still visible. It never escalates
/// to `StalledProlonged`: recovery is an event-loop action and there is no
/// event loop to act.
pub(crate) fn decide(current: u64, last: u64, consecutive: u32) -> WatchdogAction {
    let advanced = current != last;
    if advanced {
        // `last == 0` means this advance is the event loop's FIRST tick — the
        // watchdog arming, not a recovery. Reporting "main thread recovered" there
        // announces a stall that was never reported, which is the same false
        // signal in the opposite direction: it tells an operator the process
        // wedged and unwedged during startup when nothing of the sort happened.
        return if last != 0 && consecutive >= ALERT_THRESHOLD {
            WatchdogAction::Recovered {
                seconds: consecutive * CHECK_INTERVAL.as_secs() as u32,
            }
        } else {
            WatchdogAction::Quiet
        };
    }
    let seconds = consecutive * CHECK_INTERVAL.as_secs() as u32;
    if current == 0 {
        // Pre-loop: bootstrap has not reached its first tick.
        return if consecutive == BOOTSTRAP_ALERT_THRESHOLD {
            WatchdogAction::BootstrapSlow { seconds }
        } else {
            WatchdogAction::Quiet
        };
    }
    match consecutive {
        c if c == ALERT_THRESHOLD => WatchdogAction::Stalled { seconds },
        c if c == BACKTRACE_THRESHOLD => WatchdogAction::StalledProlonged { seconds },
        _ => WatchdogAction::Quiet,
    }
}

fn watchdog_loop(
    heartbeat: Arc<AtomicU64>,
    stall_count_out: Arc<AtomicU64>,
    stall_recovery: Arc<AtomicBool>,
) {
    let mut last_heartbeat = heartbeat.load(Ordering::Relaxed);
    let mut consecutive_stalls: u32 = 0;

    loop {
        thread::sleep(CHECK_INTERVAL);

        let current = heartbeat.load(Ordering::Relaxed);
        let advanced = current != last_heartbeat;
        if !advanced {
            consecutive_stalls += 1;
        }
        // Only a post-arming stall counts as a stall for introspection: a
        // pre-loop sample is startup, and reporting it as `stall_count` would
        // show a freshly booting editor as unhealthy.
        stall_count_out.store(
            if current == 0 || advanced {
                0
            } else {
                consecutive_stalls as u64
            },
            Ordering::Relaxed,
        );

        match decide(current, last_heartbeat, consecutive_stalls) {
            WatchdogAction::Quiet => {}
            WatchdogAction::BootstrapSlow { seconds } => {
                warn!(
                    bootstrap_seconds = seconds,
                    "WATCHDOG: bootstrap has not reached the event loop yet"
                );
            }
            WatchdogAction::Stalled { seconds } => {
                let threads = read_thread_info();
                warn!(
                    stall_seconds = seconds,
                    thread_count = threads.len(),
                    "WATCHDOG: main thread stall detected"
                );
                for t in &threads {
                    warn!(tid = t.id, name = %t.name, state = %t.state, "thread state");
                }
            }
            WatchdogAction::StalledProlonged { seconds } => {
                let bt = std::backtrace::Backtrace::force_capture();
                error!(
                    stall_seconds = seconds,
                    "WATCHDOG: prolonged stall — setting recovery flag\n{}", bt
                );
                stall_recovery.store(true, Ordering::Relaxed);
            }
            WatchdogAction::Recovered { seconds } => {
                warn!(stall_seconds = seconds, "WATCHDOG: main thread recovered");
            }
        }

        if advanced {
            consecutive_stalls = 0;
            last_heartbeat = current;
        }
    }
}

/// Read thread info from /proc/self/task (Linux-specific, best-effort).
fn read_thread_info() -> Vec<ThreadDump> {
    let mut threads = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc/self/task") else {
        return threads;
    };
    for entry in entries.flatten() {
        let tid_str = entry.file_name();
        let Some(tid_s) = tid_str.to_str() else {
            continue;
        };
        let Ok(tid) = tid_s.parse::<u64>() else {
            continue;
        };

        let status_path = entry.path().join("status");
        let status = std::fs::read_to_string(&status_path).unwrap_or_default();

        let name = status
            .lines()
            .find(|l| l.starts_with("Name:"))
            .map(|l| l.trim_start_matches("Name:").trim().to_string())
            .unwrap_or_else(|| format!("tid-{}", tid));

        let state = status
            .lines()
            .find(|l| l.starts_with("State:"))
            .map(|l| l.trim_start_matches("State:").trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        threads.push(ThreadDump {
            name,
            id: tid,
            state,
        });
    }
    threads
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The category error this rule exists to fix: before the event loop's first
    /// tick there is no liveness signal, so silence is startup — not a stall.
    ///
    /// It fired on every debug boot (~16s, dominated by the primary CozoDB store
    /// open; a release boot is ~1.7s) and escalated to the recovery flag, whose
    /// only effect is cancelling pending AI work on the next event-loop wake.
    /// Nothing is pending before the loop starts, so it self-cleared — leaving a
    /// warning and a backtrace that a REAL stall would have been indistinguishable
    /// from.
    #[test]
    fn a_boot_that_has_not_reached_the_event_loop_is_not_a_stall() {
        for consecutive in 1..=BACKTRACE_THRESHOLD + 5 {
            assert_eq!(
                decide(0, 0, consecutive),
                WatchdogAction::Quiet,
                "heartbeat 0 means the loop has never ticked; sample {consecutive} \
                 must not be reported as a stall"
            );
        }
    }

    /// …but a boot that never reaches the loop is still watched, under its own
    /// name and threshold. Dropping the signal entirely would trade a false
    /// positive for a blind spot.
    #[test]
    fn a_boot_that_never_reaches_the_event_loop_is_still_reported() {
        assert_eq!(
            decide(0, 0, BOOTSTRAP_ALERT_THRESHOLD),
            WatchdogAction::BootstrapSlow {
                seconds: BOOTSTRAP_ALERT_THRESHOLD * 2
            }
        );
        // A compile-time relationship, so assert it at compile time: the bootstrap
        // threshold must sit far above the stall thresholds, because it measures a
        // different thing and a debug boot legitimately takes ~16s.
        const _: () = assert!(BOOTSTRAP_ALERT_THRESHOLD > BACKTRACE_THRESHOLD * 4);
    }

    /// And it NEVER escalates to recovery. Recovery cancels pending AI work on
    /// the event loop; there is no event loop, so the action is meaningless and
    /// the flag would be a lie about the process state.
    #[test]
    fn bootstrap_never_escalates_to_recovery() {
        for consecutive in 1..=BOOTSTRAP_ALERT_THRESHOLD * 3 {
            assert_ne!(
                decide(0, 0, consecutive),
                WatchdogAction::StalledProlonged {
                    seconds: consecutive * 2
                },
                "a pre-loop sample must never set the recovery flag"
            );
        }
    }

    /// The event loop's first tick is the watchdog ARMING, not a recovery.
    ///
    /// Found by watching a real boot rather than by reasoning: suppressing the
    /// false stall left `consecutive` accumulating through bootstrap, so the
    /// 0 → 1 advance reported "main thread recovered" — announcing a stall that
    /// was never reported. Same false signal, opposite direction.
    #[test]
    fn the_first_tick_after_bootstrap_is_not_a_recovery() {
        for consecutive in 0..=BOOTSTRAP_ALERT_THRESHOLD {
            assert_eq!(
                decide(1, 0, consecutive),
                WatchdogAction::Quiet,
                "advancing from a never-ticked heartbeat is the loop starting \
                 (sample {consecutive}), not a recovery"
            );
        }
    }

    /// Once the loop has ticked, the watchdog is armed and behaves exactly as
    /// before — this is the property the fix must not weaken.
    #[test]
    fn a_running_loop_that_stops_still_stalls_and_recovers() {
        assert_eq!(
            decide(7, 7, ALERT_THRESHOLD),
            WatchdogAction::Stalled { seconds: 6 }
        );
        assert_eq!(
            decide(7, 7, BACKTRACE_THRESHOLD),
            WatchdogAction::StalledProlonged { seconds: 10 }
        );
        // Progress after a reported stall is a recovery…
        assert_eq!(
            decide(8, 7, ALERT_THRESHOLD),
            WatchdogAction::Recovered { seconds: 6 }
        );
        // …but progress that was never reported stalled is simply quiet.
        assert_eq!(decide(8, 7, 1), WatchdogAction::Quiet);
        assert_eq!(decide(1, 0, 0), WatchdogAction::Quiet);
    }
}
