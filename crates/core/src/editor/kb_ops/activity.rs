//! KB activity tracking (last-accessed/modified/linked properties) and
//! the configuration audit report.

use super::*;

impl Editor {
    /// Record an access event for a KB node. Updates `:last-accessed:` in the
    /// source .org file (if any) and in-memory properties.
    pub fn kb_record_access(&mut self, node_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb_update_property_on_disk(node_id, "last-accessed", &today);
    }

    /// Record a modification event. Computes body hash, compares to stored
    /// `:hash:`, and updates `:last-modified:` + `:hash:` if changed.
    pub fn kb_record_modification(&mut self, path: &std::path::Path) {
        if !self.kb.activity_tracking {
            return;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let new_hash = mae_kb::activity::body_hash(&content);
        // Find the node by source file path
        let node_id = self.kb_find_node_by_path(path).map(|n| n.id.clone());
        let Some(node_id) = node_id else {
            return;
        };
        // Check existing hash
        let old_hash = self
            .kb_find_node_by_path(path)
            .and_then(|n| n.properties.get("hash").cloned());
        if old_hash.as_deref() == Some(&new_hash) {
            return; // Content unchanged
        }
        let today = today_str();
        // Write hash and last-modified to file
        self.kb_update_property_in_file(path, "hash", &new_hash);
        self.kb_update_property_in_file(path, "last-modified", &today);
        // Update in-memory node properties
        if let Some(node) = self.kb_get_node_mut(&node_id) {
            node.properties.insert("hash".to_string(), new_hash);
            node.properties.insert("last-modified".to_string(), today);
        }
    }

    /// Record a link event for a target node. Updates `:last-linked:`.
    pub fn kb_record_link(&mut self, target_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb_update_property_on_disk(target_id, "last-linked", &today);
    }

    /// Update a single property in a node's source .org file on disk.
    /// Uses write-guard to prevent cascade.
    pub(super) fn kb_update_property_on_disk(&mut self, node_id: &str, key: &str, value: &str) {
        // Find the source file for this node
        let source_path = self.kb_node_source_path(node_id);
        let Some(path) = source_path else {
            return;
        };
        self.kb_update_property_in_file(&path, key, value);
        // Update in-memory node properties
        if let Some(node) = self.kb_get_node_mut(node_id) {
            node.properties.insert(key.to_string(), value.to_string());
        }
    }

    /// Write a property to a .org file and reimport. Uses write-guard.
    pub(super) fn kb_update_property_in_file(
        &mut self,
        path: &std::path::Path,
        key: &str,
        value: &str,
    ) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let Some(updated) = mae_kb::org::update_property(&content, key, value) else {
            return;
        };
        // Guard the path to prevent watcher cascade
        self.kb.write_guard.insert(path.to_path_buf());
        if std::fs::write(path, &updated).is_ok() {
            // Reimport synchronously to keep in-memory KB in sync
            self.kb_reimport_file(path);
            self.kb.watcher_stats.reimports_total += 1;
        }
    }

    /// Find a node by its source file path (across all KB instances).
    pub(super) fn kb_find_node_by_path(&self, path: &std::path::Path) -> Option<&mae_kb::Node> {
        for kb in self.kb.instances.values() {
            for id in kb.list_ids(None) {
                if let Some(node) = kb.get(&id) {
                    if node.source_file.as_deref() == Some(path) {
                        return Some(node);
                    }
                }
            }
        }
        None
    }

    /// Get the source file path for a node by ID.
    pub(super) fn kb_node_source_path(&self, node_id: &str) -> Option<std::path::PathBuf> {
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(node_id) {
                return node.source_file.clone();
            }
        }
        None
    }

    /// Get a mutable reference to a node by ID (across all KB instances).
    pub(super) fn kb_get_node_mut(&mut self, node_id: &str) -> Option<&mut mae_kb::Node> {
        for kb in self.kb.instances.values_mut() {
            if let Some(node) = kb.get_mut(node_id) {
                return Some(node);
            }
        }
        None
    }

    // ── Audit ────────────────────────────────────────────────────────

    /// Show a comprehensive KB audit report in a buffer.
    pub fn show_kb_audit_report(&mut self) {
        let mut lines = Vec::new();
        lines.push("* KB Audit Report".to_string());
        lines.push(String::new());

        // 1. Basic health
        let total_nodes: usize = self.kb.instances.values().map(|kb| kb.len()).sum();
        let total_links: usize = self
            .kb
            .instances
            .values()
            .flat_map(|kb| kb.list_ids(None))
            .filter_map(|id| {
                self.kb
                    .instances
                    .values()
                    .find_map(|kb| kb.get(&id))
                    .map(|n| n.links().len())
            })
            .sum();
        lines.push(format!("** Node count: {}", total_nodes));
        lines.push(format!("** Link count: {}", total_links));
        lines.push(String::new());

        // 2. Stale node detection
        let mut stale_count = 0;
        for kb in self.kb.instances.values() {
            for id in kb.list_ids(None) {
                if let Some(node) = kb.get(&id) {
                    if let Some(ref sf) = node.source_file {
                        if !sf.exists() {
                            stale_count += 1;
                            lines.push(format!("  - STALE: {} (file: {})", id, sf.display()));
                        }
                    }
                }
            }
        }
        if stale_count > 0 {
            lines.insert(
                lines.len() - stale_count,
                format!("** Stale nodes: {}", stale_count),
            );
        } else {
            lines.push("** Stale nodes: 0".to_string());
        }
        lines.push(String::new());

        // 3. Dailies chain validation
        if let Some(dir) = self.kb_dailies_dir() {
            if dir.exists() {
                let mut daily_files: Vec<String> = std::fs::read_dir(&dir)
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .filter_map(|e| {
                                e.path()
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .map(|s| s.to_string())
                            })
                            .filter(|s| mae_kb::activity::parse_date(s).is_some())
                            .collect()
                    })
                    .unwrap_or_default();
                daily_files.sort();
                let chain_gaps = daily_files
                    .windows(2)
                    .filter(|w| {
                        if let (Some(a), Some(b)) = (
                            mae_kb::activity::parse_date(&w[0]),
                            mae_kb::activity::parse_date(&w[1]),
                        ) {
                            mae_kb::activity::days_between(a, b) > 1
                        } else {
                            false
                        }
                    })
                    .count();
                lines.push(format!(
                    "** Dailies: {} files, {} chain gaps",
                    daily_files.len(),
                    chain_gaps
                ));
            } else {
                lines.push("** Dailies: directory not found".to_string());
            }
        } else {
            lines.push("** Dailies: not configured".to_string());
        }
        lines.push(String::new());

        // 4. Watcher stats
        let stats = &self.kb.watcher_stats;
        lines.push("** Watcher stats".to_string());
        lines.push(format!("   Upserted: {}", stats.events_upserted));
        lines.push(format!("   Removed: {}", stats.events_removed));
        lines.push(format!("   Suppressed: {}", stats.events_suppressed));
        lines.push(format!("   Reimports total: {}", stats.reimports_total));
        lines.push(format!("   Errors: {}", stats.errors));

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*KB Audit*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    // ── Dailies ─────────────────────────────────────────────────────
}
