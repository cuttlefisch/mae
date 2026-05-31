//! KB Federation — multi-KB registry and cross-instance operations.
//!
//! SQLite is the durable source of truth for KB data.
//! Org directories are an import/export format, not the runtime store.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Registry of all known KB instances. Persisted as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KbRegistry {
    pub instances: Vec<KbInstance>,
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

/// Import report from ingesting an org directory.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub nodes_imported: usize,
    pub nodes_skipped: usize,
    pub links_created: usize,
    pub duplicate_ids: Vec<(String, PathBuf)>,
    pub errors: Vec<(PathBuf, String)>,
    pub path_to_ids: Vec<(std::path::PathBuf, Vec<String>)>,
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
fn generate_uuid() -> String {
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
