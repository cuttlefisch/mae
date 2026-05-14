//! Project detection and configuration.
//!
//! A "project" is a directory tree rooted at a `.project`, `.git`,
//! `Cargo.toml`, or similar marker. The optional `.project` TOML file
//! adds metadata (name, required resources, symlinks).

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};

/// Configuration loaded from a `.project` TOML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ProjectConfig {
    /// Project name. Supports both `name` and `project-name`.
    #[serde(alias = "project-name")]
    pub name: Option<String>,
    pub root_directory: Option<String>,
    pub required_resources: Option<Vec<String>>,
    pub workspaces: Option<Vec<String>>,
    #[serde(default)]
    pub symlinks: Vec<SymlinkEntry>,
    /// Dependencies — other projects to auto-clone (future).
    #[serde(default)]
    pub deps: Vec<String>,
}

/// A symlink entry in the `.project` file.
#[derive(Debug, Clone, Deserialize)]
pub struct SymlinkEntry {
    #[serde(alias = "targ")]
    pub target: String,
    pub link: String,
}

/// Detected project.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
    pub config: Option<ProjectConfig>,
}

impl Project {
    /// Load a project from a root directory, reading `.project` if present.
    pub fn from_root(root: PathBuf) -> Self {
        let project_file = root.join(".project");
        let config = if project_file.exists() {
            std::fs::read_to_string(&project_file)
                .ok()
                .and_then(|s| toml::from_str::<ProjectConfig>(&s).ok())
        } else {
            None
        };
        let name = config
            .as_ref()
            .and_then(|c| c.name.clone())
            .unwrap_or_else(|| {
                root.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unnamed".to_string())
            });
        Project { name, root, config }
    }
}

/// Anchor markers — VCS roots and `.project` files.  These are authoritative
/// project roots: if found, they win immediately over build markers.
const ANCHOR_MARKERS: &[&str] = &[".project", ".git", ".hg", ".svn"];

/// Build markers — present in both workspace roots and subcrates/subpackages.
/// Only used as a fallback when no anchor is found.
const BUILD_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "Makefile",
];

/// Walk up from `start` looking for a project root.
///
/// Anchors (VCS dirs, `.project`) win immediately.  Build markers
/// (`Cargo.toml`, `package.json`, …) are tracked as fallbacks so that a
/// subcrate `Cargo.toml` doesn't beat the workspace `.git`.
pub fn detect_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    let mut build_fallback: Option<PathBuf> = None;
    loop {
        // Anchors win immediately.
        for marker in ANCHOR_MARKERS {
            if dir.join(marker).exists() {
                return Some(dir);
            }
        }
        // Track nearest build marker as fallback.
        if build_fallback.is_none() {
            for marker in BUILD_MARKERS {
                if dir.join(marker).exists() {
                    build_fallback = Some(dir.clone());
                    break;
                }
            }
        }
        if !dir.pop() {
            return build_fallback;
        }
    }
}

/// Bounded list of recently used project roots.
#[derive(Debug, Clone)]
pub struct RecentProjects {
    roots: VecDeque<PathBuf>,
    cap: usize,
}

impl Default for RecentProjects {
    fn default() -> Self {
        Self::new(20)
    }
}

impl RecentProjects {
    pub fn new(cap: usize) -> Self {
        RecentProjects {
            roots: VecDeque::new(),
            cap,
        }
    }

    /// Push a project root, deduplicating and enforcing capacity.
    pub fn push(&mut self, root: PathBuf) {
        self.roots.retain(|r| r != &root);
        self.roots.push_front(root);
        while self.roots.len() > self.cap {
            self.roots.pop_back();
        }
    }

    /// Remove a project root from the list.
    pub fn remove(&mut self, root: &Path) {
        self.roots.retain(|r| r != root);
    }

    pub fn list(&self) -> &VecDeque<PathBuf> {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }
}

/// Bounded list of recently opened files.
#[derive(Debug, Clone)]
pub struct RecentFiles {
    files: VecDeque<PathBuf>,
    cap: usize,
}

impl Default for RecentFiles {
    fn default() -> Self {
        Self::new(100)
    }
}

impl RecentFiles {
    pub fn new(cap: usize) -> Self {
        RecentFiles {
            files: VecDeque::new(),
            cap,
        }
    }

    /// Push a file path, deduplicating and enforcing capacity.
    pub fn push(&mut self, path: PathBuf) {
        // Remove duplicate if present
        self.files.retain(|p| p != &path);
        self.files.push_front(path);
        while self.files.len() > self.cap {
            self.files.pop_back();
        }
    }

    pub fn list(&self) -> &VecDeque<PathBuf> {
        &self.files
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.iter().any(|p| p == path)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Filter recent files to those within a given directory.
    pub fn filter_by_dir(&self, dir: &Path) -> Vec<&PathBuf> {
        self.files.iter().filter(|p| p.starts_with(dir)).collect()
    }
}

// ---------------------------------------------------------------------------
// Persistent project list (projects.toml)
// ---------------------------------------------------------------------------

/// A single entry in the persistent project list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub root: PathBuf,
    pub name: String,
    pub last_opened: String, // ISO-8601
}

/// Persistent list of known projects, stored as TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectList {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

impl ProjectList {
    /// Load from `data_dir/projects.toml`.  Returns default on any error.
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("projects.toml");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save to `data_dir/projects.toml`.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        let path = data_dir.join("projects.toml");
        let _ = std::fs::create_dir_all(data_dir);
        let content = toml::to_string_pretty(self).map_err(io::Error::other)?;
        std::fs::write(&path, content)
    }

    /// Upsert: add or update timestamp.  Returns `true` if this is a new entry.
    pub fn touch(&mut self, root: PathBuf, name: String) -> bool {
        let now = now_iso8601();
        if let Some(entry) = self.projects.iter_mut().find(|e| e.root == root) {
            entry.last_opened = now;
            entry.name = name;
            false
        } else {
            self.projects.push(ProjectEntry {
                root,
                name,
                last_opened: now,
            });
            true
        }
    }

    /// Remove entry by root path.
    pub fn remove(&mut self, root: &Path) {
        self.projects.retain(|e| e.root != root);
    }

    /// Remove entries whose root is inside another entry's root.
    pub fn prune_subprojects(&mut self) {
        let roots: Vec<PathBuf> = self.projects.iter().map(|e| e.root.clone()).collect();
        self.projects
            .retain(|e| !roots.iter().any(|r| r != &e.root && e.root.starts_with(r)));
    }

    /// Remove entries whose root directory no longer exists on disk.
    pub fn prune_missing(&mut self) {
        self.projects.retain(|e| e.root.is_dir());
    }

    /// Sorted by `last_opened` descending (most recent first).
    pub fn sorted(&self) -> Vec<&ProjectEntry> {
        let mut refs: Vec<&ProjectEntry> = self.projects.iter().collect();
        refs.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        refs
    }

    /// Sync entries into a `RecentProjects` (for palette display).
    pub fn sync_to_recent(&self, recent: &mut RecentProjects) {
        for entry in self.sorted().iter().rev() {
            recent.push(entry.root.clone());
        }
    }
}

/// Simple ISO-8601 timestamp without pulling in `chrono`.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Approximate: good enough for ordering.  No TZ libs needed.
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    // Days since 1970-01-01
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    if mo <= 2 {
        days = y + 1;
    } else {
        days = y;
    }
    (days, mo, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detect_project_root_finds_git() {
        let dir = std::env::temp_dir().join("mae_proj_test_git");
        let _ = fs::create_dir_all(&dir);
        let _ = fs::create_dir(dir.join(".git"));
        assert_eq!(detect_project_root(&dir), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_project_root_finds_cargo_toml() {
        let dir = std::env::temp_dir().join("mae_proj_test_cargo");
        let sub = dir.join("sub");
        let _ = fs::create_dir_all(&sub);
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_project_root(&sub), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_project_root_project_file_wins() {
        let dir = std::env::temp_dir().join("mae_proj_test_prio");
        let _ = fs::create_dir_all(&dir);
        let _ = fs::create_dir(dir.join(".git"));
        fs::write(dir.join(".project"), "name = \"test\"\n").unwrap();
        // Both exist at same level — .project is checked first
        let root = detect_project_root(&dir).unwrap();
        assert_eq!(root, dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detect_project_root_stops_at_root() {
        // A path with no markers should return None eventually
        let result = detect_project_root(Path::new("/tmp/nonexistent_mae_test_xyz"));
        // /tmp might have something, but we're testing that it doesn't panic
        let _ = result;
    }

    #[test]
    fn project_config_parses_toml() {
        let toml_str = r#"
name = "Test Project"
root-directory = "~/src/test"
required-resources = ["README.md", "Cargo.toml"]
workspaces = ["Test Project"]

[[symlinks]]
target = "~/notes/readme.org"
link = "README.org"
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name.as_deref(), Some("Test Project"));
        assert_eq!(config.required_resources.as_ref().unwrap().len(), 2);
        assert_eq!(config.symlinks.len(), 1);
        assert_eq!(config.symlinks[0].target, "~/notes/readme.org");
    }

    #[test]
    fn project_config_optional_fields() {
        let toml_str = "name = \"Minimal\"\n";
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name.as_deref(), Some("Minimal"));
        assert!(config.root_directory.is_none());
        assert!(config.required_resources.is_none());
        assert!(config.symlinks.is_empty());
    }

    #[test]
    fn project_from_root_with_config() {
        let dir = std::env::temp_dir().join("mae_proj_test_fromroot");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join(".project"), "name = \"My Project\"\n").unwrap();
        let project = Project::from_root(dir.clone());
        assert_eq!(project.name, "My Project");
        assert!(project.config.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_from_root_without_config() {
        let dir = std::env::temp_dir().join("mae_proj_test_noconfig");
        let _ = fs::create_dir_all(&dir);
        let project = Project::from_root(dir.clone());
        assert_eq!(project.name, "mae_proj_test_noconfig");
        assert!(project.config.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_config_with_deps() {
        let toml_str = r#"
name = "With Deps"
deps = ["github.com/org/repo1", "github.com/org/repo2"]
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.deps.len(), 2);
    }

    #[test]
    fn project_config_alias_project_name() {
        let toml_str = "project-name = \"Aliased\"\n";
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name.as_deref(), Some("Aliased"));
    }

    #[test]
    fn symlink_entry_alias_targ() {
        let toml_str = r#"
[[symlinks]]
targ = "~/notes/foo.org"
link = "FOO.org"
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.symlinks[0].target, "~/notes/foo.org");
    }

    #[test]
    fn recent_files_push_and_dedup() {
        let mut rf = RecentFiles::new(5);
        rf.push(PathBuf::from("/a"));
        rf.push(PathBuf::from("/b"));
        rf.push(PathBuf::from("/a")); // duplicate
        assert_eq!(rf.len(), 2);
        // Most recent first
        assert_eq!(rf.list()[0], PathBuf::from("/a"));
        assert_eq!(rf.list()[1], PathBuf::from("/b"));
    }

    #[test]
    fn recent_files_bounded() {
        let mut rf = RecentFiles::new(3);
        for i in 0..5 {
            rf.push(PathBuf::from(format!("/file{}", i)));
        }
        assert_eq!(rf.len(), 3);
        // Should have most recent 3
        assert_eq!(rf.list()[0], PathBuf::from("/file4"));
    }

    #[test]
    fn recent_files_contains() {
        let mut rf = RecentFiles::new(10);
        rf.push(PathBuf::from("/test"));
        assert!(rf.contains(Path::new("/test")));
        assert!(!rf.contains(Path::new("/other")));
    }

    #[test]
    fn recent_files_filter_by_dir() {
        let mut rf = RecentFiles::new(10);
        rf.push(PathBuf::from("/proj/a.rs"));
        rf.push(PathBuf::from("/proj/b.rs"));
        rf.push(PathBuf::from("/other/c.rs"));
        let filtered = rf.filter_by_dir(Path::new("/proj"));
        assert_eq!(filtered.len(), 2);
    }

    // --- Anchor-first detection tests ---

    #[test]
    fn detect_project_root_prefers_git_over_subcrate_cargo() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Workspace root with .git
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]").unwrap();
        // Subcrate with its own Cargo.toml
        let subcrate = root.join("crates/core");
        fs::create_dir_all(&subcrate).unwrap();
        fs::write(subcrate.join("Cargo.toml"), "[package]").unwrap();

        // Starting from subcrate, should find workspace root (anchor), not subcrate
        let detected = detect_project_root(&subcrate).unwrap();
        assert_eq!(detected, root.to_path_buf());
    }

    #[test]
    fn detect_project_root_fallback_to_build_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No VCS, just Cargo.toml at root
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        let sub = root.join("src");
        fs::create_dir_all(&sub).unwrap();

        let detected = detect_project_root(&sub).unwrap();
        assert_eq!(detected, root.to_path_buf());
    }

    // --- ProjectList tests ---

    #[test]
    fn project_list_touch_upserts() {
        let mut pl = ProjectList::default();
        let is_new = pl.touch(PathBuf::from("/proj/a"), "A".into());
        assert!(is_new);
        assert_eq!(pl.projects.len(), 1);

        // Touch again — should update, not add
        let is_new2 = pl.touch(PathBuf::from("/proj/a"), "A-renamed".into());
        assert!(!is_new2);
        assert_eq!(pl.projects.len(), 1);
        assert_eq!(pl.projects[0].name, "A-renamed");
    }

    #[test]
    fn project_list_prune_subprojects() {
        let mut pl = ProjectList::default();
        pl.touch(PathBuf::from("/workspace"), "WS".into());
        pl.touch(PathBuf::from("/workspace/crates/core"), "Core".into());
        pl.touch(PathBuf::from("/other"), "Other".into());

        pl.prune_subprojects();
        assert_eq!(pl.projects.len(), 2);
        let roots: Vec<&Path> = pl.projects.iter().map(|e| e.root.as_path()).collect();
        assert!(roots.contains(&Path::new("/workspace")));
        assert!(roots.contains(&Path::new("/other")));
        assert!(!roots.contains(&Path::new("/workspace/crates/core")));
    }

    #[test]
    fn project_list_prune_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut pl = ProjectList::default();
        pl.touch(tmp.path().to_path_buf(), "Exists".into());
        pl.touch(PathBuf::from("/nonexistent_mae_test_xyz_42"), "Gone".into());

        pl.prune_missing();
        assert_eq!(pl.projects.len(), 1);
        assert_eq!(pl.projects[0].root, tmp.path());
    }

    #[test]
    fn project_list_save_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();

        let mut pl = ProjectList::default();
        pl.touch(PathBuf::from("/proj/alpha"), "Alpha".into());
        pl.touch(PathBuf::from("/proj/beta"), "Beta".into());
        pl.save(data_dir).unwrap();

        let loaded = ProjectList::load(data_dir);
        assert_eq!(loaded.projects.len(), 2);
        assert_eq!(loaded.projects[0].name, "Alpha");
        assert_eq!(loaded.projects[1].name, "Beta");
    }
}
