//! KB migration — move nodes between KB instances.
//!
//! Provides functions to migrate nodes from one KB to another by exporting
//! to org-roam-compatible files and ingesting into the target. This bridges
//! the gap between MAE's internal KB and external org-roam directories.
//!
//! ## Use Cases
//!
//! - Move notes from MAE's help KB to a user's personal org-roam KB
//! - Sync nodes from a shared KB to a local one
//! - Export curated KB subsets for backup or sharing
//!
//! ## Org-Roam Compatibility
//!
//! Output files use org-roam naming: `{timestamp}-{slug}.org`
//! with `:PROPERTIES:` drawer containing `:ID:`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::store::{KbStore, KbStoreError};
use crate::{KnowledgeBase, Node, NodeKind};

/// Options for controlling migration behavior.
#[derive(Debug, Clone)]
pub struct MigrateOptions {
    /// Only migrate nodes matching these IDs (empty = all).
    pub node_ids: Vec<String>,
    /// Only migrate nodes with any of these tags (empty = no tag filter).
    pub tags: Vec<String>,
    /// Only migrate nodes matching this ID prefix (e.g. "roadmap:").
    pub id_prefix: Option<String>,
    /// Only migrate these node kinds.
    pub kinds: Vec<NodeKind>,
    /// If true, overwrite existing files with matching IDs.
    pub overwrite: bool,
    /// If true, use org-roam timestamp filenames. If false, use slug-only.
    pub orgroam_naming: bool,
}

impl Default for MigrateOptions {
    fn default() -> Self {
        Self {
            node_ids: Vec::new(),
            tags: Vec::new(),
            id_prefix: None,
            kinds: Vec::new(),
            overwrite: false,
            orgroam_naming: true,
        }
    }
}

/// Report from a migration operation.
#[derive(Debug, Clone, Default)]
pub struct MigrateReport {
    /// Number of nodes successfully written to target.
    pub written: usize,
    /// Number of nodes skipped (already exist, filtered out, etc.).
    pub skipped: usize,
    /// Number of nodes that failed to write.
    pub errors: Vec<(String, String)>,
    /// Paths of files that were written.
    pub files: Vec<PathBuf>,
}

/// Migrate nodes from a source KB to a target org directory.
///
/// Writes each matching node as an org-roam-compatible `.org` file in the
/// target directory. Existing files with matching `:ID:` are skipped unless
/// `options.overwrite` is true.
pub fn migrate_to_org_dir(
    source: &KnowledgeBase,
    target_dir: &Path,
    options: &MigrateOptions,
) -> std::io::Result<MigrateReport> {
    std::fs::create_dir_all(target_dir)?;

    let mut report = MigrateReport::default();
    let nodes = select_nodes(source, options);

    // Scan target dir for existing IDs (to avoid duplicates)
    let existing_ids = if !options.overwrite {
        scan_existing_ids(target_dir)
    } else {
        HashSet::new()
    };

    let base_timestamp = current_timestamp();

    for (i, node) in nodes.iter().enumerate() {
        // Skip if already exists in target
        if existing_ids.contains(&node.id) {
            report.skipped += 1;
            continue;
        }

        let content = node_to_orgroam(node);
        let filename = if options.orgroam_naming {
            let ts = increment_timestamp(&base_timestamp, i);
            let slug = slugify(&node.title);
            format!("{ts}-{slug}.org")
        } else {
            let slug = sanitize_id(&node.id);
            format!("{slug}.org")
        };

        let path = target_dir.join(&filename);
        match std::fs::write(&path, &content) {
            Ok(()) => {
                report.written += 1;
                report.files.push(path);
            }
            Err(e) => {
                report.errors.push((node.id.clone(), e.to_string()));
            }
        }
    }

    Ok(report)
}

/// Migrate nodes from a source org directory to a target org directory.
///
/// Reads nodes from source, filters by options, writes to target.
/// Useful for migrating between two org-roam directories (e.g., MAE help → personal).
pub fn migrate_org_to_org(
    source_dir: &Path,
    target_dir: &Path,
    options: &MigrateOptions,
) -> std::io::Result<MigrateReport> {
    let mut kb = KnowledgeBase::new();
    kb.ingest_org_dir(source_dir);
    migrate_to_org_dir(&kb, target_dir, options)
}

/// Select nodes from KB based on migration options.
fn select_nodes<'a>(kb: &'a KnowledgeBase, options: &MigrateOptions) -> Vec<&'a Node> {
    let all_ids = kb.list_ids(None);
    let tag_set: HashSet<&str> = options.tags.iter().map(|s| s.as_str()).collect();
    let id_set: HashSet<&str> = options.node_ids.iter().map(|s| s.as_str()).collect();

    all_ids
        .iter()
        .filter_map(|id| {
            let node = kb.get(id)?;

            // Filter by explicit ID list
            if !id_set.is_empty() && !id_set.contains(id.as_str()) {
                return None;
            }

            // Filter by ID prefix
            if let Some(ref prefix) = options.id_prefix {
                if !id.starts_with(prefix.as_str()) {
                    return None;
                }
            }

            // Filter by tags (any match)
            if !tag_set.is_empty() && !node.tags.iter().any(|t| tag_set.contains(t.as_str())) {
                return None;
            }

            // Filter by kind
            if !options.kinds.is_empty() && !options.kinds.contains(&node.kind) {
                return None;
            }

            Some(node)
        })
        .collect()
}

/// Convert a node to org-roam format (with proper `:PROPERTIES:` drawer).
fn node_to_orgroam(node: &Node) -> String {
    let mut out = String::new();

    out.push_str(":PROPERTIES:\n");
    out.push_str(&format!(":ID: {}\n", node.id));
    for (k, v) in &node.properties {
        // Skip internal properties
        if k == "id" {
            continue;
        }
        out.push_str(&format!(":{}: {}\n", k.to_lowercase(), v));
    }
    out.push_str(":END:\n");

    out.push_str(&format!("#+title: {}\n", node.title));

    if !node.tags.is_empty() {
        out.push_str(&format!("#+filetags: :{}:\n", node.tags.join(":")));
    }

    out.push('\n');

    // Body is already in org format for nodes parsed from org files
    out.push_str(&node.body);
    if !node.body.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Scan an org directory for existing node IDs.
fn scan_existing_ids(dir: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return ids;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            // Quick parse: look for :ID: line in first 10 lines
            for line in content.lines().take(10) {
                let trimmed = line.trim();
                if let Some(id) = trimmed.strip_prefix(":ID:") {
                    ids.insert(id.trim().to_string());
                    break;
                }
            }
        }
    }

    ids
}

/// Get current timestamp in org-roam format (YYYYMMDDHHmmss).
fn current_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    // Convert to datetime components (simplified, no chrono dependency)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y-M-D (simplified leap year handling)
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}{month:02}{day:02}{hours:02}{minutes:02}{seconds:02}")
}

/// Increment a timestamp string by N seconds.
fn increment_timestamp(base: &str, offset: usize) -> String {
    if base.len() != 14 {
        return format!("{base}{offset:02}");
    }
    // Just increment the last two digits (seconds)
    let prefix = &base[..12];
    let secs: u32 = base[12..14].parse().unwrap_or(0);
    let new_secs = (secs as usize + offset) % 60;
    format!("{prefix}{new_secs:02}")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Approximate: 365.2425 days/year
    let mut year = 1970;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let month_days: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }

    (year, month, days + 1)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Slugify a title for filenames (lowercase, spaces to underscores).
fn slugify(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Sanitize an ID for use as a filename.
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            ':' | '/' | '\\' | ' ' => '-',
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => c,
            _ => '-',
        })
        .collect()
}

/// Result of migrating between KbStore backends.
#[derive(Debug, Default)]
pub struct StoreMigrationReport {
    pub nodes_migrated: usize,
    pub links_migrated: usize,
    pub pending_migrated: usize,
    pub errors: Vec<String>,
}

/// Migrate all data from one KbStore backend to another.
///
/// Copies all nodes (including CRDT docs), links, and pending updates.
/// The destination store is cleared before migration.
pub fn migrate_between_stores(
    src: &dyn KbStore,
    dst: &dyn KbStore,
) -> Result<StoreMigrationReport, KbStoreError> {
    let mut report = StoreMigrationReport::default();

    // Load all nodes from source
    let nodes = src.load_all()?;
    let node_refs: Vec<&Node> = nodes.iter().collect();

    // Save all to destination (clears first)
    dst.replace_all_nodes(&node_refs)?;
    report.nodes_migrated = nodes.len();

    // Migrate links (already handled by replace_all_nodes, which parses bodies,
    // but we also need any manually-added links)
    for node in &nodes {
        for link in src.links_from(&node.id)? {
            // Links are already created by replace_all_nodes' body parsing,
            // but add_link is idempotent so this catches any extras
            if let Err(e) = dst.add_link(&link.src, &link.dst, link.display.as_deref()) {
                report
                    .errors
                    .push(format!("link {}→{}: {e}", link.src, link.dst));
            } else {
                report.links_migrated += 1;
            }
        }
    }

    // Migrate pending updates
    let pending = src.drain_pending_updates()?;
    for pu in &pending {
        if let Err(e) = dst.push_pending_update(&pu.kb_id, &pu.node_id, &pu.update_bytes) {
            report
                .errors
                .push(format!("pending {}/{}: {e}", pu.kb_id, pu.node_id));
        } else {
            report.pending_migrated += 1;
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Backend migration: sled → sqlite (one-time, reversible)
// ---------------------------------------------------------------------------

/// Result of a [`migrate_sled_to_sqlite`] attempt.
#[derive(Debug)]
pub enum SledToSqliteOutcome {
    /// `path` is not a sled store (already sqlite, or a fresh install) — no-op.
    NotNeeded,
    /// A sled store was converted to sqlite. The old sled directory is preserved at
    /// `backup` (reversible; never deleted).
    Migrated {
        nodes: usize,
        links: usize,
        backup: PathBuf,
    },
}

/// Migrate a cozo **sled** store at `path` (a directory) to a cozo **sqlite** store
/// at the same `path` (a file). This is the one-time conversion that lets N
/// daemon-less processes share one KB store (sled takes an exclusive dir lock;
/// sqlite/WAL allows multiple processes — see `CozoKbStore`'s busy-retry).
///
/// Safety:
/// - **Atomic-ish**: the sqlite store is built at a temp path, then the sled dir is
///   renamed to `<path>.sled.bak-<ts>` and the temp renamed into place; a failure at
///   the final step restores the sled dir.
/// - **Never destructive**: the sled data is *renamed* to a backup, never deleted.
/// - **Idempotent**: a sqlite file (or an absent path) returns `NotNeeded`.
pub fn migrate_sled_to_sqlite(path: &Path) -> Result<SledToSqliteOutcome, KbStoreError> {
    // A sled store is a DIRECTORY; a sqlite store is a FILE. Only a directory needs
    // migrating — this doubles as the idempotency check (post-migration = a file).
    if !path.is_dir() {
        return Ok(SledToSqliteOutcome::NotNeeded);
    }

    // 1. Read everything out of the sled store, then release its exclusive dir lock
    //    (the `sled` handle drops at the end of this block, BEFORE we rename the dir).
    let (nodes, links) = {
        let sled = crate::CozoKbStore::open_with_engine(path, "sled")?;
        // `load_all` tolerates short-arity rows (the corrupt-store repair path), so a
        // partially-damaged sled store still migrates what it can.
        let nodes = sled.load_all()?;
        let links = sled.load_all_links()?;
        (nodes, links)
    };

    // 2. Build a fresh sqlite store at a temp path alongside the target. Bulk-import
    //    in ONE transaction (one fsync) so a large KB migrates in ~a second instead of
    //    minutes; `bulk_import` writes links verbatim (no body re-derivation), so
    //    AI-authored / non-`related_to` edges survive. Clean up the temp on failure.
    let tmp = suffixed(path, ".sqlite.tmp");
    let _ = std::fs::remove_file(&tmp); // clear a stale temp from any prior aborted run
    let build = || -> Result<(usize, usize), KbStoreError> {
        let sqlite = crate::CozoKbStore::open_with_engine(&tmp, "sqlite")?;
        sqlite.seed_type_system()?;
        let _ = sqlite.seed_typed_relationships();
        let _ = sqlite.seed_views();
        sqlite.bulk_import(&nodes, &links)
        // `sqlite` drops here → the temp store is complete + consistent on disk.
    };
    let (n_nodes, n_links) = match build() {
        Ok(counts) => counts,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };

    // 3. Atomic-ish swap: sled dir → timestamped backup, then temp → canonical path.
    //    If the final rename fails, restore the sled dir so the editor still opens it.
    let backup = suffixed(path, &format!(".sled.bak-{}", unix_ts()));
    std::fs::rename(path, &backup)
        .map_err(|e| KbStoreError::Storage(format!("sled→sqlite: backup rename failed: {e}")))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::rename(&backup, path); // roll back to the sled store
        let _ = std::fs::remove_file(&tmp);
        return Err(KbStoreError::Storage(format!(
            "sled→sqlite: final rename failed (sled store restored): {e}"
        )));
    }

    Ok(SledToSqliteOutcome::Migrated {
        nodes: n_nodes,
        links: n_links,
        backup,
    })
}

/// Append a suffix to a path's file name (preserves the parent directory).
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
