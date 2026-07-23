//! DaemonScheduler — tokio interval tasks for background KB maintenance.

use crate::config::DaemonConfig;
use crate::handler::DaemonState;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

/// Background task scheduler for daemon maintenance operations.
pub struct DaemonScheduler {
    config: DaemonConfig,
    /// Shared daemon state for scheduler tasks to operate on.
    state: Arc<Mutex<SchedulerState>>,
    /// Shared daemon state (stores, query layer).
    daemon_state: Arc<Mutex<DaemonState>>,
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
    pub fn new(config: DaemonConfig, daemon_state: Arc<Mutex<DaemonState>>) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(SchedulerState::default())),
            daemon_state,
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
                    let mut s = state.lock().await;
                    s.health_cycles += 1;
                    tracing::debug!(cycle = s.health_cycles, "Health check tick");
                    drop(s);

                    // Run hygiene scan if a store is available
                    let ds = self.daemon_state.lock().await;
                    if let Some(ref store) = ds.store {
                        let store = std::sync::Arc::clone(store);
                        drop(ds); // Release lock before blocking scan
                        // Off the async executor (ADR-054) — a synchronous CozoDB
                        // scan left inline here would starve every connection's
                        // I/O sharing this worker thread for the scan's duration.
                        let result = match tokio::task::spawn_blocking(move || {
                            crate::hygiene::run_hygiene_scan(&store)
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(e) => {
                                tracing::warn!(error = %e, "hygiene scan task panicked");
                                continue;
                            }
                        };
                        if result.suggestions_created > 0 {
                            tracing::info!(
                                created = result.suggestions_created,
                                scanned = result.nodes_scanned,
                                "Hygiene scan complete"
                            );
                        }
                        for err in &result.errors {
                            tracing::warn!(error = %err, "Hygiene scan error");
                        }
                    }
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
        let daemon_state = Arc::new(Mutex::new(DaemonState::new()));
        let scheduler = DaemonScheduler::new(config, daemon_state);
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
