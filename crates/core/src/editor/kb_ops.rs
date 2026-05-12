//! KB federation operations: register, unregister, reimport.

use std::path::{Path, PathBuf};

use mae_kb::federation::{ImportHealth, ImportReport};

use super::Editor;

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
    fn mae_config_dir() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            Some(PathBuf::from(xdg).join("mae"))
        } else if let Ok(home) = std::env::var("HOME") {
            Some(PathBuf::from(home).join(".config").join("mae"))
        } else {
            None
        }
    }

    /// Resolve the MAE data directory (~/.local/share/mae).
    fn mae_data_dir() -> Option<PathBuf> {
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

        let Some(config_dir) = Self::mae_config_dir() else {
            self.set_status("KB register error: cannot determine config directory");
            return None;
        };
        let Some(data_dir) = Self::mae_data_dir() else {
            self.set_status("KB register error: cannot determine data directory");
            return None;
        };
        let _ = std::fs::create_dir_all(&data_dir);

        let uuid = self
            .kb_registry
            .register(name.to_string(), org_dir.to_path_buf(), &data_dir);

        // Import org files recursively
        let (kb, report, health) = mae_kb::federation::import_org_dir(org_dir);

        // Store the instance
        self.kb_instances.insert(uuid.clone(), kb);

        // Start file watcher for live updates
        if let Ok(watcher) = mae_kb::watch::OrgDirWatcher::new(org_dir) {
            watcher.seed(
                report
                    .path_to_ids
                    .iter()
                    .map(|(p, ids)| (p.clone(), ids.clone())),
            );
            self.kb_watchers.insert(uuid.clone(), watcher);
        }

        // Update last_import timestamp
        if let Some(inst) = self
            .kb_registry
            .instances
            .iter_mut()
            .find(|i| i.uuid == uuid)
        {
            inst.last_import = Some(chrono_now());
        }

        // Persist registry
        let _ = self.kb_registry.save(&config_dir);

        let result = KbImportResult {
            name: name.to_string(),
            uuid,
            report,
            health,
        };

        self.set_status(result.status_summary());
        Some(result)
    }

    /// Unregister a KB instance by name or UUID.
    pub fn kb_unregister(&mut self, name_or_uuid: &str) {
        let found = self.kb_registry.find(name_or_uuid).map(|i| i.uuid.clone());
        match found {
            Some(uuid) => {
                self.kb_instances.remove(&uuid);
                self.kb_watchers.remove(&uuid);
                self.kb_registry.unregister(name_or_uuid);
                if let Some(config_dir) = Self::mae_config_dir() {
                    let _ = self.kb_registry.save(&config_dir);
                }
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
    pub fn kb_reimport(&mut self, name_or_uuid: &str) -> Option<KbImportResult> {
        let inst = self.kb_registry.find(name_or_uuid).cloned();
        match inst {
            Some(instance) => {
                let (kb, report, health) = mae_kb::federation::import_org_dir(&instance.org_dir);
                self.kb_instances.insert(instance.uuid.clone(), kb);

                // Update timestamp
                if let Some(reg_inst) = self
                    .kb_registry
                    .instances
                    .iter_mut()
                    .find(|i| i.uuid == instance.uuid)
                {
                    reg_inst.last_import = Some(chrono_now());
                }
                if let Some(config_dir) = Self::mae_config_dir() {
                    let _ = self.kb_registry.save(&config_dir);
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
        if let Some(existing) = self.kb.get(id) {
            if existing.source == Some(mae_kb::NodeSource::Seed) {
                return Err(format!(
                    "Cannot overwrite seed node '{}' — built-in help is protected",
                    id
                ));
            }
        }
        let node =
            mae_kb::Node::new(id, title, kind, body).with_source(mae_kb::NodeSource::Manual, 0);
        self.kb.insert(node);
        self.set_status(format!("KB node created: {}", id));
        Ok(())
    }

    /// Delete a KB node from the local knowledge base.
    /// Rejects deleting seed nodes (built-in help).
    pub fn kb_delete_node(&mut self, id: &str) -> Result<(), String> {
        match self.kb.get(id) {
            None => Err(format!("No KB node: {}", id)),
            Some(node) if node.source == Some(mae_kb::NodeSource::Seed) => Err(format!(
                "Cannot delete seed node '{}' — built-in help is protected",
                id
            )),
            Some(_) => {
                self.kb.remove(id);
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
        self.kb.insert(updated);
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
        let count = self.kb_registry.instances.len();
        if self.kb_registry.instances.is_empty() {
            lines.push("  (none registered)".to_string());
        } else {
            for inst in &self.kb_registry.instances {
                let node_count = self
                    .kb_instances
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

    /// Search across local KB and all federated instances.
    /// Returns (instance_name_or_none, node) pairs.
    pub fn kb_federated_search(&self, query: &str) -> Vec<(Option<String>, &mae_kb::Node)> {
        let mut results: Vec<(Option<String>, &mae_kb::Node)> = Vec::new();

        // Local KB first
        for id in self.kb.search(query) {
            if let Some(node) = self.kb.get(&id) {
                results.push((None, node));
            }
        }

        // Then each federated instance
        for (uuid, kb) in &self.kb_instances {
            let inst_name = self.kb_registry.find_by_uuid(uuid).map(|i| i.name.clone());
            for id in kb.search(query) {
                if let Some(node) = kb.get(&id) {
                    results.push((inst_name.clone(), node));
                }
            }
        }

        results
    }

    /// Get a node by ID, searching local first then federated instances.
    pub fn kb_federated_get(&self, id: &str) -> Option<(Option<String>, &mae_kb::Node)> {
        if let Some(node) = self.kb.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.kb_instances {
            if let Some(node) = kb.get(id) {
                let name = self.kb_registry.find_by_uuid(uuid).map(|i| i.name.clone());
                return Some((name, node));
            }
        }
        None
    }

    /// Drain KB file watchers — apply changes from filesystem events.
    /// Called from `idle_work()` to pick up org file edits without `:kb-reimport`.
    pub fn drain_kb_watchers(&mut self) {
        let uuids: Vec<String> = self.kb_watchers.keys().cloned().collect();
        let mut changed = false;
        for uuid in uuids {
            let changes = match self.kb_watchers.get(&uuid) {
                Some(w) => w.drain(),
                None => continue,
            };
            if changes.is_empty() {
                continue;
            }
            for change in changes {
                match change {
                    mae_kb::watch::OrgChange::Upserted(path) => {
                        if let Some(kb) = self.kb_instances.get_mut(&uuid) {
                            let ids = kb.ingest_org_file(&path);
                            if let Some(w) = self.kb_watchers.get(&uuid) {
                                w.record_ids(path, ids);
                            }
                            changed = true;
                        }
                    }
                    mae_kb::watch::OrgChange::Removed(ids) => {
                        if let Some(kb) = self.kb_instances.get_mut(&uuid) {
                            for id in ids {
                                kb.remove(&id);
                            }
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.fire_hook("after-kb-change");
        }
    }
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

    #[test]
    fn kb_register_creates_instance() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path());
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.name, "TestNotes");
        assert_eq!(result.report.nodes_imported, 2);
        assert_eq!(result.report.nodes_skipped, 1); // no-id.org
        assert!(result.report.links_created >= 1); // note2 links to note1
        assert!(!result.uuid.is_empty());
        assert!(editor.kb_instances.contains_key(&result.uuid));
        assert_eq!(editor.kb_instances[&result.uuid].len(), 2);
    }

    #[test]
    fn kb_register_handles_subdirs() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        // note2.org is in subdir/ — must be found
        assert_eq!(result.report.nodes_imported, 2);
        let kb = &editor.kb_instances[&result.uuid];
        assert!(kb.get("test-note-2").is_some());
    }

    #[test]
    fn kb_unregister_removes_instance() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();
        assert!(editor.kb_instances.contains_key(&uuid));

        editor.kb_unregister("TestNotes");
        assert!(!editor.kb_instances.contains_key(&uuid));
        assert!(editor.kb_registry.find("TestNotes").is_none());
    }

    #[test]
    fn kb_reimport_refreshes_nodes() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();

        // Add a new file
        std::fs::write(
            dir.path().join("note3.org"),
            ":PROPERTIES:\n:ID: test-note-3\n:END:\n#+title: Note Three\n\nNew note.\n",
        )
        .unwrap();

        let result2 = editor.kb_reimport("TestNotes").unwrap();
        assert_eq!(result2.report.nodes_imported, 3);
        assert!(editor.kb_instances[&uuid].get("test-note-3").is_some());
    }

    #[test]
    fn kb_federated_search_finds_across_instances() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        editor.kb_register("TestNotes", dir.path());

        // Search should find nodes from federated instance
        let results = editor.kb_federated_search("Note");
        let federated: Vec<_> = results.iter().filter(|(name, _)| name.is_some()).collect();
        assert!(!federated.is_empty());
    }

    #[test]
    fn kb_federated_get_local_first() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
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
        let result = editor.kb_register("Bad", Path::new("/nonexistent/path"));
        assert!(result.is_none());
        assert!(editor.status_msg.contains("does not exist"));
    }

    #[test]
    fn kb_import_result_json() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
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
        let node = editor.kb.get("user:test-note").unwrap();
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
        assert!(editor.kb.get("user:del-me").is_some());
        let result = editor.kb_delete_node("user:del-me");
        assert!(result.is_ok());
        assert!(editor.kb.get("user:del-me").is_none());
    }

    #[test]
    fn kb_delete_node_rejects_seed_deletion() {
        let mut editor = Editor::new();
        let result = editor.kb_delete_node("index");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("seed node"));
        // Confirm the node still exists
        assert!(editor.kb.get("index").is_some());
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
        let node = editor.kb.get("user:upd").unwrap();
        assert_eq!(node.title, "Updated Title");
        assert_eq!(node.body, "original body"); // unchanged
        assert_eq!(node.tags, vec!["tag1".to_string()]);
    }

    #[test]
    fn watcher_starts_on_register() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        assert!(
            editor.kb_watchers.contains_key(&result.uuid),
            "watcher should start on register"
        );
    }

    #[test]
    fn watcher_removed_on_unregister() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
        let result = editor.kb_register("TestNotes", dir.path()).unwrap();
        let uuid = result.uuid.clone();
        assert!(editor.kb_watchers.contains_key(&uuid));
        editor.kb_unregister("TestNotes");
        assert!(!editor.kb_watchers.contains_key(&uuid));
    }

    #[test]
    fn watcher_drains_new_file() {
        let dir = create_test_org_dir();
        let mut editor = Editor::new();
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
            if editor.kb_instances[&uuid].get("watch-test-new").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            editor.kb_instances[&uuid].get("watch-test-new").is_some(),
            "new org file should be auto-ingested by watcher"
        );
    }
}
