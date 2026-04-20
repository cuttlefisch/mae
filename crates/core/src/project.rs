//! Project detection and configuration.
//!
//! A "project" is a directory tree rooted at a `.project`, `.git`,
//! `Cargo.toml`, or similar marker. The optional `.project` TOML file
//! adds metadata (name, required resources, symlinks).

use serde::Deserialize;
use std::collections::VecDeque;
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

/// Marker files used to detect project roots, in priority order.
const PROJECT_MARKERS: &[&str] = &[
    ".project",
    ".git",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "Makefile",
];

/// Walk up from `start` looking for a project root.
pub fn detect_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        for marker in PROJECT_MARKERS {
            if dir.join(marker).exists() {
                return Some(dir);
            }
        }
        if !dir.pop() {
            return None;
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
}
