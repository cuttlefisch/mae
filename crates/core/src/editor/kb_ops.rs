//! KB federation operations: register, unregister, reimport.

use std::path::{Path, PathBuf};

use mae_kb::federation::{ImportHealth, ImportReport};

use super::Editor;

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

impl Editor {
    /// Resolve the MAE config directory (~/.config/mae).
    /// Checks `config_dir_override` first (for test isolation).
    #[allow(dead_code)]
    fn mae_config_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.config_dir_override {
            return Some(dir.clone());
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            Some(PathBuf::from(xdg).join("mae"))
        } else if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".config").join("mae"))
        } else {
            None
        }
    }

    /// Resolve the MAE data directory (~/.local/share/mae).
    /// Checks `data_dir_override` first (for test isolation).
    pub fn mae_data_dir(&self) -> Option<PathBuf> {
        if let Some(ref dir) = self.data_dir_override {
            return Some(dir.clone());
        }
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            Some(PathBuf::from(xdg).join("mae"))
        } else if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".local").join("share").join("mae"))
        } else {
            None
        }
    }

    /// Register an external org directory as a federated KB instance.
    ///
    /// Recursively imports all `.org` files, computes health metrics,
    /// and reports results via the status bar.
    pub fn kb_register(&mut self, name: &str, org_dir: &Path) -> Option<KbImportResult> {
        if !org_dir.exists() {
            self.set_status(format!(
                "KB register error: path does not exist: {}",
                org_dir.display()
            ));
            return None;
        }
        if !org_dir.is_dir() {
            self.set_status(format!(
                "KB register error: not a directory: {}",
                org_dir.display()
            ));
            return None;
        }

        let Some(data_dir) = self.mae_data_dir() else {
            self.set_status("KB register error: cannot determine data directory");
            return None;
        };
        let _ = std::fs::create_dir_all(&data_dir);

        let uuid = self.kb.registry.register(
            name.to_string(),
            org_dir.to_path_buf(),
            &data_dir,
            self.kb.data_dir.as_ref(),
        );

        // Import org files — try CozoDB-direct ingestion first.
        let inst_ref = self.kb.registry.find(&uuid).cloned();
        let (kb, report, health) = if let Some(inst) = inst_ref {
            match mae_kb::CozoKbStore::open(&inst.db_path) {
                Ok(store) => {
                    match mae_kb::federation::import_org_dir_to_store(
                        org_dir,
                        &store,
                        &mae_kb::IngestMode::Full,
                    ) {
                        Ok((kb, report)) => {
                            let health = mae_kb::ImportHealth::from_kb(&kb);
                            // Retain the CozoDB store handle for runtime queries.
                            self.kb
                                .instance_stores
                                .insert(uuid.clone(), std::sync::Arc::new(store));
                            (kb, report, health)
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "CozoDB ingestion failed, falling back to in-memory import"
                            );
                            mae_kb::federation::import_org_dir(org_dir)
                        }
                    }
                }
                Err(_) => mae_kb::federation::import_org_dir(org_dir),
            }
        } else {
            mae_kb::federation::import_org_dir(org_dir)
        };

        // Store the instance
        self.kb.instances.insert(uuid.clone(), kb);

        // Start file watcher for live updates (if enabled)
        if self.kb.watcher_enabled {
            match mae_kb::watch::OrgDirWatcher::new(org_dir) {
                Ok(watcher) => {
                    watcher.seed(
                        report
                            .path_to_ids
                            .iter()
                            .map(|(p, ids)| (p.clone(), ids.clone())),
                    );
                    self.kb.watchers.insert(uuid.clone(), watcher);
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("inotify") || msg.contains("No space left") {
                        self.set_status(
                            "KB watcher failed: inotify limit reached. \
                             Run `sysctl fs.inotify.max_user_watches=65536` \
                             or set `kb_watcher_enabled=false`.",
                        );
                    }
                    // Watcher is optional — registration still succeeds
                }
            }
        }

        // Update last_import timestamp
        if let Some(inst) = self
            .kb
            .registry
            .instances
            .iter_mut()
            .find(|i| i.uuid == uuid)
        {
            inst.last_import = Some(chrono_now());
        }

        // Persist registry
        let _ = self.kb.registry.save(&data_dir);

        let result = KbImportResult {
            name: name.to_string(),
            uuid,
            report,
            health,
        };

        // Rebuild the query layer to include the new instance.
        self.kb.rebuild_query_layer();

        self.set_status(result.status_summary());
        Some(result)
    }

    /// Unregister a KB instance by name or UUID.
    pub fn kb_unregister(&mut self, name_or_uuid: &str) {
        let found = self.kb.registry.find(name_or_uuid).map(|i| i.uuid.clone());
        match found {
            Some(uuid) => {
                self.kb.instances.remove(&uuid);
                self.kb.instance_stores.remove(&uuid);
                self.kb.watchers.remove(&uuid);
                self.kb.registry.unregister(name_or_uuid);
                if let Some(data_dir) = self.mae_data_dir() {
                    let _ = self.kb.registry.save(&data_dir);
                }
                // Rebuild query layer without the removed instance.
                self.kb.rebuild_query_layer();
                self.set_status(format!("KB instance '{}' unregistered", name_or_uuid));
            }
            None => {
                self.set_status(format!(
                    "KB unregister: no instance found matching '{}'",
                    name_or_uuid
                ));
            }
        }
    }

    /// Re-import an existing KB instance (refresh after org file edits).
    ///
    /// When `mode` is `None`, defaults to `IngestMode::Full`.
    pub fn kb_reimport(
        &mut self,
        name_or_uuid: &str,
        mode: Option<mae_kb::IngestMode>,
    ) -> Option<KbImportResult> {
        let inst = self.kb.registry.find(name_or_uuid).cloned();
        match inst {
            Some(instance) => {
                let mode = mode.unwrap_or_default();

                // Try CozoDB-direct ingestion for the instance's DB.
                let (kb, report, health) = match mae_kb::CozoKbStore::open(&instance.db_path) {
                    Ok(store) => {
                        match mae_kb::federation::import_org_dir_to_store(
                            &instance.org_dir,
                            &store,
                            &mode,
                        ) {
                            Ok((kb, report)) => {
                                let health = mae_kb::ImportHealth::from_kb(&kb);
                                (kb, report, health)
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "CozoDB ingestion failed, falling back to in-memory import"
                                );
                                mae_kb::federation::import_org_dir(&instance.org_dir)
                            }
                        }
                    }
                    Err(_) => {
                        // No CozoDB store for this instance — use in-memory import.
                        mae_kb::federation::import_org_dir(&instance.org_dir)
                    }
                };

                self.kb.instances.insert(instance.uuid.clone(), kb);

                // Update timestamp
                if let Some(reg_inst) = self
                    .kb
                    .registry
                    .instances
                    .iter_mut()
                    .find(|i| i.uuid == instance.uuid)
                {
                    reg_inst.last_import = Some(chrono_now());
                }
                if let Some(data_dir) = self.mae_data_dir() {
                    let _ = self.kb.registry.save(&data_dir);
                }

                let result = KbImportResult {
                    name: instance.name.clone(),
                    uuid: instance.uuid.clone(),
                    report,
                    health,
                };

                let msg = format!(
                    "Reimported '{}': {}",
                    instance.name,
                    result.status_summary()
                );
                self.set_status(&msg);
                Some(result)
            }
            None => {
                self.set_status(format!(
                    "KB reimport: no instance found matching '{}'",
                    name_or_uuid
                ));
                None
            }
        }
    }

    /// Persist a node to the backing store (if present). Best-effort — logs errors.
    fn kb_persist_node(&self, node: &mae_kb::Node) {
        if let Some(ref store) = self.kb.store {
            if let Err(e) = store.update_node(node) {
                tracing::warn!(node_id = %node.id, error = %e, "KB store write-through failed");
            }
        }
    }

    /// Persist a deletion to the backing store (if present). Best-effort.
    fn kb_persist_delete(&self, id: &str) {
        if let Some(ref store) = self.kb.store {
            if let Err(e) = store.delete_node(id) {
                tracing::warn!(node_id = %id, error = %e, "KB store delete failed");
            }
        }
    }

    /// Create a new KB node in the local knowledge base.
    /// Rejects overwriting seed nodes (built-in help).
    pub fn kb_create_node(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        kind: mae_kb::NodeKind,
    ) -> Result<(), String> {
        // Guard: refuse to overwrite seed nodes
        if let Some(existing) = self.kb.primary.get(id) {
            if existing.source == Some(mae_kb::NodeSource::Seed) {
                return Err(format!(
                    "Cannot overwrite seed node '{}' — built-in help is protected",
                    id
                ));
            }
        }
        let node =
            mae_kb::Node::new(id, title, kind, body).with_source(mae_kb::NodeSource::Manual, 0);
        self.kb_persist_node(&node);
        self.kb.primary.insert(node);
        self.set_status(format!("KB node created: {}", id));
        Ok(())
    }

    /// Delete a KB node from the local knowledge base.
    /// Rejects deleting seed nodes (built-in help).
    pub fn kb_delete_node(&mut self, id: &str) -> Result<(), String> {
        match self.kb.primary.get(id) {
            None => Err(format!("No KB node: {}", id)),
            Some(node) if node.source == Some(mae_kb::NodeSource::Seed) => Err(format!(
                "Cannot delete seed node '{}' — built-in help is protected",
                id
            )),
            Some(_) => {
                self.kb_persist_delete(id);
                self.kb.primary.remove(id);
                self.set_status(format!("KB node deleted: {}", id));
                Ok(())
            }
        }
    }

    /// Update fields on an existing KB node.
    /// Rejects modifying seed nodes (built-in help).
    pub fn kb_update_node(
        &mut self,
        id: &str,
        title: Option<&str>,
        body: Option<&str>,
        tags: Option<Vec<String>>,
    ) -> Result<(), String> {
        let existing = self
            .kb
            .primary
            .get(id)
            .ok_or_else(|| format!("No KB node: {}", id))?
            .clone();
        if existing.source == Some(mae_kb::NodeSource::Seed) {
            return Err(format!(
                "Cannot modify seed node '{}' — built-in help is protected",
                id
            ));
        }
        let mut updated = existing;
        if let Some(t) = title {
            updated.title = t.to_string();
        }
        if let Some(b) = body {
            updated.body = b.to_string();
        }
        if let Some(t) = tags {
            updated.tags = t;
        }

        // Check if this node belongs to a shared KB and sync mode is "on_save".
        let shared_kb_id = if self.collab.kb_sync_mode == "on_save" {
            self.collab
                .shared_kbs
                .iter()
                .find(|(_, nodes)| nodes.contains(id))
                .map(|(kb_id, _)| kb_id.clone())
        } else {
            None
        };

        if let Some(kb_id) = shared_kb_id {
            // Use CRDT-aware upsert to generate update bytes for broadcasting.
            // client_id 1 is used for local edits (distinct from remote).
            if let Some(update_bytes) = self.kb.primary.upsert_with_crdt(updated, 1) {
                // Persist CRDT update to pending queue (durable offline queue).
                if let Some(ref store) = self.kb.store {
                    let _ = store.push_pending_update(&kb_id, id, &update_bytes);
                }
                self.collab
                    .pending_kb_updates
                    .push((kb_id, id.to_string(), update_bytes));
            }
            // Persist the updated node to store.
            if let Some(node) = self.kb.primary.get(id) {
                self.kb_persist_node(node);
            }
        } else {
            self.kb_persist_node(&updated);
            self.kb.primary.insert(updated);
        }

        self.set_status(format!("KB node updated: {}", id));
        Ok(())
    }

    /// Show KB instances in a dedicated read-only buffer.
    pub fn show_kb_instances(&mut self) {
        let mut lines = vec![
            "KB Instances".to_string(),
            "============".to_string(),
            String::new(),
        ];
        let count = self.kb.registry.instances.len();
        if self.kb.registry.instances.is_empty() {
            lines.push("  (none registered)".to_string());
        } else {
            for inst in &self.kb.registry.instances {
                let node_count = self
                    .kb
                    .instances
                    .get(&inst.uuid)
                    .map(|kb| kb.len())
                    .unwrap_or(0);
                let status = if inst.enabled { "enabled" } else { "disabled" };
                lines.push(format!(
                    "  {} [{}] — {} nodes, {} ({})",
                    inst.name,
                    inst.uuid,
                    node_count,
                    status,
                    inst.org_dir.display(),
                ));
            }
        }
        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*KB Instances*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;
        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
        self.set_status(format!("{} KB instance(s) registered", count));
    }

    /// Create a KB note from just a title (org-roam-style).
    ///
    /// Auto-generates a `user:TIMESTAMP-slug` ID. If `kb_notes_dir` is set,
    /// persists the note as an `.org` file and imports it into the matching
    /// KB instance. Otherwise creates an ephemeral in-memory node.
    ///
    /// Returns `(id, Option<path>)` — the node id and the file path if written.
    pub fn kb_create_note_from_title(
        &mut self,
        title: &str,
    ) -> Result<(String, Option<std::path::PathBuf>), String> {
        let slug = mae_kb::slugify(title);
        if slug.is_empty() {
            return Err("Title cannot be empty".to_string());
        }
        let timestamp = mae_kb::timestamp_id();
        let id = format!("user:{}-{}", timestamp, slug);

        if let Some(dir) = self.kb.notes_dir.clone() {
            // Ensure directory exists
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Cannot create kb-notes-dir: {}", e))?;

            // Write .org file
            let filename = format!("{}.org", slug);
            let path = dir.join(&filename);
            let content = format!(
                ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {}\n#+filetags:\n\n",
                id, title
            );
            std::fs::write(&path, &content)
                .map_err(|e| format!("Cannot write note file: {}", e))?;

            // Insert into matching KB instance (if registered)
            self.kb_insert_to_notes_instance(&id, title, &path);

            // Record return buffer before opening new file
            let return_idx = self.active_buffer_idx();

            // Open the file for editing
            self.open_file(&path);

            // Seed KB buffer (hidden) so SPC n v can toggle to rendered view later.
            // Do NOT call open_help_at() — that would display it and create a split.
            let help_idx = self.ensure_kb_buffer_idx(&id);
            self.kb_populate_buffer(help_idx);

            // Enter capture mode (C-c C-c to finalize, C-c C-k to abort)
            self.kb.capture_state = Some(super::CaptureState {
                node_id: id.clone(),
                file_path: Some(path.clone()),
                return_buffer_idx: return_idx,
            });

            self.set_status(format!(
                "Capture: {} — SPC n s to finish | SPC n k to abort",
                title
            ));
            Ok((id, Some(path)))
        } else {
            // Ephemeral in-memory node (fallback)
            self.kb_create_node(&id, title, "", mae_kb::NodeKind::Note)?;
            Ok((id, None))
        }
    }

    /// Insert a node into the KB instance that covers `kb_notes_dir`.
    /// Falls back to inserting into the local KB if no matching instance.
    fn kb_insert_to_notes_instance(&mut self, id: &str, title: &str, path: &std::path::Path) {
        let node = mae_kb::Node::new(id, title, mae_kb::NodeKind::Note, "")
            .with_source(mae_kb::NodeSource::UserOrg, 0)
            .with_source_file(path);

        // Try to find a registered instance whose org_dir matches kb_notes_dir
        let notes_dir = self.kb.notes_dir.clone();
        if let Some(ref dir) = notes_dir {
            for inst in &self.kb.registry.instances {
                if inst.org_dir == *dir {
                    if let Some(kb) = self.kb.instances.get_mut(&inst.uuid) {
                        kb.insert(node);
                        return;
                    }
                }
            }
        }

        // Fallback: insert into local KB
        self.kb.primary.insert(node);
    }

    /// Collect all KB node (id, title) pairs from local + federated instances.
    pub fn kb_all_node_pairs(&self) -> Vec<(String, String)> {
        if let Some(q) = self.kb.query_layer() {
            let mut pairs = q.id_title_pairs(None);
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            return pairs;
        }
        let mut pairs: Vec<(String, String)> = self.kb.primary.all_id_title_pairs();
        let mut seen: std::collections::HashSet<String> =
            pairs.iter().map(|(id, _)| id.clone()).collect();

        for kb in self.kb.instances.values() {
            for (id, title) in kb.all_id_title_pairs() {
                if seen.insert(id.clone()) {
                    pairs.push((id, title));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    /// Collect all KB node (id, title, body) triples from local + federated instances.
    /// Used by KB palettes that need body content for search matching.
    /// Sorted according to `kb_search_sort` option: alphabetical (default/relevance),
    /// activity (recent first), or alphabetical.
    pub fn kb_all_node_triples(&self) -> Vec<(String, String, String)> {
        // Body truncated to 500 chars — only used for fuzzy search, not display.
        let mut triples: Vec<(String, String, String)> = if let Some(q) = self.kb.query_layer() {
            q.id_title_body_triples(None, 500)
        } else {
            self.kb.primary.all_id_title_body_triples()
        };
        let mut seen: std::collections::HashSet<String> =
            triples.iter().map(|(id, _, _)| id.clone()).collect();

        if self.kb.query_layer().is_none() {
            for kb in self.kb.instances.values() {
                for (id, title, body) in kb.all_id_title_body_triples() {
                    if seen.insert(id.clone()) {
                        triples.push((id, title, body));
                    }
                }
            }
        }

        if self.kb.search_sort == "activity" {
            let weights = mae_kb::activity::ActivityWeights {
                decay: self.kb.activity_decay,
                ..Default::default()
            };
            let today = today_ymd();
            triples.sort_by(|a, b| {
                let score_a = self.kb_activity_score_for_id(&a.0, &weights, today);
                let score_b = self.kb_activity_score_for_id(&b.0, &weights, today);
                score_b
                    .partial_cmp(&score_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
        } else {
            triples.sort_by(|a, b| a.0.cmp(&b.0));
        }
        triples
    }

    /// Get activity score for a node by ID, searching local then federated KBs.
    pub fn kb_activity_score_for_id(
        &self,
        id: &str,
        weights: &mae_kb::activity::ActivityWeights,
        today: (i32, u32, u32),
    ) -> f64 {
        if let Some(q) = self.kb.query_layer() {
            if let Some(node) = q.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
            return 0.0;
        }
        if let Some(node) = self.kb.primary.get(id) {
            return mae_kb::activity::activity_score(&node.properties, weights, today);
        }
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
        }
        0.0
    }

    /// Re-import a single file into the KB instance that covers its directory.
    /// Used after saving a file inside `kb_notes_dir` to keep the graph in sync.
    pub fn kb_reimport_file(&mut self, path: &std::path::Path) {
        for (uuid, inst) in self
            .kb
            .registry
            .instances
            .iter()
            .map(|i| (i.uuid.clone(), i.org_dir.clone()))
        {
            if path.starts_with(&inst) {
                if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                    kb.ingest_org_file(path);
                    return;
                }
            }
        }
    }

    /// Check if a path is inside a registered KB instance directory.
    pub fn kb_path_in_instance(&self, path: &std::path::Path) -> bool {
        self.kb
            .registry
            .instances
            .iter()
            .any(|i| path.starts_with(&i.org_dir))
    }

    /// Search across local KB and all federated instances.
    /// Returns (instance_name_or_none, node) pairs, deduplicated by node ID.
    /// Local results take priority over federated.
    /// Respects `kb_search_sort` option: "relevance" (default), "activity", "alphabetical".
    pub fn kb_federated_search(&self, query: &str) -> Vec<(Option<String>, &mae_kb::Node)> {
        self.kb_federated_search_scoped(query, &mae_kb::KbScope::All)
    }

    /// Search across the primary KB and federated instances, restricted to the
    /// given `scope` (plan decision D4). `KbScope::All` reproduces
    /// `kb_federated_search` exactly. Local results always win on duplicates.
    /// Respects `kb_search_sort` ("relevance" default / "activity" /
    /// "alphabetical" / "recency"). "recency" ranks by relevance first, then
    /// stably re-sorts so session-visited nodes float to the top (most-recent
    /// first; unvisited nodes keep their relevance order below them).
    pub fn kb_federated_search_scoped(
        &self,
        query: &str,
        scope: &mae_kb::KbScope,
    ) -> Vec<(Option<String>, &mae_kb::Node)> {
        use mae_kb::KbScope;
        let use_activity = self.kb.search_sort == "activity";
        let use_alpha = self.kb.search_sort == "alphabetical";
        let use_recency = self.kb.search_sort == "recency";
        let weights = mae_kb::activity::ActivityWeights {
            decay: self.kb.activity_decay,
            ..Default::default()
        };
        let today = if use_activity { today_ymd() } else { (0, 0, 0) };

        // Per-instance ranking, shared by primary + federated members.
        let rank = |kb: &mae_kb::KnowledgeBase| -> Vec<String> {
            if use_activity {
                kb.search_sorted_by_activity(query, &weights, today)
            } else if use_alpha {
                kb.search(query)
            } else {
                kb.search_ranked(query, self.kb.search_max_results)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
        };

        let mut results: Vec<(Option<String>, &mae_kb::Node)> = Vec::new();
        let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();

        // Does the primary participate under this scope? The primary's registry
        // name is matched for the Named case.
        let primary_name = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.primary)
            .map(|i| i.name.as_str());
        let include_primary = match scope {
            KbScope::All | KbScope::LocalOnly => true,
            KbScope::RemoteOnly => false,
            KbScope::Named(n) => primary_name == Some(n.as_str()),
        };

        if include_primary {
            for id in rank(&self.kb.primary) {
                if let Some(node) = self.kb.primary.get(&id) {
                    if seen_ids.insert(&node.id) {
                        results.push((None, node));
                    }
                }
            }
        }

        // Then each federated instance permitted by the scope (skip if seen).
        for (uuid, kb) in &self.kb.instances {
            let inst = self.kb.registry.find_by_uuid(uuid);
            let include = match scope {
                KbScope::All => true,
                KbScope::LocalOnly => false,
                KbScope::RemoteOnly => inst.is_some_and(|i| i.is_remote()),
                KbScope::Named(n) => inst.is_some_and(|i| &i.name == n),
            };
            if !include {
                continue;
            }
            let inst_name = inst.map(|i| i.name.clone());
            for id in rank(kb) {
                if let Some(node) = kb.get(&id) {
                    if seen_ids.insert(&node.id) {
                        results.push((inst_name.clone(), node));
                    }
                }
            }
        }

        if use_alpha {
            results.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        } else if use_recency {
            // Stable sort by descending visit ordinal: most-recently-visited
            // first; ties (incl. all unvisited at rank 0) keep relevance order.
            results.sort_by(|a, b| {
                self.kb
                    .visit_rank(&b.1.id)
                    .cmp(&self.kb.visit_rank(&a.1.id))
            });
        }

        results
    }

    /// Get a node by ID, searching local first then federated instances.
    pub fn kb_federated_get(&self, id: &str) -> Option<(Option<String>, &mae_kb::Node)> {
        if let Some(node) = self.kb.primary.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.kb.instances {
            if let Some(node) = kb.get(id) {
                let name = self.kb.registry.find_by_uuid(uuid).map(|i| i.name.clone());
                return Some((name, node));
            }
        }
        None
    }

    /// Drain KB file watchers — apply changes from filesystem events.
    /// Called from `idle_work()` to pick up org file edits without `:kb-reimport`.
    ///
    /// Hardened with: debounce (skip if too recent), drain cap (max N events),
    /// time-boxing (50ms deadline), error tracking, and enable/disable toggle.
    pub fn drain_kb_watchers(&mut self) {
        // Early return if watchers disabled
        if !self.kb.watcher_enabled {
            return;
        }

        let drain_start = std::time::Instant::now();
        let debounce_dur = std::time::Duration::from_millis(self.kb.watcher_debounce_ms);
        let max_events = self.kb.max_drain_events;
        let deadline = drain_start + std::time::Duration::from_millis(50);

        let uuids: Vec<String> = self.kb.watchers.keys().cloned().collect();
        let mut changed = false;
        let mut total_processed: usize = 0;

        for uuid in uuids {
            // Debounce: skip if last drain was too recent
            if let Some(last) = self.kb.last_drain.get(&uuid) {
                if last.elapsed() < debounce_dur {
                    self.kb.watcher_stats.suppressed_debounce += 1;
                    continue;
                }
            }

            let changes = match self.kb.watchers.get(&uuid) {
                Some(w) => {
                    // Track watcher errors
                    let errs = w.error_count();
                    if errs > self.kb.watcher_stats.errors {
                        self.kb.watcher_stats.errors = errs;
                    }
                    w.drain()
                }
                None => continue,
            };
            if changes.is_empty() {
                continue;
            }

            // Update last drain timestamp
            self.kb
                .last_drain
                .insert(uuid.clone(), std::time::Instant::now());

            let skipped = changes.len().saturating_sub(max_events);
            if skipped > 0 {
                self.kb.watcher_stats.suppressed_timebox += skipped as u64;
            }

            for change in changes.into_iter().take(max_events) {
                // Time-boxing: break if we've exceeded the 50ms budget
                if std::time::Instant::now() > deadline {
                    self.kb.watcher_stats.suppressed_timebox += 1;
                    break;
                }

                match change {
                    mae_kb::watch::OrgChange::Upserted(path) => {
                        // Suppress events for paths MAE is currently writing
                        // (activity tracking, chain-fill) to prevent cascade.
                        if self.kb.write_guard.remove(&path) {
                            self.kb.watcher_stats.events_suppressed += 1;
                            total_processed += 1;
                            continue;
                        }
                        if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                            let ids = kb.ingest_org_file(&path);
                            if let Some(w) = self.kb.watchers.get(&uuid) {
                                w.record_ids(path, ids);
                            }
                            self.kb.watcher_stats.events_upserted += 1;
                            changed = true;
                            total_processed += 1;
                        }
                    }
                    mae_kb::watch::OrgChange::Removed(ids) => {
                        if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                            for id in ids {
                                kb.remove(&id);
                            }
                            self.kb.watcher_stats.events_removed += 1;
                            changed = true;
                            total_processed += 1;
                        }
                    }
                }
            }
        }

        // Record timing in both watcher stats and perf stats
        let elapsed_us = drain_start.elapsed().as_micros() as u64;
        self.kb.watcher_stats.last_drain_us = elapsed_us;
        self.kb.watcher_stats.last_drain_event_count = total_processed;
        if total_processed > 0 {
            self.kb.watcher_stats.drain_us_sum += elapsed_us;
            self.kb.watcher_stats.drain_count += 1;
            self.kb.watcher_stats.reimports_total +=
                self.kb.watcher_stats.events_upserted + self.kb.watcher_stats.events_removed;
        }
        self.perf_stats.kb_watcher_drain_us = elapsed_us;
        self.perf_stats.kb_watcher_events += total_processed as u64;

        if changed {
            self.fire_hook("after-kb-change");
        }
    }

    /// Validate links in the current buffer's KB node after save.
    /// Shows a status bar warning if broken links are found.
    /// Advisory only — does NOT block the save.
    pub fn validate_kb_links_on_save(&mut self) {
        let idx = self.active_buffer_idx();
        let buf = &self.buffers[idx];

        // Only validate KB-sourced buffers (have a source_file or daily: prefix)
        let node_id: Option<String> = buf.file_path().and_then(|path| {
            // Find a node whose source_file matches this path
            if let Some(q) = self.kb.query_layer() {
                q.list_ids(None).into_iter().find(|id| {
                    q.get(id)
                        .and_then(|n| n.source_file)
                        .map(|sf| sf.as_path() == path)
                        .unwrap_or(false)
                })
            } else {
                self.kb
                    .primary
                    .all_id_title_pairs()
                    .into_iter()
                    .find_map(|(id, _)| {
                        self.kb.primary.get(&id).and_then(|n| {
                            n.source_file
                                .as_ref()
                                .filter(|sf| sf.as_path() == path)
                                .map(|_| id.clone())
                        })
                    })
            }
        });

        // Also check dailies buffers
        let node_id = node_id.or_else(|| {
            let name = &self.buffers[self.active_buffer_idx()].name;
            if name.starts_with("daily:") {
                Some(name.clone())
            } else {
                None
            }
        });

        if let Some(id) = node_id {
            let missing: Vec<String> = if let Some(q) = self.kb.query_layer() {
                q.links_from(&id)
                    .into_iter()
                    .filter(|l| !q.contains(&l.dst))
                    .map(|l| l.dst)
                    .collect()
            } else {
                let m = self.kb.primary.validate_links(&id);
                // Also check federated instances for the targets
                m.into_iter()
                    .filter(|target| !self.kb.instances.values().any(|kb| kb.contains(target)))
                    .collect()
            };
            if !missing.is_empty() {
                self.set_status(format!(
                    "Warning: {} broken link(s) in this node",
                    missing.len()
                ));
            }
        }
    }

    /// Clean up orphan user notes (no links in or out).
    /// Preserves seed nodes (cmd:, concept:, lesson:, scheme:, option:).
    /// Returns the number of orphans removed.
    pub fn kb_cleanup_orphans(&mut self) -> usize {
        let seed_prefixes = ["cmd:", "concept:", "lesson:", "scheme:", "option:"];
        let orphan_ids: Vec<String> = if let Some(q) = self.kb.query_layer() {
            q.health_report().map(|r| r.orphan_ids).unwrap_or_default()
        } else {
            self.kb.primary.health_report().orphan_ids
        };
        let to_remove: Vec<String> = orphan_ids
            .into_iter()
            .filter(|id| !seed_prefixes.iter().any(|p| id.starts_with(p)))
            .collect();
        let count = to_remove.len();
        for id in &to_remove {
            self.kb.primary.remove(id);
        }
        if count > 0 {
            self.fire_hook("after-kb-change");
        }
        count
    }
}

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
    fn kb_update_property_on_disk(&mut self, node_id: &str, key: &str, value: &str) {
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
    fn kb_update_property_in_file(&mut self, path: &std::path::Path, key: &str, value: &str) {
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
    fn kb_find_node_by_path(&self, path: &std::path::Path) -> Option<&mae_kb::Node> {
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
    fn kb_node_source_path(&self, node_id: &str) -> Option<std::path::PathBuf> {
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(node_id) {
                return node.source_file.clone();
            }
        }
        None
    }

    /// Get a mutable reference to a node by ID (across all KB instances).
    fn kb_get_node_mut(&mut self, node_id: &str) -> Option<&mut mae_kb::Node> {
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

    /// Resolve the dailies directory. Explicit setting takes priority;
    /// falls back to `kb_notes_dir/daily`.
    pub fn kb_dailies_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref dir) = self.kb.dailies_dir {
            return Some(dir.clone());
        }
        self.kb.notes_dir.as_ref().map(|d| d.join("daily"))
    }

    /// Path for a daily note file: `dailies_dir/YYYY-MM-DD.org`.
    fn kb_daily_path(&self, y: i32, m: u32, d: u32) -> Option<std::path::PathBuf> {
        self.kb_dailies_dir()
            .map(|dir| dir.join(format!("{}.org", mae_kb::activity::format_date(y, m, d))))
    }

    /// Canonical ID for a daily note.
    fn kb_daily_id(y: i32, m: u32, d: u32) -> String {
        format!("daily:{}", mae_kb::activity::format_date(y, m, d))
    }

    /// Check if a daily file exists on disk.
    fn kb_daily_exists(&self, y: i32, m: u32, d: u32) -> bool {
        self.kb_daily_path(y, m, d)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Create a daily .org file stub with PROPERTIES drawer + title.
    /// Does NOT insert Previous: link (chain_fill does that).
    fn kb_create_daily_stub(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
    ) -> Result<std::path::PathBuf, String> {
        let dir = self
            .kb_dailies_dir()
            .ok_or("No dailies directory configured")?;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create dailies dir: {}", e))?;
        }
        let path = dir.join(format!("{}.org", mae_kb::activity::format_date(y, m, d)));
        if path.exists() {
            return Ok(path);
        }
        let id = Self::kb_daily_id(y, m, d);
        let date_str = mae_kb::activity::format_date(y, m, d);
        let content = format!(
            ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {}\n\n",
            id, date_str
        );
        std::fs::write(&path, &content).map_err(|e| format!("Failed to write daily: {}", e))?;
        // Guard and reimport
        self.kb.write_guard.insert(path.clone());
        self.kb_reimport_file(&path);
        self.kb.watcher_stats.reimports_total += 1;
        Ok(path)
    }

    /// Find the nearest existing daily before/after a date.
    /// `direction`: -1 = backward, 1 = forward.
    fn kb_daily_find_nearest(
        &self,
        y: i32,
        m: u32,
        d: u32,
        direction: i32,
    ) -> Option<(i32, u32, u32)> {
        let max_search = self.kb.daily_chain_gap_max;
        let step = if direction < 0 {
            mae_kb::activity::prev_day
        } else {
            mae_kb::activity::next_day
        };
        let mut cur = step(y, m, d);
        for _ in 0..max_search {
            if self.kb_daily_exists(cur.0, cur.1, cur.2) {
                return Some(cur);
            }
            cur = step(cur.0, cur.1, cur.2);
        }
        None
    }

    /// Chain-fill: ensure target date's daily exists and is linked back to
    /// the most recent pre-existing daily. Creates stub files for gaps.
    pub fn kb_daily_chain_fill(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
    ) -> Result<ChainFillResult, String> {
        let mut result = ChainFillResult {
            stubs_created: Vec::new(),
            links_inserted: 0,
            anchor_date: None,
        };

        // Ensure target date exists
        let target_path = self.kb_create_daily_stub(y, m, d)?;
        let _ = target_path; // used implicitly via reimport

        // Walk backwards to find the anchor (pre-existing daily)
        let max_gap = self.kb.daily_chain_gap_max;
        let mut cur = (y, m, d);
        let mut chain: Vec<(i32, u32, u32)> = vec![cur];

        for _ in 0..max_gap {
            let prev = mae_kb::activity::prev_day(cur.0, cur.1, cur.2);
            if self.kb_daily_exists(prev.0, prev.1, prev.2) {
                // This is a pre-existing daily — it's our anchor
                result.anchor_date = Some(prev);
                chain.push(prev);
                break;
            }
            // Create stub for the gap day
            self.kb_create_daily_stub(prev.0, prev.1, prev.2)?;
            result.stubs_created.push(prev);
            chain.push(prev);
            cur = prev;
        }

        // Now insert "Previous:" links from newest → oldest
        // chain is [target, ..., anchor] so we link chain[i] → chain[i+1]
        for i in 0..chain.len().saturating_sub(1) {
            let (cy, cm, cd) = chain[i];
            let (py, pm, pd) = chain[i + 1];
            let prev_id = Self::kb_daily_id(py, pm, pd);
            let prev_date_str = mae_kb::activity::format_date(py, pm, pd);
            let link_line = format!("Previous: [[id:{}][{}]]", prev_id, prev_date_str);

            // Insert "Previous:" link on chain[i] pointing to chain[i+1]
            if let Some(path) = self.kb_daily_path(cy, cm, cd) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.contains("Previous:") {
                        let mut lines: Vec<&str> = content.lines().collect();
                        let insert_pos = lines
                            .iter()
                            .position(|l| l.starts_with("#+title:"))
                            .map(|i| i + 1)
                            .unwrap_or(lines.len());
                        lines.insert(insert_pos, &link_line);
                        let updated = lines.join("\n") + "\n";
                        self.kb.write_guard.insert(path.clone());
                        if std::fs::write(&path, &updated).is_ok() {
                            self.kb_reimport_file(&path);
                            self.kb.watcher_stats.reimports_total += 1;
                            result.links_inserted += 1;
                        }
                    }
                }
            }

            // Insert symmetric "Next:" link on chain[i+1] pointing to chain[i]
            let next_id = Self::kb_daily_id(cy, cm, cd);
            let next_date_str = mae_kb::activity::format_date(cy, cm, cd);
            let next_link_line = format!("Next: [[id:{}][{}]]", next_id, next_date_str);

            if let Some(prev_path) = self.kb_daily_path(py, pm, pd) {
                if let Ok(content) = std::fs::read_to_string(&prev_path) {
                    if !content.contains("Next:") {
                        let mut lines: Vec<&str> = content.lines().collect();
                        let insert_pos = lines
                            .iter()
                            .position(|l| l.starts_with("#+title:"))
                            .map(|i| i + 1)
                            .unwrap_or(lines.len());
                        lines.insert(insert_pos, &next_link_line);
                        let updated = lines.join("\n") + "\n";
                        self.kb.write_guard.insert(prev_path.clone());
                        if std::fs::write(&prev_path, &updated).is_ok() {
                            self.kb_reimport_file(&prev_path);
                            self.kb.watcher_stats.reimports_total += 1;
                            result.links_inserted += 1;
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Open today's daily with chain-fill.
    pub fn kb_goto_daily_today(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        self.kb_daily_chain_fill(y, m, d)?;
        let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Open yesterday's daily.
    pub fn kb_goto_daily_yesterday(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        let (py, pm, pd) = mae_kb::activity::prev_day(y, m, d);
        if !self.kb_daily_exists(py, pm, pd) {
            self.kb_create_daily_stub(py, pm, pd)?;
        }
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to previous daily from current buffer's date.
    pub fn kb_daily_prev(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (py, pm, pd) = self
            .kb_daily_find_nearest(y, m, d, -1)
            .ok_or("No previous daily found")?;
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to next daily from current buffer's date.
    pub fn kb_daily_next(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (ny, nm, nd) = self
            .kb_daily_find_nearest(y, m, d, 1)
            .ok_or("No next daily found")?;
        let path = self
            .kb_daily_path(ny, nm, nd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Open a daily for a specific date string (YYYY-MM-DD).
    pub fn kb_goto_daily_date(&mut self, date_str: &str) -> Result<(), String> {
        let (y, m, d) = mae_kb::activity::parse_date(date_str)
            .ok_or_else(|| format!("Invalid date: '{}' (expected YYYY-MM-DD)", date_str))?;
        self.kb_daily_chain_fill(y, m, d)?;
        let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Extract a date from the current buffer's filename or title.
    fn kb_daily_date_from_buffer(&self) -> Result<(i32, u32, u32), String> {
        let buf = &self.buffers[self.active_buffer_idx()];
        // Try filename: YYYY-MM-DD.org
        if let Some(fp) = buf.file_path() {
            if let Some(stem) = fp.file_stem().and_then(|s| s.to_str()) {
                if let Some(date) = mae_kb::activity::parse_date(stem) {
                    return Ok(date);
                }
            }
        }
        // Try title line: #+title: YYYY-MM-DD
        let content = buf.text();
        for line in content.lines().take(10) {
            if let Some(rest) = line.strip_prefix("#+title:") {
                let trimmed = rest.trim();
                if let Some(date) = mae_kb::activity::parse_date(trimmed) {
                    return Ok(date);
                }
            }
        }
        Err("Current buffer is not a daily note".to_string())
    }

    /// Open a file at a given path (helper for dailies navigation).
    pub(crate) fn open_file_at_path(&mut self, path: &std::path::Path) {
        // Check if buffer already open
        for (i, buf) in self.buffers.iter().enumerate() {
            if buf.file_path().map(|p| p == path).unwrap_or(false) {
                self.display_buffer(i);
                return;
            }
        }
        // Open new buffer
        match crate::buffer::Buffer::from_file(path) {
            Ok(mut buf) => {
                buf.name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("daily")
                    .to_string();
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;

                // Language detection (same as open_file_hidden in file_ops.rs)
                let detected_lang = self.buffers[idx]
                    .file_path()
                    .and_then(|p| crate::syntax::language_for_buffer(p, &self.buffers[idx].text()));
                if let Some(lang) = detected_lang {
                    self.syntax.set_language(idx, lang);
                    self.buffers[idx]
                        .local_options
                        .apply_defaults(&lang.default_local_options());
                }

                self.display_buffer(idx);
            }
            Err(e) => {
                self.set_status(format!("Failed to open daily: {}", e));
            }
        }
    }

    // --- Graph KB dispatch helpers (CozoDB backend) ---

    /// Show text content in a read-only scratch buffer.
    fn show_scratch_buffer(&mut self, name: &str, content: &str) {
        let mut buf = crate::buffer::Buffer::new();
        buf.name = name.to_string();
        buf.replace_contents(content);
        buf.modified = false;
        buf.read_only = true;
        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.display_buffer(buf_idx);
    }

    /// Dispatch `:kb-agenda` with a filter string.
    pub fn dispatch_kb_agenda(&mut self, input: &str) {
        use mae_kb::AgendaFilter;

        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let filter = match parts[0] {
            "todo" => AgendaFilter::Todo(parts.get(1).map(|s| s.trim().to_string())),
            "priority" => {
                let ch = parts
                    .get(1)
                    .and_then(|s| s.trim().chars().next())
                    .unwrap_or('A');
                AgendaFilter::Priority(ch)
            }
            "tag" => match parts.get(1) {
                Some(t) => AgendaFilter::Tag(t.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda tag <TAG>");
                    return;
                }
            },
            "orphan" => AgendaFilter::Orphan,
            "dead-end" | "deadend" => AgendaFilter::DeadEnd,
            "stale" => {
                let days = parts
                    .get(1)
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(30);
                AgendaFilter::Stale(days)
            }
            "custom" => match parts.get(1) {
                Some(q) => AgendaFilter::Custom(q.trim().to_string()),
                None => {
                    self.set_status("Usage: :kb-agenda custom <datalog-query>");
                    return;
                }
            },
            other => {
                self.set_status(format!(
                    "Unknown filter '{}'. Use: todo, priority, tag, orphan, dead-end, stale, custom",
                    other
                ));
                return;
            }
        };

        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.agenda_query(&filter) {
            Ok(nodes) => {
                let mut lines = Vec::new();
                lines.push(format!("KB Agenda: {} results", nodes.len()));
                lines.push("=".repeat(40));
                lines.push(String::new());
                for node in &nodes {
                    let todo = match &node.todo_state {
                        Some(s) if !s.is_empty() => format!(" [{}]", s),
                        _ => String::new(),
                    };
                    let prio = match node.priority {
                        Some(c) => format!(" #{}", c),
                        None => String::new(),
                    };
                    lines.push(format!("  {}{}{} — {}", node.id, todo, prio, node.title));
                }
                if nodes.is_empty() {
                    lines.push("  (no matching nodes)".to_string());
                }
                self.show_scratch_buffer("*KB Agenda*", &lines.join("\n"));
            }
            Err(e) => {
                self.set_status(format!("Agenda query failed: {}", e));
            }
        }
    }

    /// Dispatch `:kb-history <node-id>`.
    pub fn dispatch_kb_history(&mut self, id: &str) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.node_history(id, 50) {
            Ok(versions) => {
                let mut lines = Vec::new();
                lines.push(format!(
                    "Version History: {} ({} versions)",
                    id,
                    versions.len()
                ));
                lines.push("=".repeat(50));
                lines.push(String::new());
                for v in &versions {
                    let ts = if v.created_at > 0 {
                        format!(" @{}", v.created_at)
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "  v{}: {} [{}]{} — {}",
                        v.version, v.title, v.author, ts, v.change_summary
                    ));
                }
                if versions.is_empty() {
                    lines.push("  (no version history)".to_string());
                }
                self.show_scratch_buffer("*KB History*", &lines.join("\n"));
            }
            Err(e) => {
                self.set_status(format!("History query failed: {}", e));
            }
        }
    }

    /// Dispatch `:kb-restore <node-id> <version>`.
    pub fn dispatch_kb_restore(&mut self, id: &str, version: i64) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.restore_version(id, version) {
            Ok(()) => {
                self.set_status(format!("Restored '{}' to version {}", id, version));
            }
            Err(e) => {
                self.set_status(format!("Restore failed: {}", e));
            }
        }
    }

    /// Dispatch `:kb-raw-query <datalog>`.
    pub fn dispatch_kb_raw_query(&mut self, query: &str) {
        let store = match &self.kb.store {
            Some(s) => s.clone(),
            None => {
                self.set_status("No persistent KB store (CozoDB required)");
                return;
            }
        };

        match store.raw_query(query) {
            Ok((headers, rows)) => {
                let mut lines = Vec::new();
                lines.push(format!("Datalog Query Results ({} rows)", rows.len()));
                lines.push("=".repeat(50));
                lines.push(String::new());

                if !headers.is_empty() {
                    lines.push(format!("  {}", headers.join(" | ")));
                    lines.push(format!("  {}", "-".repeat(headers.len() * 15)));
                }

                for row in &rows {
                    lines.push(format!("  {}", row.join(" | ")));
                }
                if rows.is_empty() {
                    lines.push("  (no results)".to_string());
                }
                self.show_scratch_buffer("*KB Query*", &lines.join("\n"));
            }
            Err(e) => {
                self.set_status(format!("Query failed: {}", e));
            }
        }
    }

    // --- Meta-node narrow/widen editing (Phase 7) ---

    /// Narrow to a meta-node component for editing.
    ///
    /// If the current help buffer shows a meta-node, presents its members
    /// for selection. On selection, opens the member node's body in a
    /// new buffer for editing.
    pub fn kb_narrow_meta(&mut self) {
        // Get current KB view's node ID.
        let node_id = match self.buffers[self.active_buffer_idx()].kb_view() {
            Some(hv) => hv.current.clone(),
            None => {
                self.set_status("kb-narrow: not in a KB view");
                return;
            }
        };

        // Query meta-node members from the store.
        let members = if let Some(ref store) = self.kb.store {
            match store.meta_members(&node_id) {
                Ok(m) if !m.is_empty() => m,
                Ok(_) => {
                    self.set_status(format!("'{}' has no meta-members", node_id));
                    return;
                }
                Err(e) => {
                    self.set_status(format!("kb-narrow: {}", e));
                    return;
                }
            }
        } else {
            self.set_status("kb-narrow: no KB store available");
            return;
        };

        // Build completion list from members.
        let items: Vec<(String, String)> = members
            .iter()
            .map(|m| {
                let title = if let Some(q) = self.kb.query_layer() {
                    q.get(&m.member_id).map(|n| n.title)
                } else {
                    self.kb.primary.get(&m.member_id).map(|n| n.title.clone())
                }
                .unwrap_or_else(|| m.member_id.clone());
                (m.member_id.clone(), format!("{} ({})", title, m.role))
            })
            .collect();

        // For simplicity, if there's only one member, open it directly.
        // Otherwise, show first member (full completion UI deferred).
        let member_id = &items[0].0;
        self.kb_open_member_for_editing(&node_id, member_id);
    }

    /// Open a meta-node member for editing in a new buffer.
    ///
    /// Buffer name encodes both IDs: `*kb-narrow:META_ID:MEMBER_ID*`
    fn kb_open_member_for_editing(&mut self, meta_id: &str, member_id: &str) {
        let node = if let Some(q) = self.kb.query_layer() {
            q.get(member_id)
        } else {
            self.kb.primary.get(member_id).cloned()
        };
        let node = match node {
            Some(n) => n,
            None => {
                self.set_status(format!("Node '{}' not found", member_id));
                return;
            }
        };

        // Create an edit buffer with the node's body.
        let buf_name = format!("*kb-narrow:{}:{}*", meta_id, member_id);
        let mut buf = crate::Buffer::new();
        buf.name = buf_name;
        buf.insert_text_at(0, &node.body);
        buf.modified = false;

        self.buffers.push(buf);
        let idx = self.buffers.len() - 1;
        self.display_buffer(idx);
        self.set_status(format!(
            "Narrowed to '{}' — :kb-widen to save and return",
            member_id
        ));
    }

    /// Parse meta_id and member_id from a `*kb-narrow:META:MEMBER*` buffer name.
    fn parse_narrow_buffer_name(name: &str) -> Option<(String, String)> {
        let inner = name.strip_prefix("*kb-narrow:")?.strip_suffix('*')?;
        let colon = inner.find(':')?;
        let meta_id = &inner[..colon];
        let member_id = &inner[colon + 1..];
        if meta_id.is_empty() || member_id.is_empty() {
            return None;
        }
        Some((meta_id.to_string(), member_id.to_string()))
    }

    /// Save edits from a narrowed meta-node component and widen back.
    pub fn kb_widen_meta(&mut self) {
        let idx = self.active_buffer_idx();
        let buf_name = self.buffers[idx].name.clone();

        // Check if this is a narrowed KB buffer.
        let (meta_id, member_id) = match Self::parse_narrow_buffer_name(&buf_name) {
            Some(ids) => ids,
            None => {
                self.set_status("kb-widen: not in a narrowed KB buffer");
                return;
            }
        };

        // Extract edited content.
        let new_body = self.buffers[idx].text().to_string();

        // Update the node in the primary KB.
        if let Some(node) = self.kb.primary.get_mut(&member_id) {
            node.body.clone_from(&new_body);
        }

        // Update in the CozoDB store if available.
        if let Some(ref store) = self.kb.store {
            if let Some(node) = self.kb.primary.get(&member_id) {
                let _ = store.save_all(&[node]);
            }
            // Recompose the meta-node body.
            if let Ok(composed) = store.compose_meta_body(&meta_id) {
                if let Some(meta_node) = self.kb.primary.get_mut(&meta_id) {
                    meta_node.body = composed;
                }
            }
        }

        // Close the narrow buffer and return.
        self.buffers.remove(idx);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx >= idx {
                win.buffer_idx = win.buffer_idx.saturating_sub(1);
            }
        }
        let ret = idx.min(self.buffers.len().saturating_sub(1));
        self.display_buffer(ret);
        self.set_status(format!("Widened from '{}', changes saved", member_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_org_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        // File with :ID:
        std::fs::write(
            dir.path().join("note1.org"),
            ":PROPERTIES:\n:ID: test-note-1\n:END:\n#+title: Note One\n\nBody of note one.\n",
        )
        .unwrap();
        // File with :ID: in subdir
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(
            sub.join("note2.org"),
            ":PROPERTIES:\n:ID: test-note-2\n:END:\n#+title: Note Two\n\nLinks to [[id:test-note-1][Note One]].\n",
        )
        .unwrap();
        // File without :ID: (should be skipped)
        std::fs::write(
            dir.path().join("no-id.org"),
            "#+title: No ID\n\nJust a note without an ID property.\n",
        )
        .unwrap();
        dir
    }

    /// Set config/data dir overrides to a tempdir so tests never touch
    /// real user directories (~/.config/mae, ~/.local/share/mae).
    fn with_test_dirs(editor: &mut Editor) -> TempDir {
        let tmp = TempDir::new().unwrap();
        editor.config_dir_override = Some(tmp.path().join("config"));
        editor.data_dir_override = Some(tmp.path().join("data"));
        tmp
    }

    #[test]
    fn open_file_at_path_detects_language() {
        let dir = TempDir::new().unwrap();
        let org_path = dir.path().join("test-daily.org");
        std::fs::write(&org_path, "#+title: Test\n* Heading\n").unwrap();

        let mut editor = Editor::new();
        editor.open_file_at_path(&org_path);

        let idx = editor.buffers.len() - 1;
        assert_eq!(
            editor.syntax.language_of(idx),
            Some(crate::syntax::Language::Org),
            "open_file_at_path must set Language::Org for .org files"
        );
    }

    #[test]
    fn kb_register_creates_instance() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path());
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.name, "TestNotes");
        assert_eq!(result.report.nodes_imported, 2);
        assert_eq!(result.report.nodes_skipped, 1); // no-id.org
        assert!(result.report.links_created >= 1); // note2 links to note1
        assert!(!result.uuid.is_empty());
        assert!(editor.kb.instances.contains_key(&result.uuid));
        assert_eq!(editor.kb.instances[&result.uuid].len(), 2);
    }

    #[test]
    fn kb_register_handles_subdirs() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        // note2.org is in subdir/ — must be found
        assert_eq!(result.report.nodes_imported, 2);
        let kb = &editor.kb.instances[&result.uuid];
        assert!(kb.get("test-note-2").is_some());
    }

    #[test]
    fn kb_unregister_removes_instance() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();
        assert!(editor.kb.instances.contains_key(&uuid));

        editor.kb_unregister("TestNotes");
        assert!(!editor.kb.instances.contains_key(&uuid));
        assert!(editor.kb.registry.find("TestNotes").is_none());
    }

    #[test]
    fn kb_reimport_refreshes_nodes() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();

        // Add a new file
        std::fs::write(
            dir.path().join("note3.org"),
            ":PROPERTIES:\n:ID: test-note-3\n:END:\n#+title: Note Three\n\nNew note.\n",
        )
        .unwrap();

        let result2 = editor.kb_reimport("TestNotes", None).unwrap();
        // Total nodes = imported (new) + updated (changed/existing)
        let total = result2.report.nodes_imported + result2.report.nodes_updated;
        assert_eq!(
            total, 3,
            "expected 3 total nodes (imported={}, updated={})",
            result2.report.nodes_imported, result2.report.nodes_updated
        );
        assert!(editor.kb.instances[&uuid].get("test-note-3").is_some());
    }

    #[test]
    fn kb_federated_search_finds_across_instances() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        editor.kb_register("TestNotes", dir.path());

        // Search should find nodes from federated instance
        let results = editor.kb_federated_search("Note");
        let federated: Vec<_> = results.iter().filter(|(name, _)| name.is_some()).collect();
        assert!(!federated.is_empty());
    }

    #[test]
    fn kb_federated_search_scope_filters_instances() {
        use mae_kb::KbScope;
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        editor.kb_register("TestNotes", dir.path());

        let count_federated = |r: &[(Option<String>, &mae_kb::Node)]| {
            r.iter().filter(|(name, _)| name.is_some()).count()
        };

        // All: includes the federated TestNotes instance.
        let all = editor.kb_federated_search_scoped("Note", &KbScope::All);
        assert!(count_federated(&all) > 0, "All should include federated");

        // LocalOnly: drops every federated result.
        let local = editor.kb_federated_search_scoped("Note", &KbScope::LocalOnly);
        assert_eq!(count_federated(&local), 0, "LocalOnly excludes federated");

        // Named: selects exactly the named instance's results.
        let named = editor.kb_federated_search_scoped("Note", &KbScope::Named("TestNotes".into()));
        assert!(count_federated(&named) > 0, "Named selects the instance");
        assert!(
            named
                .iter()
                .all(|(name, _)| name.is_none() || name.as_deref() == Some("TestNotes")),
            "Named yields only that instance (+ local)"
        );

        // RemoteOnly: TestNotes is a local import (not shared), so no results.
        let remote = editor.kb_federated_search_scoped("Note", &KbScope::RemoteOnly);
        assert_eq!(
            count_federated(&remote),
            0,
            "RemoteOnly excludes non-shared local imports"
        );
    }

    #[test]
    fn kb_search_recency_floats_visited_to_top() {
        let mut editor = Editor::new();
        editor.kb.search_sort = "recency".to_string();

        // Pick two nodes that both match a common query but aren't the top
        // relevance hit, then visit the second one and confirm it leads.
        let baseline = editor.kb_federated_search("buffer");
        assert!(baseline.len() >= 2, "need ≥2 matches for the query");
        // A match that is NOT already first under relevance.
        let promote = baseline[1].1.id.clone();

        // No visits yet → recency order == relevance order (stable).
        let ids_before: Vec<String> = editor
            .kb_federated_search("buffer")
            .iter()
            .map(|(_, n)| n.id.clone())
            .collect();
        assert_eq!(ids_before.first(), Some(&baseline[0].1.id.clone()));

        // Visit the promoted node; it should now sort first.
        editor.kb.record_visit(&promote);
        let ids_after: Vec<String> = editor
            .kb_federated_search("buffer")
            .iter()
            .map(|(_, n)| n.id.clone())
            .collect();
        assert_eq!(
            ids_after.first(),
            Some(&promote),
            "visited node should float to the top under recency sort"
        );
    }

    #[test]
    fn kb_search_sort_option_accepts_recency() {
        let mut editor = Editor::new();
        assert!(editor.set_option("kb_search_sort", "recency").is_ok());
        assert_eq!(editor.kb.search_sort, "recency");
        assert_eq!(
            editor.get_option("kb_search_sort").map(|(v, _)| v),
            Some("recency".to_string())
        );
        // Invalid value is rejected and leaves the setting unchanged.
        assert!(editor.set_option("kb_search_sort", "bogus").is_err());
        assert_eq!(editor.kb.search_sort, "recency");
    }

    #[test]
    fn kb_search_scope_option_round_trip() {
        let mut editor = Editor::new();
        // Keywords always validate.
        for kw in ["all", "local", "remote"] {
            assert!(editor.set_option("kb_search_scope", kw).is_ok());
            assert_eq!(editor.kb.search_scope, kw);
        }
        // An unknown instance name is rejected (no instance registered).
        assert!(editor.set_option("kb_search_scope", "NoSuchKB").is_err());
        // A registered instance name validates.
        let dir = create_test_org_dir();
        let _test_dirs = with_test_dirs(&mut editor);
        editor.kb_register("TestNotes", dir.path());
        assert!(editor.set_option("kb_search_scope", "TestNotes").is_ok());
        assert_eq!(
            editor.get_option("kb_search_scope").map(|(v, _)| v),
            Some("TestNotes".to_string())
        );
    }

    #[test]
    fn kb_visit_log_is_monotonic() {
        let mut editor = Editor::new();
        editor.kb.record_visit("concept:buffer");
        editor.kb.record_visit("concept:window");
        editor.kb.record_visit("concept:buffer"); // re-visit bumps ahead
        assert!(editor.kb.visit_rank("concept:buffer") > editor.kb.visit_rank("concept:window"));
        assert_eq!(editor.kb.visit_rank("never-visited"), 0);
    }

    #[test]
    fn kb_federated_get_local_first() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        editor.kb_register("TestNotes", dir.path());

        // Get from federated instance
        let result = editor.kb_federated_get("test-note-1");
        assert!(result.is_some());
        let (inst_name, node) = result.unwrap();
        assert_eq!(inst_name, Some("TestNotes".to_string()));
        assert_eq!(node.title, "Note One");
    }

    #[test]
    fn kb_register_nonexistent_path() {
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("Bad", Path::new("/nonexistent/path"));
        assert!(result.is_none());
        assert!(editor.status_msg.contains("does not exist"));
    }

    #[test]
    fn kb_import_result_json() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let json = result.to_json();
        assert!(json.contains("\"name\": \"TestNotes\""));
        assert!(json.contains("\"nodes_imported\": 2"));
    }

    #[test]
    fn kb_create_node_inserts_into_local_kb() {
        let mut editor = Editor::new();
        let result = editor.kb_create_node(
            "user:test-note",
            "Test Note",
            "Hello",
            mae_kb::NodeKind::Note,
        );
        assert!(result.is_ok());
        let node = editor.kb.primary.get("user:test-note").unwrap();
        assert_eq!(node.title, "Test Note");
        assert_eq!(node.body, "Hello");
        assert_eq!(node.source, Some(mae_kb::NodeSource::Manual));
    }

    #[test]
    fn kb_create_node_rejects_seed_overwrite() {
        let mut editor = Editor::new();
        // "index" is a seed node
        let result = editor.kb_create_node("index", "Override", "bad", mae_kb::NodeKind::Note);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("seed node"));
    }

    #[test]
    fn kb_delete_node_removes_from_local_kb() {
        let mut editor = Editor::new();
        editor
            .kb_create_node("user:del-me", "Delete Me", "bye", mae_kb::NodeKind::Note)
            .unwrap();
        assert!(editor.kb.primary.get("user:del-me").is_some());
        let result = editor.kb_delete_node("user:del-me");
        assert!(result.is_ok());
        assert!(editor.kb.primary.get("user:del-me").is_none());
    }

    #[test]
    fn kb_delete_node_rejects_seed_deletion() {
        let mut editor = Editor::new();
        let result = editor.kb_delete_node("index");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("seed node"));
        // Confirm the node still exists
        assert!(editor.kb.primary.get("index").is_some());
    }

    #[test]
    fn kb_update_node_merges_fields() {
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "user:upd",
                "Original",
                "original body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let result = editor.kb_update_node(
            "user:upd",
            Some("Updated Title"),
            None,
            Some(vec!["tag1".into()]),
        );
        assert!(result.is_ok());
        let node = editor.kb.primary.get("user:upd").unwrap();
        assert_eq!(node.title, "Updated Title");
        assert_eq!(node.body, "original body"); // unchanged
        assert_eq!(node.tags, vec!["tag1".to_string()]);
    }

    #[test]
    fn watcher_starts_on_register() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        assert!(
            editor.kb.watchers.contains_key(&result.uuid),
            "watcher should start on register"
        );
    }

    #[test]
    fn watcher_removed_on_unregister() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();
        assert!(editor.kb.watchers.contains_key(&uuid));
        editor.kb_unregister("TestNotes");
        assert!(!editor.kb.watchers.contains_key(&uuid));
    }

    #[test]
    fn watcher_drains_new_file() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();

        // Write a new org file
        std::fs::write(
            dir.path().join("new-note.org"),
            ":PROPERTIES:\n:ID: watch-test-new\n:END:\n#+title: Watched Note\n\nNew.\n",
        )
        .unwrap();

        // Poll until watcher picks it up (filesystem events are async)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            editor.drain_kb_watchers();
            if editor.kb.instances[&uuid].get("watch-test-new").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            editor.kb.instances[&uuid].get("watch-test-new").is_some(),
            "new org file should be auto-ingested by watcher"
        );
    }

    // --- W1: KB options tests ---

    #[test]
    fn kb_options_registered() {
        let editor = Editor::new();
        for name in &[
            "kb_watcher_enabled",
            "kb_watcher_debounce_ms",
            "kb_max_drain_events",
            "kb_search_excerpt_length",
            "kb_search_max_results",
            "kb_auto_register",
        ] {
            assert!(
                editor.option_registry.find(name).is_some(),
                "option '{}' not found in registry",
                name
            );
        }
        // Also check aliases
        assert!(editor.option_registry.find("kb-watcher-enabled").is_some());
        assert!(editor.option_registry.find("kb-max-drain-events").is_some());
    }

    #[test]
    fn kb_options_get_set_roundtrip() {
        let mut editor = Editor::new();
        // Bool roundtrip
        assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "true");
        editor.set_option("kb_watcher_enabled", "false").unwrap();
        assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "false");
        // Int roundtrip
        editor.set_option("kb_watcher_debounce_ms", "1000").unwrap();
        assert_eq!(
            editor.get_option("kb_watcher_debounce_ms").unwrap().0,
            "1000"
        );
        editor.set_option("kb_max_drain_events", "50").unwrap();
        assert_eq!(editor.get_option("kb_max_drain_events").unwrap().0, "50");
        editor
            .set_option("kb_search_excerpt_length", "300")
            .unwrap();
        assert_eq!(
            editor.get_option("kb_search_excerpt_length").unwrap().0,
            "300"
        );
        editor.set_option("kb_search_max_results", "10").unwrap();
        assert_eq!(editor.get_option("kb_search_max_results").unwrap().0, "10");
        // Bool roundtrip
        editor.set_option("kb_auto_register", "true").unwrap();
        assert_eq!(editor.get_option("kb_auto_register").unwrap().0, "true");
    }

    // --- W4: Watcher hardening tests ---

    #[test]
    fn drain_debounce_skips_recent() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();

        // Write a file and wait for watcher to see it
        std::fs::write(
            dir.path().join("debounce-first.org"),
            ":PROPERTIES:\n:ID: debounce-first\n:END:\n#+title: First\n\ntest\n",
        )
        .unwrap();
        // Drain until first file is picked up (establishes timestamp)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            editor.drain_kb_watchers();
            if editor.kb.last_drain.contains_key(&uuid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(editor.kb.last_drain.contains_key(&uuid));

        // Now set a very long debounce
        editor.kb.watcher_debounce_ms = 60_000;

        // Write another file
        std::fs::write(
            dir.path().join("debounce-second.org"),
            ":PROPERTIES:\n:ID: debounce-second\n:END:\n#+title: Second\n\ntest\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));

        // This drain should be debounced — second node should NOT appear
        editor.drain_kb_watchers();
        assert!(
            editor.kb.instances[&uuid].get("debounce-second").is_none(),
            "debounce should have skipped the drain"
        );
    }

    #[test]
    fn watcher_disabled_skips_drain() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        editor.kb.watcher_enabled = false;
        // Register should skip watcher creation
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        assert!(
            !editor.kb.watchers.contains_key(&result.uuid),
            "watcher should not be created when disabled"
        );
        // drain should be a no-op
        editor.drain_kb_watchers();
    }

    #[test]
    fn watcher_error_count_exposed() {
        let dir = create_test_org_dir();
        let watcher = mae_kb::watch::OrgDirWatcher::new(dir.path()).unwrap();
        // Initial error count should be 0
        assert_eq!(watcher.error_count(), 0);
    }

    #[test]
    fn kb_federated_search_deduplicates() {
        let mut editor = Editor::new();
        // Insert a node locally
        editor
            .kb_create_node("dedup-test", "Dedup", "body", mae_kb::NodeKind::Note)
            .unwrap();
        // Insert same node in a federated instance
        let mut inst = mae_kb::KnowledgeBase::new();
        inst.insert(mae_kb::Node::new(
            "dedup-test",
            "Dedup",
            mae_kb::NodeKind::Note,
            "body",
        ));
        editor.kb.instances.insert("inst-1".to_string(), inst);

        let results = editor.kb_federated_search("Dedup");
        let dedup_count = results.iter().filter(|(_, n)| n.id == "dedup-test").count();
        assert_eq!(dedup_count, 1, "same node ID should appear only once");
        // Local result should win (instance_name is None)
        let (inst_name, _) = results.iter().find(|(_, n)| n.id == "dedup-test").unwrap();
        assert!(
            inst_name.is_none(),
            "local result should win over federated"
        );
    }

    // --- W5: Observability tests ---

    #[test]
    fn kb_watcher_stats_update_on_drain() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();

        // Write a new file and wait for watcher
        std::fs::write(
            dir.path().join("stats-test.org"),
            ":PROPERTIES:\n:ID: stats-test\n:END:\n#+title: Stats\n\ntest\n",
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            editor.drain_kb_watchers();
            if editor.kb.instances[&uuid].get("stats-test").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        assert!(
            editor.kb.watcher_stats.events_upserted > 0,
            "events_upserted should be positive after drain"
        );
    }

    #[test]
    fn perf_stats_kb_fields_default_zero() {
        let editor = Editor::new();
        assert_eq!(editor.perf_stats.kb_search_latency_us, 0);
        assert_eq!(editor.perf_stats.kb_watcher_drain_us, 0);
        assert_eq!(editor.perf_stats.kb_watcher_events, 0);
    }

    #[test]
    fn kb_register_does_not_clobber_user_dirs() {
        // Resolve real user dirs the same way the production code does.
        let home = std::env::var("HOME").unwrap();
        let real_config = PathBuf::from(&home).join(".config/mae/kb-registry.toml");
        let real_data = PathBuf::from(&home).join(".local/share/mae/kb-registry.toml");

        // Record mtimes before
        let config_mtime = real_config.metadata().ok().and_then(|m| m.modified().ok());
        let data_mtime = real_data.metadata().ok().and_then(|m| m.modified().ok());

        // Run a register + unregister cycle with test dirs
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let result = editor.kb_register("IsolationTest", dir.path()).unwrap();
        editor.kb_unregister(&result.uuid);

        // Verify mtimes unchanged
        let config_mtime_after = real_config.metadata().ok().and_then(|m| m.modified().ok());
        let data_mtime_after = real_data.metadata().ok().and_then(|m| m.modified().ok());
        assert_eq!(
            config_mtime, config_mtime_after,
            "config dir kb-registry.toml was modified by test"
        );
        assert_eq!(
            data_mtime, data_mtime_after,
            "data dir kb-registry.toml was modified by test"
        );
    }
}
