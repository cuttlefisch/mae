//! KB federation operations: register, unregister, reimport.

use std::path::{Path, PathBuf};

use mae_kb::federation::{ImportHealth, ImportReport};
use mae_kb::KbStore;

use super::Editor;

pub use registry::KbResolution;

/// The honest, point-of-action advisory shown when a user enables E2E content
/// encryption on a KB (CF1, `docs/SECURITY_REVIEW.md §6.3`). "E2E" connotes
/// Signal-like privacy; MAE's model protects node *content* from non-members but
/// does NOT provide forward secrecy / PCS, hide metadata, or retroactively protect
/// already-shared plaintext. Surfacing this at enable-time — not only in
/// `docs/E2E_ENCRYPTION.md §7` — keeps the label from overselling. Kept as one
/// shared const so the enable surface, the `*KB Sharing*` buffer, and the Scheme
/// primitive doc all say the same thing (CLAUDE.md #3).
pub const E2E_ENABLE_ADVISORY: &str = "\
E2E content encryption is now ENABLED on this KB (one-way — it cannot be disabled).

What it protects: node CONTENT (titles + bodies) is sealed so the daemon/relay and \
non-members see only ciphertext.

What it does NOT protect (be aware before relying on it):
  • No forward secrecy / post-compromise security — a leaked key opens past AND future content.
  • Metadata is visible to the host: who is in the KB, who admitted whom, which node each \
edit touches, when, by whom, and the size of each edit — just not the content.
  • Node IDs remain cleartext in the collection manifest (titles are blanked).
  • It is NOT retroactive: anything already shared as plaintext stays on the relay until \
re-sealed — enable BEFORE sharing for full protection.
  • If you lose your identity key you lose access permanently — back it up.

See :help concept:kb-encryption and docs/E2E_ENCRYPTION.md §7 for the full model.";

/// Cumulative statistics for KB watcher drain operations.
#[derive(Debug, Default)]
pub struct KbWatcherStats {
    /// Total nodes upserted via watcher drain.
    pub events_upserted: u64,
    /// Total nodes removed via watcher drain.
    pub events_removed: u64,
    /// Events skipped due to debounce (too recent).
    pub suppressed_debounce: u64,
    /// Events skipped due to 50ms timebox deadline.
    pub suppressed_timebox: u64,
    /// Events suppressed by write-guard (MAE-initiated writes).
    pub events_suppressed: u64,
    /// Total reimport calls from all sources (save, watcher, explicit).
    pub reimports_total: u64,
    /// Watcher errors encountered.
    pub errors: u64,
    /// Durable-store write-through failures during watcher/reimport drain.
    pub store_write_errors: u64,
    /// Duration of the last drain operation in microseconds.
    pub last_drain_us: u64,
    /// Number of events processed in the last drain.
    pub last_drain_event_count: usize,
    /// Cumulative drain microseconds (for computing avg).
    pub drain_us_sum: u64,
    /// Number of drain cycles that processed at least one event.
    pub drain_count: u64,
}

/// Result of a KB registration or reimport operation.
#[derive(Debug, Clone)]
pub struct KbImportResult {
    pub name: String,
    pub uuid: String,
    pub report: ImportReport,
    pub health: ImportHealth,
}

/// Result of promoting a federated/imported node into the primary KB
/// (#303's interim bridge toward issue #111 / ADR-029's "org dirs are
/// import-only" direction — see `Editor::kb_promote_node`).
#[derive(Debug, Clone)]
pub struct KbPromoteResult {
    pub node_id: String,
    pub promoted_from_uuid: String,
    pub promoted_from_org_dir: PathBuf,
    pub dedup: PromoteDedup,
}

/// Outcome of deduplicating the origin instance's now-redundant copy of a
/// just-promoted node (`Editor::kb_dedup_promoted_instance_copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteDedup {
    /// The instance's copy was identical (expected immediately post-promote)
    /// and has been removed — exactly one copy remains, in `primary`.
    Removed,
    /// The instance's copy has diverged from what was promoted; both copies
    /// were preserved and an `action_required` notification was fired.
    KeptDiverged,
}

impl KbImportResult {
    /// Format as a user-facing status message.
    pub fn status_summary(&self) -> String {
        let mut s = format!(
            "Registered '{}': {} nodes, {} links",
            self.name, self.report.nodes_imported, self.report.links_created,
        );
        if self.report.nodes_updated > 0 {
            s.push_str(&format!(", {} updated", self.report.nodes_updated));
        }
        if self.report.nodes_unchanged > 0 {
            s.push_str(&format!(", {} unchanged", self.report.nodes_unchanged));
        }
        if self.report.nodes_removed > 0 {
            s.push_str(&format!(", {} removed", self.report.nodes_removed));
        }
        s.push_str(&format!(
            " | Health: {} orphans, {} broken links",
            self.health.orphan_count, self.health.broken_link_count,
        ));
        if !self.report.duplicate_ids.is_empty() {
            s.push_str(&format!(
                ", {} duplicate IDs",
                self.report.duplicate_ids.len()
            ));
        }
        if self.report.nodes_skipped > 0 {
            s.push_str(&format!(
                ", {} files without :ID:",
                self.report.nodes_skipped
            ));
        }
        if !self.report.errors.is_empty() {
            s.push_str(&format!(", {} read errors", self.report.errors.len()));
        }
        if self.report.duration_ms > 0 {
            s.push_str(&format!(" ({}ms)", self.report.duration_ms));
        }
        s
    }

    /// Format as structured JSON for the AI agent.
    pub fn to_json(&self) -> String {
        let ns_counts: Vec<String> = self
            .health
            .namespace_counts
            .iter()
            .map(|(k, v)| format!("    \"{}\": {}", k, v))
            .collect();

        format!(
            concat!(
                "{{\n",
                "  \"name\": \"{}\",\n",
                "  \"uuid\": \"{}\",\n",
                "  \"nodes_imported\": {},\n",
                "  \"links_created\": {},\n",
                "  \"files_without_id\": {},\n",
                "  \"duplicate_ids\": {},\n",
                "  \"read_errors\": {},\n",
                "  \"health\": {{\n",
                "    \"total_nodes\": {},\n",
                "    \"total_links\": {},\n",
                "    \"orphan_count\": {},\n",
                "    \"broken_link_count\": {},\n",
                "    \"namespace_counts\": {{\n{}\n    }}\n",
                "  }}\n",
                "}}"
            ),
            self.name,
            self.uuid,
            self.report.nodes_imported,
            self.report.links_created,
            self.report.nodes_skipped,
            self.report.duplicate_ids.len(),
            self.report.errors.len(),
            self.health.total_nodes,
            self.health.total_links,
            self.health.orphan_count,
            self.health.broken_link_count,
            ns_counts.join(",\n"),
        )
    }
}

mod activity;
mod daily;
mod dispatch;
mod nodes;
mod registry;
mod search;
mod sync;
mod watchers;

/// Result of a dailies chain-fill operation.
pub struct ChainFillResult {
    pub stubs_created: Vec<(i32, u32, u32)>,
    pub links_inserted: usize,
    pub anchor_date: Option<(i32, u32, u32)>,
}

/// Current date as YYYY-MM-DD using proper calendar math.
fn today_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d) = unix_secs_to_date(secs);
    mae_kb::activity::format_date(y, m, d)
}

/// Current date as (year, month, day). Used by dailies, activity sorting.
pub fn today_ymd() -> (i32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    unix_secs_to_date(secs)
}

/// Convert Unix epoch seconds to (year, month, day).
/// Civil calendar conversion without chrono.
fn unix_secs_to_date(secs: u64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant's civil_from_days
    let z = (secs / 86400) as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Simple ISO-8601 timestamp without pulling in chrono.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Approximate: good enough for display purposes
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remainder_days = days % 365;
    let months = remainder_days / 30 + 1;
    let day = remainder_days % 30 + 1;
    format!("{:04}-{:02}-{:02}", years, months, day)
}

#[cfg(test)]
mod tests;
