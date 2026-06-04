//! DaemonScheduler — tokio interval tasks for background KB maintenance.

use crate::config::DaemonConfig;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

/// Background task scheduler for daemon maintenance operations.
pub struct DaemonScheduler {
    config: DaemonConfig,
    /// Shared daemon state for scheduler tasks to operate on.
    state: Arc<Mutex<SchedulerState>>,
}

/// Mutable state accessed by scheduler tasks.
#[derive(Default)]
pub struct SchedulerState {
    /// Number of watcher drain cycles completed.
    pub drain_cycles: u64,
    /// Number of maintenance cycles completed.
    pub maintenance_cycles: u64,
    /// Number of health checks completed.
    pub health_cycles: u64,
    /// Whether the scheduler is running.
    pub running: bool,
}

impl DaemonScheduler {
    pub fn new(config: DaemonConfig) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(SchedulerState::default())),
        }
    }

    /// Run all scheduled tasks. Cancels on shutdown signal.
    pub async fn run(&self, mut shutdown: tokio::sync::broadcast::Receiver<()>) {
        let state = Arc::clone(&self.state);
        {
            let mut s = state.lock().await;
            s.running = true;
        }

        let watcher_interval = Duration::from_millis(self.config.watcher_interval_ms);
        let maintenance_interval = Duration::from_secs(self.config.maintenance_interval_secs);
        let health_interval = Duration::from_secs(self.config.health_interval_secs);

        let mut watcher_tick = interval(watcher_interval);
        let mut maintenance_tick = interval(maintenance_interval);
        let mut health_tick = interval(health_interval);

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    tracing::info!("Scheduler shutting down");
                    break;
                }
                _ = watcher_tick.tick() => {
                    // TODO: drain file watcher events, trigger incremental reimport
                    let mut s = state.lock().await;
                    s.drain_cycles += 1;
                }
                _ = maintenance_tick.tick() => {
                    // TODO: integrity check, statistics, compaction
                    let mut s = state.lock().await;
                    s.maintenance_cycles += 1;
                    tracing::debug!(cycle = s.maintenance_cycles, "DB maintenance tick");
                }
                _ = health_tick.tick() => {
                    // TODO: broken links, stale nodes, orphan detection
                    let mut s = state.lock().await;
                    s.health_cycles += 1;
                    tracing::debug!(cycle = s.health_cycles, "Health check tick");
                }
            }
        }

        let mut s = state.lock().await;
        s.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scheduler_starts_and_stops() {
        let config = DaemonConfig::default();
        let scheduler = DaemonScheduler::new(config);
        let state = Arc::clone(&scheduler.state);

        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

        let handle = tokio::spawn(async move {
            scheduler.run(shutdown_rx).await;
        });

        // Let it run a few ticks
        tokio::time::sleep(Duration::from_millis(100)).await;

        {
            let s = state.lock().await;
            assert!(s.running);
            assert!(s.drain_cycles > 0);
        }

        // Shutdown
        shutdown_tx.send(()).unwrap();
        handle.await.unwrap();

        let s = state.lock().await;
        assert!(!s.running);
    }
}
