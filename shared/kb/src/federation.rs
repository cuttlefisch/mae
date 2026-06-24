//! KB Federation — multi-KB registry and cross-instance operations.
//!
//! CozoDB is the durable source of truth for KB data.
//! Org directories are an import/export format, not the runtime store.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::org::parse_org_multi_result;
use crate::store::KbStoreError;
use crate::{KnowledgeBase, Node};

/// A registered KB instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbInstance {
    pub uuid: String,
    pub name: String,
    pub org_dir: PathBuf,
    pub db_path: PathBuf,
    pub primary: bool,
    pub enabled: bool,
    pub last_import: Option<String>,
    /// Collaborative KB identity (FNV-1a hash of name + creator).
    /// Present only for shared KBs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collab_id: Option<String>,
    /// Whether this KB is shared with peers.
    #[serde(default)]
    pub shared: bool,
    /// Connected peers for this KB.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remote_peers: Vec<String>,
    /// Last sync timestamp (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
}

impl KbInstance {
    /// Whether this instance is shared/remote (collaborative). Used by
    /// `KbScope::RemoteOnly` to select only network-backed instances.
    pub fn is_remote(&self) -> bool {
        self.shared || self.collab_id.is_some() || !self.remote_peers.is_empty()
    }
}

/// Which federated KB instances participate in a search/traversal query.
///
/// This is a query-time selector, not new plumbing (plan decision D4): it
/// filters which of the primary + registered instances contribute results.
/// Parsed from the `kb_search_scope` option / AI-tool `scope` argument.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KbScope {
    /// Primary (local) KB + all enabled instances. Default.
    #[default]
    All,
    /// Only the primary (local) KB.
    LocalOnly,
    /// Only shared/remote (collaborative) instances; skip the primary.
    RemoteOnly,
    /// A single instance addressed by name (matches the primary's name too).
    Named(String),
}

impl KbScope {
    /// Parse a scope token from config / AI-tool input.
    /// `"" | "all"` → All, `"local"` → LocalOnly, `"remote"` → RemoteOnly,
    /// anything else → `Named(<trimmed>)`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "all" => KbScope::All,
            "local" | "local-only" | "localonly" => KbScope::LocalOnly,
            "remote" | "remote-only" | "remoteonly" => KbScope::RemoteOnly,
            _ => KbScope::Named(s.trim().to_string()),
        }
    }

    /// Canonical token for persistence / display.
    pub fn as_token(&self) -> String {
        match self {
            KbScope::All => "all".to_string(),
            KbScope::LocalOnly => "local".to_string(),
            KbScope::RemoteOnly => "remote".to_string(),
            KbScope::Named(n) => n.clone(),
        }
    }
}

/// Registry of all known KB instances. Persisted as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KbRegistry {
    pub instances: Vec<KbInstance>,
    /// Whether the **primary** KB is shared for collaboration (ADR-019). The
    /// primary KB has no `KbInstance` row, so its durable share marker lives
    /// here — making "is the primary KB syncing?" reconstructable across
    /// restarts instead of depending on a transient in-memory event.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary_shared: bool,
    /// Collaborative id the primary KB is shared under (when `primary_shared`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_collab_id: Option<String>,
}

impl KbRegistry {
    /// Load registry from `~/.local/share/mae/kb-registry.toml`.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("kb-registry.toml");
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save registry to `~/.local/share/mae/kb-registry.toml`.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let path = data_dir.join("kb-registry.toml");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| io::Error::other(e.to_string()))?;
        std::fs::write(&path, content)
    }

    /// Register a new org-roam directory.
    ///
    /// If a `KbDataDir` is provided, the SQLite database is placed in the
    /// standardized `kb/local/{slug}/kb.sqlite` layout. Otherwise falls back
    /// to the legacy `{data_dir}/{uuid}.db` flat layout.
    pub fn register(
        &mut self,
        name: String,
        org_dir: PathBuf,
        data_dir: &Path,
        kb_data_dir: Option<&crate::data_dir::KbDataDir>,
    ) -> String {
        // Check for existing registration with same path
        if let Some(existing) = self.instances.iter().find(|i| i.org_dir == org_dir) {
            return existing.uuid.clone();
        }

        // Check for sentinel file with existing UUID
        let uuid = read_sentinel_uuid(&org_dir).unwrap_or_else(generate_uuid);

        let slug = crate::data_dir::slugify(&name);
        let db_path = if let Some(kdd) = kb_data_dir {
            // Standardized layout: kb/local/{slug}/kb.sqlite
            let meta = crate::data_dir::LocalKbMeta {
                name: name.clone(),
                uuid: uuid.clone(),
                created_at: crate::data_dir::chrono_now_iso(),
                node_count: 0,
                org_dir: Some(org_dir.clone()),
            };
            match kdd.init_local_kb(&slug, &meta) {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!(error = %e, slug, "failed to init local KB dir, using legacy path");
                    data_dir.join(format!("{}.db", uuid))
                }
            }
        } else {
            // Legacy flat layout
            data_dir.join(format!("{}.db", uuid))
        };

        // Write sentinel file (idempotent)
        let _ = write_sentinel(&org_dir, &uuid, &name);

        let instance = KbInstance {
            uuid: uuid.clone(),
            name,
            org_dir,
            db_path,
            primary: self.instances.is_empty(),
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
        };
        self.instances.push(instance);
        uuid
    }

    /// Unregister an instance by name or UUID.
    pub fn unregister(&mut self, name_or_uuid: &str) {
        self.instances
            .retain(|i| i.name != name_or_uuid && i.uuid != name_or_uuid);
    }

    /// Find an instance by name or UUID.
    pub fn find(&self, name_or_uuid: &str) -> Option<&KbInstance> {
        self.instances
            .iter()
            .find(|i| i.name == name_or_uuid || i.uuid == name_or_uuid)
    }

    /// Find an instance by UUID.
    pub fn find_by_uuid(&self, uuid: &str) -> Option<&KbInstance> {
        self.instances.iter().find(|i| i.uuid == uuid)
    }

    /// Find a mutable instance by name or UUID (ADR-019: the share path stamps
    /// `shared`/`collab_id` durable markers).
    pub fn find_mut(&mut self, name_or_uuid: &str) -> Option<&mut KbInstance> {
        self.instances
            .iter_mut()
            .find(|i| i.name == name_or_uuid || i.uuid == name_or_uuid)
    }

    /// Find a shared instance by its collaborative id (ADR-019: receive-side
    /// routing + reconstruction resolve a `collab_id` back to its instance).
    pub fn find_by_collab_id(&self, collab_id: &str) -> Option<&KbInstance> {
        self.instances
            .iter()
            .find(|i| i.collab_id.as_deref() == Some(collab_id))
    }
}

/// Federated KB — wraps local KB plus imported instances.
#[derive(Debug, Default, Clone)]
pub struct FederatedKb {
    pub local: KnowledgeBase,
    pub instances: HashMap<String, KnowledgeBase>,
    pub registry: KbRegistry,
}

impl FederatedKb {
    pub fn new(local: KnowledgeBase) -> Self {
        FederatedKb {
            local,
            instances: HashMap::new(),
            registry: KbRegistry::default(),
        }
    }

    /// Search across local KB and all instances.
    pub fn search(&self, query: &str) -> Vec<(Option<&str>, &Node)> {
        let mut results: Vec<(Option<&str>, &Node)> = Vec::new();

        // Local KB first
        for id in self.local.search(query) {
            if let Some(node) = self.local.get(&id) {
                results.push((None, node));
            }
        }

        // Then each instance
        for (uuid, kb) in &self.instances {
            for id in kb.search(query) {
                if let Some(node) = kb.get(&id) {
                    results.push((Some(uuid.as_str()), node));
                }
            }
        }

        results
    }

    /// Get a node by ID, searching local first then instances.
    pub fn get(&self, id: &str) -> Option<(Option<&str>, &Node)> {
        if let Some(node) = self.local.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.instances {
            if let Some(node) = kb.get(id) {
                return Some((Some(uuid.as_str()), node));
            }
        }
        None
    }

    /// Get from a specific instance.
    pub fn get_from_instance(&self, uuid: &str, id: &str) -> Option<&Node> {
        self.instances.get(uuid)?.get(id)
    }

    /// Resolve an `eor:` link.
    /// Format: `eor:node-id` (local-first) or `eor:uuid/node-id` (targeted).
    pub fn resolve_eor_link<'a>(&'a self, link: &'a str) -> Option<(Option<&'a str>, &'a Node)> {
        let link = link.strip_prefix("eor:").unwrap_or(link);

        if let Some(slash_pos) = link.find('/') {
            // Targeted: eor:uuid/node-id
            let uuid = &link[..slash_pos];
            let node_id = &link[slash_pos + 1..];
            if let Some(node) = self.get_from_instance(uuid, node_id) {
                return Some((Some(uuid), node));
            }
            return None;
        }

        // Local-first
        self.get(link)
    }

    /// Number of total nodes across all KBs.
    pub fn total_nodes(&self) -> usize {
        self.local.len() + self.instances.values().map(|kb| kb.len()).sum::<usize>()
    }

    /// List instance names and node counts.
    pub fn instance_summary(&self) -> Vec<(String, String, usize, bool)> {
        self.registry
            .instances
            .iter()
            .map(|inst| {
                let count = self
                    .instances
                    .get(&inst.uuid)
                    .map(|kb| kb.len())
                    .unwrap_or(0);
                (inst.uuid.clone(), inst.name.clone(), count, inst.enabled)
            })
            .collect()
    }
}

/// How to ingest an external KB directory.
#[derive(Debug, Clone, Default)]
pub enum IngestMode {
    /// Re-parse all files. Existing nodes updated, deleted files' nodes removed.
    #[default]
    Full,
    /// Only re-parse files whose content hash has changed since last import.
    Incremental,
}

impl IngestMode {
    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "incremental" | "incr" => IngestMode::Incremental,
            _ => IngestMode::Full,
        }
    }
}

/// Import report from ingesting an org directory.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub nodes_imported: usize,
    pub nodes_skipped: usize,
    pub nodes_updated: usize,
    pub nodes_unchanged: usize,
    pub nodes_removed: usize,
    pub links_created: usize,
    pub duplicate_ids: Vec<(String, PathBuf)>,
    pub errors: Vec<(PathBuf, String)>,
    pub path_to_ids: Vec<(std::path::PathBuf, Vec<String>)>,
    pub mode: String,
    pub duration_ms: u64,
}

/// Health metrics computed after ingestion.
#[derive(Debug, Clone, Default)]
pub struct ImportHealth {
    pub total_nodes: usize,
    pub total_links: usize,
    pub orphan_count: usize,
    pub broken_link_count: usize,
    pub broken_link_deleted: usize,
    pub broken_link_malformed: usize,
    pub namespace_counts: HashMap<String, usize>,
}

impl ImportHealth {
    /// Compute health metrics from a freshly-imported KB.
    pub fn from_kb(kb: &KnowledgeBase) -> Self {
        let report = kb.health_report();
        Self {
            total_nodes: report.total_nodes,
            total_links: report.total_links,
            orphan_count: report.orphan_ids.len(),
            broken_link_count: report.broken_links.len(),
            broken_link_deleted: report
                .broken_links
                .iter()
                .filter(|b| b.kind == crate::BrokenLinkKind::DeletedNode)
                .count(),
            broken_link_malformed: report
                .broken_links
                .iter()
                .filter(|b| b.kind == crate::BrokenLinkKind::MalformedId)
                .count(),
            namespace_counts: report.namespace_counts,
        }
    }
}

/// Import an org-roam directory (recursively) into a MAE KB instance.
///
/// Uses `walkdir` to handle nested subdirectories. Skips the sentinel
/// file (`eor-instance.org`) and files without `:ID:` properties.
pub fn import_org_dir(org_dir: &Path) -> (KnowledgeBase, ImportReport, ImportHealth) {
    let mut kb = KnowledgeBase::new();
    let mut report = ImportReport::default();
    let mut seen_ids = std::collections::HashSet::new();
    let mut file_id_map: HashMap<PathBuf, Vec<String>> = HashMap::new();

    let walker = walkdir::WalkDir::new(org_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        // Skip sentinel file
        if path.file_name().and_then(|n| n.to_str()) == Some("eor-instance.org") {
            continue;
        }

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let nodes = crate::org::parse_org_multi(&content);
                if nodes.is_empty() {
                    report.nodes_skipped += 1;
                } else {
                    for mut node in nodes {
                        node.source_file = Some(path.to_path_buf());
                        report.links_created += node.links().len();
                        if seen_ids.insert(node.id.clone()) {
                            let nid = node.id.clone();
                            kb.insert(node);
                            report.nodes_imported += 1;
                            file_id_map.entry(path.to_path_buf()).or_default().push(nid);
                        } else {
                            report
                                .duplicate_ids
                                .push((node.id.clone(), path.to_path_buf()));
                        }
                    }
                }
            }
            Err(e) => {
                report.errors.push((path.to_path_buf(), e.to_string()));
            }
        }
    }

    report.path_to_ids = file_id_map.into_iter().collect();
    let health = ImportHealth::from_kb(&kb);
    (kb, report, health)
}

/// Import an org-roam directory directly into a CozoDB store.
///
/// Unlike `import_org_dir`, this writes nodes directly to CozoDB (no
/// intermediate in-memory KB). Supports full and incremental modes.
///
/// Returns a report and also populates an in-memory KB for the caller
/// to use as a read cache.
pub fn import_org_dir_to_store(
    org_dir: &Path,
    store: &crate::CozoKbStore,
    mode: &IngestMode,
) -> Result<(KnowledgeBase, ImportReport), KbStoreError> {
    use crate::store::KbStore;
    use sha2::{Digest, Sha256};

    let start = std::time::Instant::now();
    let mut kb = KnowledgeBase::new();
    let mut report = ImportReport {
        mode: format!("{mode:?}"),
        ..Default::default()
    };
    let mut seen_ids = std::collections::HashSet::new();

    let walker = walkdir::WalkDir::new(org_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    // Track which files we visit (for detecting deletions in Full mode).
    let mut visited_files = std::collections::HashSet::new();

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("eor-instance.org") {
            continue;
        }

        let file_path_str = path.to_string_lossy().to_string();
        visited_files.insert(file_path_str.clone());

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                report.errors.push((path.to_path_buf(), e.to_string()));
                continue;
            }
        };

        // Compute content hash for change detection.
        let content_hash = hex::encode(Sha256::digest(content.as_bytes()));

        // In incremental mode, skip files whose content hasn't changed.
        if matches!(mode, IngestMode::Incremental) {
            if let Ok(Some(stored_hash)) = store.get_source_file_hash(&file_path_str) {
                if stored_hash == content_hash {
                    // Content unchanged — load existing node IDs into in-memory KB.
                    if let Ok(node_ids) = store.get_source_file_node_ids(&file_path_str) {
                        for id in &node_ids {
                            if let Ok(Some(node)) = store.get_node(id) {
                                seen_ids.insert(id.clone());
                                kb.insert(node);
                            }
                        }
                    }
                    report.nodes_unchanged += 1;
                    continue;
                }
            }
        }

        // Parse with typed link support (query known rel types from store).
        let known_rel_types = store.known_rel_types().ok();
        let parse_result = parse_org_multi_result(&content, known_rel_types.as_ref());
        if parse_result.nodes.is_empty() {
            report.nodes_skipped += 1;
            continue;
        }

        let mut file_node_ids = Vec::new();
        for mut node in parse_result.nodes {
            node.source_file = Some(path.to_path_buf());
            report.links_created += node.links().len();

            if seen_ids.insert(node.id.clone()) {
                file_node_ids.push(node.id.clone());

                // Write to CozoDB.
                store.insert_node(&node)?;
                kb.insert(node);

                // Check if this was an update or new node.
                if let Ok(Some(old_hash)) = store.get_source_file_hash(&file_path_str) {
                    if !old_hash.is_empty() {
                        report.nodes_updated += 1;
                    } else {
                        report.nodes_imported += 1;
                    }
                } else {
                    report.nodes_imported += 1;
                }
            } else {
                report
                    .duplicate_ids
                    .push((node.id.clone(), path.to_path_buf()));
            }
        }

        // Wire typed links to CozoDB.
        for (src_id, link) in &parse_result.typed_links {
            if let Err(e) = store.add_typed_link(src_id, &link.target, &link.rel_type, 1.0) {
                tracing::debug!(src = %src_id, dst = %link.target, rel = %link.rel_type, error = %e, "typed link insert failed");
            }
        }

        // Wire transclusions to meta_members.
        for (order, (meta_id, member_id, role)) in parse_result.transclusions.iter().enumerate() {
            if let Err(e) = store.add_meta_member(meta_id, member_id, order as i32, role) {
                tracing::debug!(meta = %meta_id, member = %member_id, error = %e, "meta_member insert failed");
            }
        }

        // Record source file metadata for incremental reimport.
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        store.record_source_file(&file_path_str, &content_hash, mtime, &file_node_ids)?;

        report.path_to_ids.push((path.to_path_buf(), file_node_ids));
    }

    // In full mode, detect deleted files and remove their nodes.
    if matches!(mode, IngestMode::Full) {
        if let Ok(tracked_files) = store.list_source_files() {
            for (tracked_path, _, _) in tracked_files {
                if !visited_files.contains(&tracked_path) {
                    // File was deleted — remove its nodes.
                    if let Ok(removed_ids) = store.remove_source_file(&tracked_path) {
                        report.nodes_removed += removed_ids.len();
                    }
                }
            }
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;
    Ok((kb, report))
}

/// Read UUID from sentinel file in org directory.
fn read_sentinel_uuid(org_dir: &Path) -> Option<String> {
    let sentinel = org_dir.join("eor-instance.org");
    if !sentinel.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&sentinel).ok()?;
    for line in content.lines() {
        if let Some(id) = line.strip_prefix(":ID:") {
            return Some(id.trim().to_string());
        }
    }
    None
}

/// Write sentinel file to org directory (idempotent).
fn write_sentinel(org_dir: &Path, uuid: &str, name: &str) -> io::Result<()> {
    let sentinel = org_dir.join("eor-instance.org");
    if sentinel.exists() {
        return Ok(()); // Don't overwrite
    }
    let content = format!(
        ":PROPERTIES:\n:ID: {}\n:END:\n#+title: {} (MAE KB Instance)\n\nThis file marks this directory as a MAE KB instance.\nIt is safe to delete — MAE will recreate it on next registration.\n",
        uuid, name
    );
    std::fs::write(&sentinel, content)
}

/// Generate a simple UUID-like string.
pub fn generate_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    format!(
        "{:016x}-{:04x}-4000-8000-{:012x}",
        ts & 0xFFFFFFFFFFFFFFFF,
        pid & 0xFFFF,
        ts >> 64
    )
}

/// Parse an `eor:` link into (optional_uuid, node_id).
pub fn parse_eor_link(link: &str) -> (Option<&str>, &str) {
    let link = link.strip_prefix("eor:").unwrap_or(link);
    if let Some(slash_pos) = link.find('/') {
        (Some(&link[..slash_pos]), &link[slash_pos + 1..])
    } else {
        (None, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

    #[test]
    fn kb_scope_parse_tokens() {
        assert_eq!(KbScope::parse(""), KbScope::All);
        assert_eq!(KbScope::parse("all"), KbScope::All);
        assert_eq!(KbScope::parse("ALL"), KbScope::All);
        assert_eq!(KbScope::parse("local"), KbScope::LocalOnly);
        assert_eq!(KbScope::parse("local-only"), KbScope::LocalOnly);
        assert_eq!(KbScope::parse("remote"), KbScope::RemoteOnly);
        assert_eq!(KbScope::parse("MyNotes"), KbScope::Named("MyNotes".into()));
        // Round-trip through the canonical token.
        assert_eq!(
            KbScope::parse(&KbScope::RemoteOnly.as_token()),
            KbScope::RemoteOnly
        );
        assert_eq!(
            KbScope::parse(&KbScope::Named("Work".into()).as_token()),
            KbScope::Named("Work".into())
        );
    }

    #[test]
    fn kb_instance_is_remote() {
        let mut inst = KbInstance {
            uuid: "u".into(),
            name: "n".into(),
            org_dir: PathBuf::from("/tmp/n"),
            db_path: PathBuf::from("/tmp/n.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
        };
        assert!(!inst.is_remote(), "plain local import is not remote");
        inst.shared = true;
        assert!(inst.is_remote(), "shared instance is remote");
        inst.shared = false;
        inst.remote_peers.push("peer1".into());
        assert!(inst.is_remote(), "instance with peers is remote");
    }

    #[test]
    fn registry_register_and_find() {
        let mut reg = KbRegistry::default();
        let tmp = std::env::temp_dir().join("mae-test-fed-1");
        let _ = std::fs::create_dir_all(&tmp);
        let data = std::env::temp_dir().join("mae-test-fed-data");
        let _ = std::fs::create_dir_all(&data);

        let uuid = reg.register("Test".to_string(), tmp.clone(), &data, None);
        assert!(!uuid.is_empty());
        assert!(reg.find("Test").is_some());
        assert!(reg.find(&uuid).is_some());

        // Idempotent
        let uuid2 = reg.register("Test2".to_string(), tmp.clone(), &data, None);
        assert_eq!(uuid, uuid2);
        assert_eq!(reg.instances.len(), 1);

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn registry_unregister() {
        let mut reg = KbRegistry::default();
        let tmp = std::env::temp_dir().join("mae-test-fed-2");
        let _ = std::fs::create_dir_all(&tmp);
        let data = std::env::temp_dir().join("mae-test-fed-data-2");
        let _ = std::fs::create_dir_all(&data);

        reg.register("Test".to_string(), tmp.clone(), &data, None);
        assert_eq!(reg.instances.len(), 1);
        reg.unregister("Test");
        assert_eq!(reg.instances.len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn federated_search_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new(
            "test-node",
            "Test Node",
            NodeKind::Note,
            "content",
        ));

        let fed = FederatedKb::new(local);
        let results = fed.search("test");
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_none()); // from local
    }

    #[test]
    fn federated_search_across_instances() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("local-node", "Local", NodeKind::Note, "local"));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("remote-node", "Remote", NodeKind::Note, "remote"));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("inst-1".to_string(), instance);

        let results = fed.search("node");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn federated_get_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("shared-id", "Local Version", NodeKind::Note, ""));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("shared-id", "Remote Version", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("inst-1".to_string(), instance);

        let result = fed.get("shared-id").unwrap();
        assert!(result.0.is_none()); // local wins
        assert_eq!(result.1.title, "Local Version");
    }

    #[test]
    fn resolve_eor_link_local_first() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("my-node", "Node", NodeKind::Note, ""));

        let fed = FederatedKb::new(local);
        let result = fed.resolve_eor_link("eor:my-node");
        assert!(result.is_some());
        assert!(result.unwrap().0.is_none());
    }

    #[test]
    fn resolve_eor_link_targeted() {
        let local = KnowledgeBase::new();
        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("target", "Target", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("uuid-123".to_string(), instance);

        let result = fed.resolve_eor_link("eor:uuid-123/target");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, Some("uuid-123"));
    }

    #[test]
    fn resolve_eor_link_not_found() {
        let fed = FederatedKb::new(KnowledgeBase::new());
        assert!(fed.resolve_eor_link("eor:nonexistent").is_none());
    }

    #[test]
    fn parse_eor_link_formats() {
        assert_eq!(parse_eor_link("eor:node-id"), (None, "node-id"));
        assert_eq!(
            parse_eor_link("eor:uuid/node-id"),
            (Some("uuid"), "node-id")
        );
        assert_eq!(parse_eor_link("node-id"), (None, "node-id"));
    }

    #[test]
    fn total_nodes_count() {
        let mut local = KnowledgeBase::new();
        local.insert(Node::new("a", "A", NodeKind::Note, ""));
        local.insert(Node::new("b", "B", NodeKind::Note, ""));

        let mut instance = KnowledgeBase::new();
        instance.insert(Node::new("c", "C", NodeKind::Note, ""));

        let mut fed = FederatedKb::new(local);
        fed.instances.insert("i1".to_string(), instance);
        assert_eq!(fed.total_nodes(), 3);
    }

    #[test]
    fn import_org_dir_populates_source_file() {
        let tmp = std::env::temp_dir().join("mae-test-source-file");
        let _ = std::fs::create_dir_all(&tmp);
        std::fs::write(
            tmp.join("note.org"),
            ":PROPERTIES:\n:ID: src-test-1\n:END:\n#+title: Source Test\n\nBody.\n",
        )
        .unwrap();
        let (kb, report, _health) = import_org_dir(&tmp);
        assert!(kb.get("src-test-1").is_some());
        let node = kb.get("src-test-1").unwrap();
        assert!(
            node.source_file.is_some(),
            "source_file should be populated"
        );
        assert!(node.source_file.as_ref().unwrap().ends_with("note.org"));
        // path_to_ids populated
        assert!(!report.path_to_ids.is_empty());
        let ids_for_note: Vec<_> = report
            .path_to_ids
            .iter()
            .filter(|(p, _)| p.ends_with("note.org"))
            .collect();
        assert!(!ids_for_note.is_empty());
        assert!(ids_for_note[0].1.contains(&"src-test-1".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sentinel_roundtrip() {
        let tmp = std::env::temp_dir().join("mae-test-sentinel");
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::remove_file(tmp.join("eor-instance.org"));

        assert!(read_sentinel_uuid(&tmp).is_none());
        write_sentinel(&tmp, "test-uuid-123", "MyKB").unwrap();
        assert_eq!(read_sentinel_uuid(&tmp), Some("test-uuid-123".to_string()));

        // Idempotent — doesn't overwrite
        write_sentinel(&tmp, "different-uuid", "Other").unwrap();
        assert_eq!(read_sentinel_uuid(&tmp), Some("test-uuid-123".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
