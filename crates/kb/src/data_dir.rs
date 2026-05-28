//! Standardized KB data directory layout (XDG-compliant).
//!
//! All KB data lives under `$XDG_DATA_HOME/mae/kb/` with this structure:
//!
//! ```text
//! kb/
//!   registry.toml              # KB registry (migrated from kb-registry.toml)
//!   local/                     # Local-only KBs
//!     {kb-slug}/
//!       kb.sqlite              # Node storage
//!       meta.toml              # Name, UUID, node count, timestamps
//!   shared/                    # Collaborative KBs
//!     {kb-slug}/
//!       kb.sqlite              # Local CRDT-enabled storage
//!       meta.toml              # Name, collab_id, creator, peers, last_sync
//!   backups/
//!     {kb-slug}/
//!       {iso-timestamp}.sqlite # Periodic snapshots
//! ```
//!
//! ## Migration
//!
//! Old layout: `kb-registry.toml` + scattered `{uuid}.db` in data root.
//! Migration copies to new layout non-destructively.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Metadata for a local KB instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalKbMeta {
    pub name: String,
    pub uuid: String,
    pub created_at: String,
    pub node_count: usize,
    pub org_dir: Option<PathBuf>,
}

/// Metadata for a shared/collaborative KB instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedKbMeta {
    pub name: String,
    pub collab_id: String,
    pub creator: String,
    pub created_at: String,
    pub peers: Vec<String>,
    pub last_sync: Option<String>,
    pub sync_mode: String,
}

/// Manages the standardized KB data directory layout.
pub struct KbDataDir {
    root: PathBuf,
}

impl KbDataDir {
    /// Create a new KbDataDir rooted at `$XDG_DATA_HOME/mae/kb/`.
    ///
    /// Creates the directory structure if it doesn't exist.
    pub fn new(data_home: &Path) -> std::io::Result<Self> {
        let root = data_home.join("kb");
        std::fs::create_dir_all(root.join("local"))?;
        std::fs::create_dir_all(root.join("shared"))?;
        std::fs::create_dir_all(root.join("backups"))?;
        Ok(Self { root })
    }

    /// Create from an explicit root path (for testing).
    pub fn from_root(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(root.join("local"))?;
        std::fs::create_dir_all(root.join("shared"))?;
        std::fs::create_dir_all(root.join("backups"))?;
        Ok(Self { root })
    }

    /// Root directory for all KB data.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to the registry file.
    pub fn registry_path(&self) -> PathBuf {
        self.root.join("registry.toml")
    }

    // --- Local KB paths ---

    /// Directory for a local KB by slug.
    pub fn local_kb_dir(&self, slug: &str) -> PathBuf {
        self.root.join("local").join(slug)
    }

    /// SQLite path for a local KB.
    pub fn local_kb_db(&self, slug: &str) -> PathBuf {
        self.local_kb_dir(slug).join("kb.sqlite")
    }

    /// Metadata path for a local KB.
    pub fn local_kb_meta(&self, slug: &str) -> PathBuf {
        self.local_kb_dir(slug).join("meta.toml")
    }

    // --- Shared KB paths ---

    /// Directory for a shared KB by slug.
    pub fn shared_kb_dir(&self, slug: &str) -> PathBuf {
        self.root.join("shared").join(slug)
    }

    /// SQLite path for a shared KB.
    pub fn shared_kb_db(&self, slug: &str) -> PathBuf {
        self.shared_kb_dir(slug).join("kb.sqlite")
    }

    /// Metadata path for a shared KB.
    pub fn shared_kb_meta(&self, slug: &str) -> PathBuf {
        self.shared_kb_dir(slug).join("meta.toml")
    }

    // --- Backup paths ---

    /// Directory for backups of a specific KB.
    pub fn backup_dir(&self, slug: &str) -> PathBuf {
        self.root.join("backups").join(slug)
    }

    /// List all local KB slugs.
    pub fn list_local_kbs(&self) -> Vec<String> {
        list_subdirs(&self.root.join("local"))
    }

    /// List all shared KB slugs.
    pub fn list_shared_kbs(&self) -> Vec<String> {
        list_subdirs(&self.root.join("shared"))
    }

    /// Read metadata for a local KB.
    pub fn read_local_meta(&self, slug: &str) -> Option<LocalKbMeta> {
        let path = self.local_kb_meta(slug);
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }

    /// Write metadata for a local KB.
    pub fn write_local_meta(&self, slug: &str, meta: &LocalKbMeta) -> std::io::Result<()> {
        let dir = self.local_kb_dir(slug);
        std::fs::create_dir_all(&dir)?;
        let content =
            toml::to_string_pretty(meta).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(self.local_kb_meta(slug), content)
    }

    /// Read metadata for a shared KB.
    pub fn read_shared_meta(&self, slug: &str) -> Option<SharedKbMeta> {
        let path = self.shared_kb_meta(slug);
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }

    /// Write metadata for a shared KB.
    pub fn write_shared_meta(&self, slug: &str, meta: &SharedKbMeta) -> std::io::Result<()> {
        let dir = self.shared_kb_dir(slug);
        std::fs::create_dir_all(&dir)?;
        let content =
            toml::to_string_pretty(meta).map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(self.shared_kb_meta(slug), content)
    }

    /// Initialize a new local KB directory. Returns the db path.
    pub fn init_local_kb(&self, slug: &str, meta: &LocalKbMeta) -> std::io::Result<PathBuf> {
        let dir = self.local_kb_dir(slug);
        std::fs::create_dir_all(&dir)?;
        self.write_local_meta(slug, meta)?;
        info!(slug, "initialized local KB directory");
        Ok(self.local_kb_db(slug))
    }

    /// Initialize a new shared KB directory. Returns the db path.
    pub fn init_shared_kb(&self, slug: &str, meta: &SharedKbMeta) -> std::io::Result<PathBuf> {
        let dir = self.shared_kb_dir(slug);
        std::fs::create_dir_all(&dir)?;
        self.write_shared_meta(slug, meta)?;
        info!(slug, "initialized shared KB directory");
        Ok(self.shared_kb_db(slug))
    }
}

/// Slugify a name for filesystem use (lowercase, replace non-alnum with dash).
pub fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn list_subdirs(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| {
            let e = e.ok()?;
            if e.file_type().ok()?.is_dir() {
                e.file_name().to_str().map(String::from)
            } else {
                None
            }
        })
        .collect()
}

/// Migrate from old layout (`kb-registry.toml` + scattered `{uuid}.db`) to new layout.
///
/// Non-destructive: copies files to new locations. Returns number of KBs migrated.
pub fn migrate_legacy_layout(data_home: &Path) -> std::io::Result<usize> {
    let old_registry = data_home.join("kb-registry.toml");
    if !old_registry.exists() {
        debug!("no legacy KB registry found, skipping migration");
        return Ok(0);
    }

    let data_dir = KbDataDir::new(data_home)?;
    let content = std::fs::read_to_string(&old_registry)?;

    #[derive(Deserialize)]
    struct LegacyRegistry {
        instances: Vec<LegacyInstance>,
    }
    #[derive(Deserialize)]
    struct LegacyInstance {
        uuid: String,
        name: String,
        org_dir: PathBuf,
        db_path: PathBuf,
        enabled: bool,
        last_import: Option<String>,
    }

    let registry: LegacyRegistry = match toml::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to parse legacy KB registry");
            return Ok(0);
        }
    };

    let mut migrated = 0;
    for inst in &registry.instances {
        if !inst.enabled {
            continue;
        }
        let slug = slugify(&inst.name);
        let target_dir = data_dir.local_kb_dir(&slug);
        if target_dir.exists() {
            debug!(slug, "KB directory already exists, skipping");
            continue;
        }

        std::fs::create_dir_all(&target_dir)?;

        // Copy the SQLite database if it exists
        if inst.db_path.exists() {
            let target_db = data_dir.local_kb_db(&slug);
            std::fs::copy(&inst.db_path, &target_db)?;
            info!(
                slug,
                src = %inst.db_path.display(),
                dst = %target_db.display(),
                "migrated KB database"
            );
        }

        // Write meta.toml
        let now = chrono_now_iso();
        let meta = LocalKbMeta {
            name: inst.name.clone(),
            uuid: inst.uuid.clone(),
            created_at: inst.last_import.clone().unwrap_or_else(|| now.clone()),
            node_count: 0,
            org_dir: Some(inst.org_dir.clone()),
        };
        data_dir.write_local_meta(&slug, &meta)?;
        migrated += 1;
    }

    if migrated > 0 {
        // Copy old registry to new location for reference
        let new_registry = data_dir.registry_path();
        if !new_registry.exists() {
            std::fs::copy(&old_registry, &new_registry)?;
        }
        info!(
            count = migrated,
            "migrated legacy KB instances to new layout"
        );
    }

    Ok(migrated)
}

pub(crate) fn chrono_now_iso() -> String {
    // Simple ISO 8601 without chrono dependency
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}Z", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Research Notes"), "research-notes");
        assert_eq!(slugify("my_KB 2"), "my-kb-2");
        assert_eq!(slugify("---trimmed---"), "trimmed");
        assert_eq!(slugify("simple"), "simple");
    }

    #[test]
    fn data_dir_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        assert!(dir.root().join("local").is_dir());
        assert!(dir.root().join("shared").is_dir());
        assert!(dir.root().join("backups").is_dir());
    }

    #[test]
    fn local_kb_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        assert_eq!(
            dir.local_kb_dir("my-notes"),
            tmp.path().join("kb/local/my-notes")
        );
        assert_eq!(
            dir.local_kb_db("my-notes"),
            tmp.path().join("kb/local/my-notes/kb.sqlite")
        );
    }

    #[test]
    fn shared_kb_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        assert_eq!(
            dir.shared_kb_dir("research"),
            tmp.path().join("kb/shared/research")
        );
    }

    #[test]
    fn init_and_read_local_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        let meta = LocalKbMeta {
            name: "My Notes".to_string(),
            uuid: "abc-123".to_string(),
            created_at: "2026-05-28T12:00:00Z".to_string(),
            node_count: 42,
            org_dir: Some(PathBuf::from("/home/user/notes")),
        };

        let db = dir.init_local_kb("my-notes", &meta).unwrap();
        assert!(db.parent().unwrap().is_dir());

        let read_back = dir.read_local_meta("my-notes").unwrap();
        assert_eq!(read_back.name, "My Notes");
        assert_eq!(read_back.uuid, "abc-123");
        assert_eq!(read_back.node_count, 42);
    }

    #[test]
    fn init_and_read_shared_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        let meta = SharedKbMeta {
            name: "Research".to_string(),
            collab_id: "a7f3c2d1".to_string(),
            creator: "alice".to_string(),
            created_at: "2026-05-28T12:00:00Z".to_string(),
            peers: vec!["bob".to_string()],
            last_sync: None,
            sync_mode: "continuous".to_string(),
        };

        dir.init_shared_kb("research", &meta).unwrap();
        let read_back = dir.read_shared_meta("research").unwrap();
        assert_eq!(read_back.collab_id, "a7f3c2d1");
        assert_eq!(read_back.creator, "alice");
        assert_eq!(read_back.peers, vec!["bob"]);
    }

    #[test]
    fn list_kbs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        std::fs::create_dir_all(dir.local_kb_dir("notes-a")).unwrap();
        std::fs::create_dir_all(dir.local_kb_dir("notes-b")).unwrap();

        let mut kbs = dir.list_local_kbs();
        kbs.sort();
        assert_eq!(kbs, vec!["notes-a", "notes-b"]);
    }

    #[test]
    fn migrate_legacy_no_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let count = migrate_legacy_layout(tmp.path()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn migrate_legacy_with_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let data_home = tmp.path();

        // Create a legacy registry + db
        let db_path = data_home.join("abc-uuid.db");
        std::fs::write(&db_path, b"fake sqlite data").unwrap();

        let registry = format!(
            r#"[[instances]]
uuid = "abc-uuid"
name = "RoamNotes"
org_dir = "/home/user/RoamNotes"
db_path = "{}"
primary = true
enabled = true
"#,
            db_path.display()
        );
        std::fs::write(data_home.join("kb-registry.toml"), registry).unwrap();

        let count = migrate_legacy_layout(data_home).unwrap();
        assert_eq!(count, 1);

        // Verify new layout
        let kb_dir = KbDataDir::from_root(data_home.join("kb")).unwrap();
        assert!(kb_dir.local_kb_db("roamnotes").exists());
        let meta = kb_dir.read_local_meta("roamnotes").unwrap();
        assert_eq!(meta.name, "RoamNotes");
        assert_eq!(meta.uuid, "abc-uuid");
    }
}
