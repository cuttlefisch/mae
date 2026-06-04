//! Hygiene engine — deterministic KB quality assessment.
//!
//! Runs periodically via the daemon scheduler. Detects:
//! - Orphan nodes (no incoming or outgoing links)
//! - Broken links (target doesn't exist)
//! - Kind/namespace mismatches (ID prefix doesn't match node kind)
//! - Missing metadata (tasks without priority, etc.)
//!
//! AI-powered suggestions (link type inference, missing link detection)
//! are planned for a future release.

use mae_kb::hygiene::{CAT_BROKEN_LINK, CAT_KIND_MISMATCH, CAT_MISSING_METADATA, CAT_ORPHAN};
use mae_kb::CozoKbStore;
use std::sync::Arc;

/// Result of a hygiene scan.
#[derive(Debug, Default)]
pub struct HygieneScanResult {
    /// Number of new suggestions created.
    pub suggestions_created: usize,
    /// Number of nodes scanned.
    pub nodes_scanned: usize,
    /// Errors encountered (non-fatal).
    pub errors: Vec<String>,
}

/// Run a full hygiene scan against the given store.
///
/// This performs deterministic checks only (no AI calls). Suggestions
/// are inserted into the `hygiene_suggestions` CozoDB relation with
/// `status = "pending"`.
pub fn run_hygiene_scan(store: &Arc<CozoKbStore>) -> HygieneScanResult {
    let mut result = HygieneScanResult::default();

    // Get health report for orphans and broken links
    let report = match store.health_report() {
        Ok(r) => r,
        Err(e) => {
            result
                .errors
                .push(format!("Failed to get health report: {e}"));
            return result;
        }
    };

    result.nodes_scanned = report.total_nodes;

    // --- Orphan detection ---
    for orphan_id in &report.orphan_ids {
        // Skip system nodes (index, meta) — they're often intentionally unlinked
        if orphan_id.starts_with("index:") || orphan_id.starts_with("meta:") {
            continue;
        }
        match store.has_suggestion(orphan_id, CAT_ORPHAN) {
            Ok(true) => continue, // Already flagged
            Ok(false) => {}
            Err(e) => {
                result
                    .errors
                    .push(format!("has_suggestion check failed for {orphan_id}: {e}"));
                continue;
            }
        }
        let msg = format!("Node '{orphan_id}' has no incoming or outgoing links");
        match store.insert_suggestion(orphan_id, CAT_ORPHAN, &msg, "{}", 1.0) {
            Ok(_) => result.suggestions_created += 1,
            Err(e) => result.errors.push(format!(
                "Failed to insert orphan suggestion for {orphan_id}: {e}"
            )),
        }
    }

    // --- Broken link detection ---
    for broken in &report.broken_links {
        let src = &broken.source;
        match store.has_suggestion(src, CAT_BROKEN_LINK) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                result
                    .errors
                    .push(format!("has_suggestion check failed for {src}: {e}"));
                continue;
            }
        }
        let msg = format!(
            "Link from '{src}' to '{}' is broken: {:?}",
            broken.target, broken.reason
        );
        let action = serde_json::json!({
            "action": "remove_link",
            "src": src,
            "dst": broken.target,
        })
        .to_string();
        match store.insert_suggestion(src, CAT_BROKEN_LINK, &msg, &action, 1.0) {
            Ok(_) => result.suggestions_created += 1,
            Err(e) => result.errors.push(format!(
                "Failed to insert broken_link suggestion for {src}: {e}"
            )),
        }
    }

    // --- Kind/namespace mismatch detection ---
    detect_kind_mismatches(store, &mut result);

    // --- Missing metadata detection ---
    detect_missing_metadata(store, &mut result);

    result
}

/// Detect nodes where the ID prefix doesn't match the node's kind.
fn detect_kind_mismatches(store: &CozoKbStore, result: &mut HygieneScanResult) {
    use mae_kb::KbStore;

    let ids = match store.list_ids(None) {
        Ok(ids) => ids,
        Err(e) => {
            result
                .errors
                .push(format!("Kind mismatch: list_ids failed: {e}"));
            return;
        }
    };

    for id in &ids {
        let node = match store.get_node(id) {
            Ok(Some(n)) => n,
            _ => continue,
        };

        let kind_str = node.kind.as_str();
        let expected_prefix = kind_to_prefix(kind_str);
        if let Some(prefix) = expected_prefix {
            if let Some(actual_prefix) = id.split(':').next() {
                if actual_prefix != prefix && !id.starts_with("user:") {
                    match store.has_suggestion(id, CAT_KIND_MISMATCH) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(_) => continue,
                    }
                    let msg = format!(
                        "Node '{id}' has kind '{kind_str}' but ID prefix '{actual_prefix}' \
                         (expected '{prefix}')"
                    );
                    let action = serde_json::json!({
                        "action": "set_field",
                        "node_id": id,
                        "field": "kind",
                        "suggested_value": prefix_to_kind(actual_prefix),
                    })
                    .to_string();
                    match store.insert_suggestion(id, CAT_KIND_MISMATCH, &msg, &action, 0.9) {
                        Ok(_) => result.suggestions_created += 1,
                        Err(e) => result.errors.push(format!(
                            "Failed to insert kind_mismatch suggestion for {id}: {e}"
                        )),
                    }
                }
            }
        }
    }
}

/// Detect nodes with missing required metadata for their kind.
fn detect_missing_metadata(store: &CozoKbStore, result: &mut HygieneScanResult) {
    use mae_kb::KbStore;

    // Check task nodes for missing priority
    let ids = match store.list_ids(Some("task:")) {
        Ok(ids) => ids,
        Err(e) => {
            result
                .errors
                .push(format!("Missing metadata: list_ids failed: {e}"));
            return;
        }
    };

    for id in &ids {
        let node = match store.get_node(id) {
            Ok(Some(n)) => n,
            _ => continue,
        };

        if node.priority.is_none() {
            match store.has_suggestion(id, CAT_MISSING_METADATA) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => continue,
            }
            let msg = format!("Task '{id}' has no priority set");
            let action = serde_json::json!({
                "action": "set_field",
                "node_id": id,
                "field": "priority",
                "value": "C",
            })
            .to_string();
            match store.insert_suggestion(id, CAT_MISSING_METADATA, &msg, &action, 0.8) {
                Ok(_) => result.suggestions_created += 1,
                Err(e) => result.errors.push(format!(
                    "Failed to insert missing_metadata suggestion for {id}: {e}"
                )),
            }
        }
    }
}

/// Map node kind string (lowercase, as stored in CozoDB) to expected ID prefix.
fn kind_to_prefix(kind: &str) -> Option<&str> {
    match kind {
        "command" => Some("cmd"),
        "concept" => Some("concept"),
        "lesson" => Some("lesson"),
        "tutorial" => Some("tutorial"),
        "category" => Some("category"),
        "task" => Some("task"),
        "view" => Some("view"),
        "index" => Some("index"),
        "meta" => Some("meta"),
        "scheme_api" => Some("scheme"),
        "key" => Some("key"),
        "block" => Some("block"),
        // note and project don't have enforced prefixes
        _ => None,
    }
}

/// Map ID prefix to likely kind string (lowercase).
fn prefix_to_kind(prefix: &str) -> &str {
    match prefix {
        "cmd" => "command",
        "concept" => "concept",
        "lesson" => "lesson",
        "tutorial" => "tutorial",
        "category" => "category",
        "task" => "task",
        "view" => "view",
        "index" => "index",
        "meta" => "meta",
        "scheme" => "scheme_api",
        "key" => "key",
        "block" => "block",
        _ => "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_kb::KbStore;

    #[test]
    fn kind_prefix_roundtrip() {
        assert_eq!(kind_to_prefix("command"), Some("cmd"));
        assert_eq!(kind_to_prefix("concept"), Some("concept"));
        assert_eq!(kind_to_prefix("note"), None);
        assert_eq!(prefix_to_kind("cmd"), "command");
        assert_eq!(prefix_to_kind("unknown"), "note");
    }

    #[test]
    fn hygiene_scan_on_empty_store() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        let result = run_hygiene_scan(&store);
        assert_eq!(result.nodes_scanned, 0);
        assert_eq!(result.suggestions_created, 0);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn hygiene_detects_orphan() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        let node = mae_kb::Node::new(
            "concept:lonely",
            "Lonely",
            mae_kb::NodeKind::Concept,
            "No links here",
        );
        store.insert_node(&node).unwrap();

        let result = run_hygiene_scan(&store);
        assert_eq!(result.suggestions_created, 1);

        let suggestions = store.list_suggestions(Some("orphan"), None).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].node_id, "concept:lonely");
        assert_eq!(suggestions[0].category, "orphan");
        assert_eq!(suggestions[0].status, "pending");
    }

    #[test]
    fn hygiene_detects_broken_link() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        let node = mae_kb::Node::new(
            "concept:a",
            "A",
            mae_kb::NodeKind::Concept,
            "See [[concept:nonexistent]]",
        );
        store.insert_node(&node).unwrap();

        let result = run_hygiene_scan(&store);
        // Should detect broken link (concept:a → concept:nonexistent)
        let suggestions = store.list_suggestions(Some("broken_link"), None).unwrap();
        assert!(
            !suggestions.is_empty() || result.suggestions_created > 0,
            "expected at least one broken_link or orphan suggestion"
        );
    }

    #[test]
    fn hygiene_detects_kind_mismatch() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        // ID says "lesson" but kind is Concept
        let node = mae_kb::Node::new(
            "lesson:wrong",
            "Wrong Kind",
            mae_kb::NodeKind::Concept,
            "body",
        );
        store.insert_node(&node).unwrap();
        // Give it a link so it's not flagged as orphan too
        let other = mae_kb::Node::new(
            "concept:other",
            "Other",
            mae_kb::NodeKind::Concept,
            "See [[lesson:wrong]]",
        );
        store.insert_node(&other).unwrap();

        let result = run_hygiene_scan(&store);
        let suggestions = store.list_suggestions(Some("kind_mismatch"), None).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].node_id, "lesson:wrong");
        assert!(result.suggestions_created >= 1);
    }

    #[test]
    fn suggestion_status_lifecycle() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        let sid = store
            .insert_suggestion("concept:test", "orphan", "Test orphan", "{}", 1.0)
            .unwrap();

        let pending = store.list_suggestions(None, Some("pending")).unwrap();
        assert_eq!(pending.len(), 1);

        store
            .update_suggestion_status("concept:test", sid, "accepted")
            .unwrap();

        let pending = store.list_suggestions(None, Some("pending")).unwrap();
        assert_eq!(pending.len(), 0);

        let accepted = store.list_suggestions(None, Some("accepted")).unwrap();
        assert_eq!(accepted.len(), 1);
    }

    #[test]
    fn no_duplicate_suggestions() {
        let store = Arc::new(CozoKbStore::open_mem().unwrap());
        let node = mae_kb::Node::new(
            "concept:lonely",
            "Lonely",
            mae_kb::NodeKind::Concept,
            "No links",
        );
        store.insert_node(&node).unwrap();

        let r1 = run_hygiene_scan(&store);
        assert_eq!(r1.suggestions_created, 1);

        // Second scan should not create duplicates
        let r2 = run_hygiene_scan(&store);
        assert_eq!(r2.suggestions_created, 0);
    }
}
