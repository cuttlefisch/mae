//! Lock contention tracking for FairMutex instrumentation.
//!
//! Records acquisition count, total/max wait time per lock site.
//! Exposed via the `introspect` AI tool.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Per-site lock statistics.
#[derive(Debug, Clone, Default)]
pub struct LockEntry {
    pub acquisitions: u64,
    pub total_wait_us: u64,
    pub max_wait_us: u64,
    pub currently_held: bool,
}

/// Global lock stats registry.
static LOCK_STATS: std::sync::OnceLock<Mutex<HashMap<&'static str, LockEntry>>> =
    std::sync::OnceLock::new();

fn global_stats() -> &'static Mutex<HashMap<&'static str, LockEntry>> {
    LOCK_STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record a lock acquisition at `site` that waited `wait` duration.
pub fn record_lock(site: &'static str, wait: Duration) {
    let us = wait.as_micros() as u64;
    let Ok(mut map) = global_stats().lock() else {
        return;
    };
    let entry = map.entry(site).or_default();
    entry.acquisitions += 1;
    entry.total_wait_us += us;
    if us > entry.max_wait_us {
        entry.max_wait_us = us;
    }
}

/// Mark a lock site as currently held or released.
pub fn set_held(site: &'static str, held: bool) {
    let Ok(mut map) = global_stats().lock() else {
        return;
    };
    let entry = map.entry(site).or_default();
    entry.currently_held = held;
}

/// Snapshot all lock stats for reporting.
pub fn snapshot() -> HashMap<String, LockEntry> {
    let Ok(map) = global_stats().lock() else {
        return HashMap::new();
    };
    map.iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}
