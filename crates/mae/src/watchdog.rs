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

    thread::Builder::new()
        .name("mae-watchdog".into())
        .spawn(move || {
            watchdog_loop(heartbeat, stall_count, stall_recovery);
        })
        .expect("failed to spawn watchdog thread");

    state
}

fn watchdog_loop(
    heartbeat: Arc<AtomicU64>,
    stall_count_out: Arc<AtomicU64>,
    stall_recovery: Arc<AtomicBool>,
) {
    const CHECK_INTERVAL: Duration = Duration::from_secs(2);
    const ALERT_THRESHOLD: u32 = 3; // 6s
    const BACKTRACE_THRESHOLD: u32 = 5; // 10s

    let mut last_heartbeat = heartbeat.load(Ordering::Relaxed);
    let mut consecutive_stalls: u32 = 0;

    loop {
        thread::sleep(CHECK_INTERVAL);

        let current = heartbeat.load(Ordering::Relaxed);
        if current == last_heartbeat {
            consecutive_stalls += 1;
            stall_count_out.store(consecutive_stalls as u64, Ordering::Relaxed);

            if consecutive_stalls == ALERT_THRESHOLD {
                let threads = read_thread_info();
                warn!(
                    stall_seconds = consecutive_stalls * 2,
                    thread_count = threads.len(),
                    "WATCHDOG: main thread stall detected"
                );
                for t in &threads {
                    warn!(tid = t.id, name = %t.name, state = %t.state, "thread state");
                }
            }

            if consecutive_stalls == BACKTRACE_THRESHOLD {
                let bt = std::backtrace::Backtrace::force_capture();
                error!(
                    stall_seconds = consecutive_stalls * 2,
                    "WATCHDOG: prolonged stall — setting recovery flag\n{}", bt
                );
                stall_recovery.store(true, Ordering::Relaxed);
            }
        } else {
            if consecutive_stalls >= ALERT_THRESHOLD {
                warn!(
                    stall_seconds = consecutive_stalls * 2,
                    "WATCHDOG: main thread recovered"
                );
            }
            consecutive_stalls = 0;
            stall_count_out.store(0, Ordering::Relaxed);
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
