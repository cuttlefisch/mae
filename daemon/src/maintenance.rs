//! Deterministic maintenance scan — the non-AI half of the daemon's
//! `maintenance_tick` (ADR-065 item 2). Companion to `hygiene.rs`'s `health_tick`
//! scan: this covers periodic stats logging plus a bulk content-hash integrity
//! check. AI-driven enrichment is a separate, larger capability (ADR-061 Phase C)
//! that claims the *other* half of this same tick — see `scheduler.rs`'s comment
//! at the `maintenance_tick` arm for the coordination boundary between the two.
//!
//! Compaction (a VACUUM-equivalent) is deliberately NOT implemented here: Cozo's
//! Rust API is Datalog-only and exposes no compaction primitive for either its
//! sqlite or sled storage engines, and reaching around Cozo to issue a raw
//! `VACUUM` against its backing SQLite file risks violating invariants Cozo's own
//! storage layer expects to hold — a real, unverified risk, not a stylistic
//! omission. Left for a follow-up once a safe primitive exists (either upstream
//! from Cozo, or a specifically adversarially-tested design of its own) rather
//! than shipped as an unverified guess.

use mae_kb::store::HealthReport;
use mae_kb::{CozoKbStore, KbStore};
use std::sync::Arc;

/// Result of one store's deterministic maintenance pass.
#[derive(Debug, Default)]
pub struct MaintenanceResult {
    /// Node count as of this pass (from `stats`, duplicated here for convenient
    /// logging without unwrapping `stats`).
    pub nodes_checked: usize,
    /// IDs whose latest version's `content_hash` no longer matches its fields —
    /// tamper or corruption evidence (`NodeVersion::verify_integrity`).
    pub integrity_failures: Vec<String>,
    /// The periodic stats snapshot (same `HealthReport` shape `health_tick`'s
    /// hygiene scan and `kb_health` already use) — `None` only if the
    /// `health_report()` call itself failed.
    pub stats: Option<HealthReport>,
    /// Non-fatal errors encountered (e.g. a single node's history lookup
    /// failing does not abort the rest of the scan).
    pub errors: Vec<String>,
}

/// Run the deterministic maintenance pass against one store: a stats snapshot
/// (reuses the same `health_report()` the `health_tick` hygiene scan and
/// `kb_health` tool already call) plus an integrity check over every node's
/// latest recorded version. Honest scoping: version history (`node_history`)
/// is populated only for nodes that have been explicitly snapshotted at least
/// once (`snapshot_version` — called today from the destructive-ingest path
/// (`insert_node_with_history`, ADR-106) and the restore path, not from every
/// `kb_create`/`kb_update`), so a node with no recorded
/// history is silently skipped here rather than treated as a false-positive
/// integrity failure. Read-only — never mutates the store, so it is always
/// safe to run concurrently with reads and safe to re-run after an
/// interrupted pass (there is no partial-write state to reconcile).
pub fn run_maintenance_scan(store: &Arc<CozoKbStore>) -> MaintenanceResult {
    let mut result = MaintenanceResult::default();

    match store.health_report() {
        Ok(report) => {
            result.nodes_checked = report.total_nodes;
            result.stats = Some(report);
        }
        Err(e) => {
            result.errors.push(format!("health_report failed: {e}"));
        }
    }

    let ids = match store.list_ids(None) {
        Ok(ids) => ids,
        Err(e) => {
            result.errors.push(format!("list_ids failed: {e}"));
            return result;
        }
    };

    for id in ids {
        match store.node_history(&id, 1) {
            Ok(versions) => {
                if let Some(latest) = versions.first() {
                    if !latest.verify_integrity() {
                        result.integrity_failures.push(id);
                    }
                }
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("node_history failed for {id}: {e}"));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_kb::{Node, NodeKind};

    #[test]
    fn maintenance_scan_reports_stats_for_every_node_and_checks_snapshotted_ones() {
        // Note: the integrity check walks `node_history`, which is populated
        // only for nodes that have been explicitly snapshotted at least once
        // (`snapshot_version` — see the doc comment on `run_maintenance_scan`).
        // Snapshot both nodes here so the "zero failures" assertion below is
        // actually verifying something, not vacuously true because nothing
        // had any history to check.
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        store
            .insert_node(&Node::new("note:a", "Alpha", NodeKind::Note, "body a"))
            .unwrap();
        store
            .insert_node(&Node::new("note:b", "Beta", NodeKind::Note, "body b"))
            .unwrap();
        store.snapshot_version("note:a", "initial").unwrap();
        store.snapshot_version("note:b", "initial").unwrap();

        let result = run_maintenance_scan(&store);
        assert_eq!(result.nodes_checked, 2);
        assert!(result.stats.is_some());
        assert!(
            result.integrity_failures.is_empty(),
            "freshly-snapshotted, untampered versions must report zero integrity failures"
        );
        assert!(result.errors.is_empty());
    }

    #[test]
    fn maintenance_scan_skips_nodes_with_no_recorded_version_history() {
        // Honest-scoping test: a node that was never snapshotted has no
        // `node_history` to check, so it must be silently skipped by the
        // integrity check (not reported as a false-positive failure) while
        // still being counted in `nodes_checked` via the stats snapshot.
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        store
            .insert_node(&Node::new(
                "note:unsnapshotted",
                "Gamma",
                NodeKind::Note,
                "c",
            ))
            .unwrap();

        let result = run_maintenance_scan(&store);
        assert_eq!(result.nodes_checked, 1);
        assert!(result.integrity_failures.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn maintenance_scan_is_stable_across_repeated_runs() {
        // ADR-065 item 2's DoD: "kill mid-tick, resume must not double-apply
        // completed work". `run_maintenance_scan` is read-only (documented
        // above), so an interrupted pass leaves no partial-write state — the
        // property this proves directly is that re-running it (as the next
        // scheduled tick would after an interruption discarded the first
        // pass's in-flight result) produces identical counts, not
        // accumulated/doubled ones from some hidden mutable state.
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        store
            .insert_node(&Node::new("note:a", "Alpha", NodeKind::Note, "body a"))
            .unwrap();
        store.snapshot_version("note:a", "initial").unwrap();

        let first = run_maintenance_scan(&store);
        let second = run_maintenance_scan(&store);

        assert_eq!(first.nodes_checked, second.nodes_checked);
        assert_eq!(
            first.integrity_failures, second.integrity_failures,
            "repeated scans of unchanged content must report identical results, \
             never accumulating across calls"
        );
        assert_eq!(first.nodes_checked, 1);
    }
}
