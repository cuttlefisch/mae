//! DaemonScheduler — tokio interval tasks for background KB maintenance.

use crate::config::DaemonConfig;
use crate::handler::{resolve_kb_store, DaemonState};
use mae_kb::CozoKbStore;
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Live per-instance org-directory watchers (ADR-065 item 2), keyed by
    /// instance UUID. Lazily created as org-directory-backed instances are
    /// registered and torn down when they're unregistered — `run()` only
    /// borrows `&self`, hence the `Mutex` for interior mutability.
    watchers: Arc<Mutex<HashMap<String, mae_kb::watch::OrgDirWatcher>>>,
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
            watchers: Arc::new(Mutex::new(HashMap::new())),
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
                    self.run_watcher_tick(&state).await;
                }
                _ = maintenance_tick.tick() => {
                    // Deterministic half only (ADR-065 item 2) — integrity check +
                    // stats. AI-driven enrichment is a separate, larger capability
                    // (ADR-061 Phase C) that claims the *other* half of this same
                    // tick; the two are coordinated by scope, not by a shared
                    // function, so neither implementation collides with the other.
                    self.run_maintenance_tick(&state).await;
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

    /// ADR-065 item 2: drain pending org-file-watcher events for every
    /// registered, org-directory-backed KB instance and trigger a reimport
    /// for any that changed. A no-op today for the daemon's own primary
    /// `daemon-kb.cozo` store — it is populated via collaborative RPC writes
    /// (`kb/create`, `kb/update`), not an org directory, and carries no
    /// `org_dir` — and for `registry.instances` until something registers
    /// one with a real `org_dir` (federated-instance registration is not yet
    /// wired into daemon startup; tracked separately under ADR-060). This
    /// activates automatically the moment either happens, with no further
    /// changes needed here.
    async fn run_watcher_tick(&self, state: &Arc<Mutex<SchedulerState>>) {
        {
            let mut s = state.lock().await;
            s.drain_cycles += 1;
        }

        // Snapshot (uuid, org_dir, store) triples without holding the
        // daemon_state lock across the blocking reimport below.
        let targets: Vec<(String, PathBuf, Arc<CozoKbStore>)> = {
            let ds = self.daemon_state.lock().await;
            ds.registry
                .instances
                .iter()
                .filter(|inst| inst.enabled && !inst.org_dir.as_os_str().is_empty())
                .filter_map(|inst| {
                    let store = resolve_kb_store(&ds, &inst.uuid)?;
                    Some((inst.uuid.clone(), inst.org_dir.clone(), store))
                })
                .collect()
        };

        let mut watchers = self.watchers.lock().await;
        // Drop watchers for instances unregistered since the last tick — the
        // registry can change live (kb_register/kb_unregister RPCs).
        let live_uuids: std::collections::HashSet<&str> =
            targets.iter().map(|(u, _, _)| u.as_str()).collect();
        watchers.retain(|uuid, _| live_uuids.contains(uuid.as_str()));

        for (uuid, org_dir, store) in &targets {
            if !watchers.contains_key(uuid) {
                match mae_kb::watch::OrgDirWatcher::new(org_dir) {
                    Ok(w) => {
                        watchers.insert(uuid.clone(), w);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            uuid,
                            org_dir = %org_dir.display(),
                            "watcher_tick: failed to start watcher for instance"
                        );
                        continue;
                    }
                }
            }
            let Some(watcher) = watchers.get(uuid) else {
                continue;
            };
            let changes = watcher.drain();
            if changes.is_empty() {
                continue;
            }
            tracing::debug!(
                uuid,
                changed = changes.len(),
                "watcher_tick: reimporting instance after detected change"
            );
            let org_dir = org_dir.clone();
            let store = Arc::clone(store);
            // Off the async executor (ADR-054) — a synchronous whole-directory
            // reimport left inline here would starve every connection's I/O
            // sharing this worker thread for the reimport's duration.
            //
            // Idempotent by construction: `IngestMode::Full` re-derives every
            // node from current file content and upserts by id (deleted files'
            // nodes are removed too), so a kill mid-reimport followed by a
            // restart-and-retry naturally converges to the same end state —
            // there is no partial-write state that needs explicit reconciling.
            let result = tokio::task::spawn_blocking(move || {
                mae_kb::federation::import_org_dir_to_store(
                    &org_dir,
                    &store,
                    &mae_kb::federation::IngestMode::Full,
                )
            })
            .await;
            match result {
                Ok(Ok((_kb, report))) => {
                    tracing::info!(
                        uuid,
                        imported = report.nodes_imported,
                        updated = report.nodes_updated,
                        removed = report.nodes_removed,
                        errors = report.errors.len(),
                        "watcher_tick: reimport complete"
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, uuid, "watcher_tick: reimport failed");
                }
                Err(e) => {
                    tracing::warn!(error = %e, uuid, "watcher_tick: reimport task panicked");
                }
            }
        }
    }

    /// ADR-065 item 2: deterministic maintenance pass (stats + integrity
    /// check — see `crate::maintenance::run_maintenance_scan`) against the
    /// primary store and every registered instance store. Read-only; safe to
    /// interrupt and re-run at any point (no partial-write state).
    async fn run_maintenance_tick(&self, state: &Arc<Mutex<SchedulerState>>) {
        {
            let mut s = state.lock().await;
            s.maintenance_cycles += 1;
            tracing::debug!(cycle = s.maintenance_cycles, "DB maintenance tick");
        }

        let stores: Vec<(String, Arc<CozoKbStore>)> = {
            let ds = self.daemon_state.lock().await;
            let mut stores = Vec::new();
            if let Some(ref store) = ds.store {
                stores.push(("primary".to_string(), Arc::clone(store)));
            }
            for (name, store) in &ds.instance_stores {
                stores.push((name.clone(), Arc::clone(store)));
            }
            stores
        };

        for (name, store) in stores {
            // Off the async executor (ADR-054) — same rationale as the
            // hygiene/watcher ticks above.
            let result = tokio::task::spawn_blocking(move || {
                crate::maintenance::run_maintenance_scan(&store)
            })
            .await;
            match result {
                Ok(result) => {
                    if !result.integrity_failures.is_empty() {
                        tracing::warn!(
                            instance = %name,
                            failures = ?result.integrity_failures,
                            "maintenance_tick: integrity check found tampered/corrupted version(s)"
                        );
                    }
                    for err in &result.errors {
                        tracing::warn!(instance = %name, error = %err, "maintenance_tick: scan error");
                    }
                    if let Some(stats) = result.stats {
                        tracing::debug!(
                            instance = %name,
                            total_nodes = stats.total_nodes,
                            total_links = stats.total_links,
                            orphans = stats.orphan_ids.len(),
                            broken_links = stats.broken_links.len(),
                            "maintenance_tick: stats"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, instance = %name, "maintenance_tick: scan task panicked");
                }
            }
        }
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

    #[tokio::test]
    async fn watcher_tick_reimport_converges_after_a_partial_then_full_pass() {
        // Adversarial per CLAUDE.md principle #14 + ADR-065 item 2's DoD ("kill
        // mid-tick, resume must not double-apply completed work and must not
        // silently drop pending work"). A `spawn_blocking` task can't be
        // faithfully interrupted mid-flight in a deterministic unit test (Tokio
        // runs it to completion on its OS thread regardless of `abort()`), so
        // this test instead proves the actual property that makes an
        // interruption safe: `import_org_dir_to_store(..., IngestMode::Full)`
        // re-derives every node from current directory content and upserts by
        // id rather than replaying an incremental diff. Simulate "the daemon
        // was killed after file A was imported but before file B" by reimporting
        // a directory containing only A first, then reimporting again once B has
        // been added — the second (full) pass must converge to both nodes
        // present, correct, and not duplicated.
        let tmp = tempfile::tempdir().unwrap();
        let org_dir = tmp.path().join("notes");
        std::fs::create_dir_all(&org_dir).unwrap();
        std::fs::write(
            org_dir.join("a.org"),
            ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Note A\n\nBody A.\n",
        )
        .unwrap();

        let store = CozoKbStore::open_mem().unwrap();

        // "Killed after A" — only a.org exists yet.
        let (_kb, report1) = mae_kb::federation::import_org_dir_to_store(
            &org_dir,
            &store,
            &mae_kb::federation::IngestMode::Full,
        )
        .unwrap();
        assert_eq!(report1.nodes_imported, 1);
        assert!(report1.errors.is_empty());

        // "Restart, next tick" — b.org has since appeared.
        std::fs::write(
            org_dir.join("b.org"),
            ":PROPERTIES:\n:ID: test:b\n:END:\n#+title: Note B\n\nBody B.\n",
        )
        .unwrap();
        let (_kb, report2) = mae_kb::federation::import_org_dir_to_store(
            &org_dir,
            &store,
            &mae_kb::federation::IngestMode::Full,
        )
        .unwrap();
        assert!(report2.errors.is_empty());

        // Converged correctly: both nodes present, A not duplicated (still
        // exactly one node under id "test:a", correct final content) despite
        // having been imported by two separate passes.
        use mae_kb::KbStore;
        let node_a = store.get_node("test:a").unwrap().unwrap();
        assert_eq!(node_a.title, "Note A");
        let node_b = store.get_node("test:b").unwrap().unwrap();
        assert_eq!(node_b.title, "Note B");
        let report = store.health_report().unwrap();
        assert_eq!(
            report.total_nodes, 2,
            "the partial-then-full reimport sequence must converge to exactly \
             2 nodes, not 1 (dropped) or 3+ (duplicated): {report:?}"
        );
    }

    #[tokio::test]
    async fn watcher_tick_is_a_correct_no_op_when_nothing_is_registered() {
        // The daemon's default, real-world deployment shape today: no
        // org-directory-backed instance registered at all (the primary
        // `daemon-kb.cozo` store is populated via collaborative RPC writes,
        // not org files). `run_watcher_tick` must handle this cleanly — no
        // panic, no spurious reimport attempt — not merely "eventually work
        // once instances exist".
        let config = DaemonConfig::default();
        let daemon_state = Arc::new(Mutex::new(DaemonState::new()));
        let scheduler = DaemonScheduler::new(config, daemon_state);
        let state = Arc::clone(&scheduler.state);

        scheduler.run_watcher_tick(&state).await;

        let s = state.lock().await;
        assert_eq!(s.drain_cycles, 1);
    }
}
