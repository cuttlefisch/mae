//! KB activity tracking (last-accessed/modified/linked properties) and
//! the configuration audit report.

use super::*;

impl Editor {
    /// Record an access event for a KB node.
    ///
    /// @ai-caution: [kb-truth] Writes ONLY to the per-replica activity table.
    /// This used to call `kb_update_property_on_disk`, which wrote the node's
    /// `.org` file and then reimported it over the store — so reading a node
    /// reverted it to disk, and could delete store-only siblings outright
    /// (#729). Reading must never write node content. If you are adding a new
    /// activity signal, put it here, not in the node.
    pub fn kb_record_access(&mut self, node_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb
            .activity
            .entry(node_id.to_string())
            .or_default()
            .accessed = Some(today);
        self.kb.activity_dirty = true;
    }

    /// Record a modification event for every node parsed from `path`.
    /// Computes each node's OWN body hash, compares to its stored `:hash:`,
    /// and updates `:last-modified:` + `:hash:` only for nodes whose body
    /// actually changed.
    ///
    /// A file can hold several nodes sharing one `source_file` — file-level,
    /// per-heading, per-list-item (see #332) — so this checks every node
    /// independently instead of guessing "the" node from the path alone
    /// (the previous approach, via a now-removed `kb_find_node_by_path`
    /// first-match helper): editing node B's text no longer misattributes a
    /// stamp to node A, and a save that legitimately changes several nodes
    /// at once updates each one correctly instead of only the first found.
    pub fn kb_record_modification(&mut self, path: &std::path::Path) {
        if !self.kb.activity_tracking {
            return;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let today = today_str();
        for parsed in mae_kb::org::parse_org_multi(&content) {
            // Only nodes this KB actually already knows about — skip a node
            // parse_org_multi found that hasn't been ingested (yet).
            if self.kb_get_node_mut(&parsed.id).is_none() {
                continue;
            }
            let new_hash = mae_kb::activity::body_hash(&parsed.body);
            // The previously-seen hash: the local table first, falling back to
            // a `:hash:` already in the node from a pre-#729 ingest so an
            // existing corpus does not report every node as changed once.
            let old_hash = self
                .kb
                .activity
                .get(&parsed.id)
                .and_then(|a| a.hash.clone())
                .or_else(|| {
                    self.kb_get_node_mut(&parsed.id)
                        .and_then(|n| n.properties.get("hash").cloned())
                });
            if old_hash.as_deref() == Some(&new_hash) {
                continue; // this node's content unchanged
            }
            let entry = self.kb.activity.entry(parsed.id.clone()).or_default();
            entry.hash = Some(new_hash);
            entry.modified = Some(today.clone());
            self.kb.activity_dirty = true;
        }
    }

    /// Record a link event for a target node. Local-only, same rule as
    /// [`Self::kb_record_access`] — inserting a link must not rewrite and
    /// reimport the target's `.org` file (#729).
    pub fn kb_record_link(&mut self, target_id: &str) {
        if !self.kb.activity_tracking {
            return;
        }
        let today = today_str();
        self.kb
            .activity
            .entry(target_id.to_string())
            .or_default()
            .linked = Some(today);
        self.kb.activity_dirty = true;
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

impl Editor {
    /// Filename for the per-replica activity table, under the MAE data dir.
    ///
    /// Deliberately NOT inside any KB's `org_dir` and NOT in the CRDT: this is
    /// local, derived state (#729). A peer has no use for another peer's read
    /// timestamps, and syncing them would make every read author an operation.
    const ACTIVITY_FILE: &'static str = "kb-activity.json";

    /// Load the activity table. Missing or unreadable file ⇒ empty, never an
    /// error: activity ranking degrading to "no signal" is a cosmetic loss,
    /// and refusing to start over it would not be.
    pub fn kb_load_activity(&mut self) {
        let Some(dir) = self.mae_data_dir() else {
            return;
        };
        let Ok(raw) = std::fs::read_to_string(dir.join(Self::ACTIVITY_FILE)) else {
            return;
        };
        match serde_json::from_str(&raw) {
            Ok(map) => {
                self.kb.activity = map;
                self.kb.activity_dirty = false;
            }
            Err(e) => {
                tracing::warn!(error = %e, "kb activity table unreadable — starting empty");
            }
        }
    }

    /// Persist the activity table if it changed. No-op when clean, so a session
    /// that never opened a node writes nothing.
    ///
    /// Written via a temp file + rename so an interrupted save cannot leave a
    /// truncated table behind — the same reason the registry does it.
    pub fn kb_save_activity(&mut self) {
        if !self.kb.activity_dirty {
            return;
        }
        let Some(dir) = self.mae_data_dir() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&self.kb.activity) else {
            return;
        };
        let final_path = dir.join(Self::ACTIVITY_FILE);
        let tmp_path = dir.join(format!("{}.tmp", Self::ACTIVITY_FILE));
        if std::fs::write(&tmp_path, json).is_ok()
            && std::fs::rename(&tmp_path, &final_path).is_ok()
        {
            self.kb.activity_dirty = false;
        } else {
            let _ = std::fs::remove_file(&tmp_path);
            tracing::warn!("failed to persist kb activity table");
        }
    }
}
