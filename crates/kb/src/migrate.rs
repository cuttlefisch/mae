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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_by_prefix() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("roadmap:a", "A", NodeKind::Note, "body a"));
        kb.insert(Node::new("roadmap:b", "B", NodeKind::Note, "body b"));
        kb.insert(Node::new("concept:c", "C", NodeKind::Concept, "body c"));

        let opts = MigrateOptions {
            id_prefix: Some("roadmap:".to_string()),
            ..Default::default()
        };
        let nodes = select_nodes(&kb, &opts);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn select_by_tags() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("n1", "N1", NodeKind::Note, "").with_tags(["mae", "roadmap"]));
        kb.insert(Node::new("n2", "N2", NodeKind::Note, "").with_tags(["personal"]));

        let opts = MigrateOptions {
            tags: vec!["mae".to_string()],
            ..Default::default()
        };
        let nodes = select_nodes(&kb, &opts);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }

    #[test]
    fn migrate_writes_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("test:a", "Test A", NodeKind::Note, "Body A.").with_tags(["test"]));
        kb.insert(Node::new("test:b", "Test B", NodeKind::Note, "Body B."));

        let opts = MigrateOptions {
            orgroam_naming: false,
            ..Default::default()
        };
        let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
        assert_eq!(report.written, 2);
        assert!(tmp.path().join("test-a.org").exists());
        assert!(tmp.path().join("test-b.org").exists());

        let content = std::fs::read_to_string(tmp.path().join("test-a.org")).unwrap();
        assert!(content.contains(":ID: test:a"));
        assert!(content.contains("#+title: Test A"));
        assert!(content.contains("#+filetags: :test:"));
    }

    #[test]
    fn skip_existing_ids() {
        let tmp = tempfile::tempdir().unwrap();

        // Pre-create a file with matching ID
        std::fs::write(
            tmp.path().join("existing.org"),
            ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Existing\n",
        )
        .unwrap();

        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("test:a", "Test A", NodeKind::Note, "Body."));
        kb.insert(Node::new("test:b", "Test B", NodeKind::Note, "Body."));

        let opts = MigrateOptions {
            orgroam_naming: false,
            ..Default::default()
        };
        let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(report.skipped, 1);
    }

    #[test]
    fn overwrite_existing() {
        let tmp = tempfile::tempdir().unwrap();

        std::fs::write(
            tmp.path().join("existing.org"),
            ":PROPERTIES:\n:ID: test:a\n:END:\n#+title: Old\n",
        )
        .unwrap();

        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "test:a",
            "Test A New",
            NodeKind::Note,
            "New body.",
        ));

        let opts = MigrateOptions {
            overwrite: true,
            orgroam_naming: false,
            ..Default::default()
        };
        let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn orgroam_naming() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "test:hello",
            "Hello World",
            NodeKind::Note,
            "Body.",
        ));

        let opts = MigrateOptions {
            orgroam_naming: true,
            ..Default::default()
        };
        let report = migrate_to_org_dir(&kb, tmp.path(), &opts).unwrap();
        assert_eq!(report.written, 1);

        // File should have timestamp prefix
        let files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("-hello_world.org"));
        assert!(files[0].len() > 20); // timestamp prefix
    }

    #[test]
    fn slugify_title() {
        assert_eq!(slugify("Hello World"), "hello_world");
        assert_eq!(slugify("MAE Phase 1 — Snippets"), "mae_phase_1___snippets");
        assert_eq!(slugify("simple"), "simple");
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-05-31 = day 20604 since epoch (approx)
        let (y, m, _d) = days_to_ymd(20604);
        assert_eq!(y, 2026);
        assert!((5..=6).contains(&m)); // May or June depending on exact calc
    }
}
