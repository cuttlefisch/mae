//! File tree sidebar — project-level directory browser with icons.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Directories to skip (reuses file_browser logic).
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".cache",
    ".next",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
    "build",
    ".tox",
    ".eggs",
    ".venv",
    "venv",
    ".direnv",
];

/// Pending file tree action for rename/create confirmation via the command line.
#[derive(Debug, Clone)]
pub enum FileTreeAction {
    /// Rename the file at the given path. Command-line is pre-filled with the current name.
    Rename(PathBuf),
    /// Create a new file or directory inside the given parent dir.
    /// If the user's input ends with `/`, a directory is created.
    Create(PathBuf),
}

/// Per-file git status for file tree display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileGitStatus {
    Modified,
    Staged,
    Added,
    Untracked,
    Conflicted,
    Renamed,
    Deleted,
    Clean,
}

impl FileGitStatus {
    /// Single-character marker for display.
    pub fn marker_char(self) -> char {
        match self {
            FileGitStatus::Modified => 'M',
            FileGitStatus::Staged => 'S',
            FileGitStatus::Added => 'A',
            FileGitStatus::Untracked => '?',
            FileGitStatus::Conflicted => 'C',
            FileGitStatus::Renamed => 'R',
            FileGitStatus::Deleted => 'D',
            FileGitStatus::Clean => ' ',
        }
    }

    /// Theme key for colorizing the file name.
    pub fn theme_key(self) -> &'static str {
        match self {
            FileGitStatus::Modified | FileGitStatus::Renamed => "diff.modified",
            FileGitStatus::Staged => "diff.added",
            FileGitStatus::Added | FileGitStatus::Untracked => "diff.added",
            FileGitStatus::Conflicted => "diff.removed",
            FileGitStatus::Deleted => "diff.removed",
            FileGitStatus::Clean => "ui.text",
        }
    }
}

/// A single entry in the file tree.
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub git_status: Option<FileGitStatus>,
}

/// Fold cycle state for NERDTree-style S-Tab cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldCycleState {
    Default,
    AllClosed,
    AllExpanded,
}

/// Persistent file tree state for the sidebar.
#[derive(Debug, Clone)]
pub struct FileTree {
    pub root: PathBuf,
    pub entries: Vec<FileTreeEntry>,
    pub expanded_dirs: HashSet<PathBuf>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub git_statuses: HashMap<PathBuf, FileGitStatus>,
    pub fold_cycle_state: FoldCycleState,
    pub default_expanded: HashSet<PathBuf>,
}

impl FileTree {
    /// Open (scan) a directory tree at `root`, expanding only the root level.
    pub fn open(root: &Path) -> Self {
        let mut tree = FileTree {
            root: root.to_path_buf(),
            entries: Vec::new(),
            expanded_dirs: HashSet::new(),
            scroll_offset: 0,
            selected: 0,
            git_statuses: HashMap::new(),
            fold_cycle_state: FoldCycleState::Default,
            default_expanded: HashSet::new(),
        };
        tree.expanded_dirs.insert(root.to_path_buf());
        tree.refresh();
        tree
    }

    /// Re-scan the filesystem and rebuild the flat entry list.
    pub fn refresh(&mut self) {
        self.entries.clear();
        self.refresh_git_status();
        self.scan_dir(&self.root.clone(), 0);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    fn scan_dir(&mut self, dir: &Path, depth: usize) {
        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();
        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') && depth == 0 && SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                let path = entry.path();
                let is_dir = path.is_dir();
                children.push((name, path, is_dir));
            }
        }
        // Sort: dirs first, then alphabetical (case-insensitive)
        children.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        for (name, path, is_dir) in children {
            let expanded = is_dir && self.expanded_dirs.contains(&path);
            let git_status = if is_dir {
                // Propagate: use the most notable child status
                self.dir_git_status(&path)
            } else {
                self.git_statuses.get(&path).copied()
            };
            self.entries.push(FileTreeEntry {
                path: path.clone(),
                name,
                is_dir,
                depth,
                git_status,
            });
            if expanded {
                self.scan_dir(&path, depth + 1);
            }
        }
    }

    /// Parse `git status --porcelain` output and populate git_statuses map.
    pub fn refresh_git_status(&mut self) {
        self.git_statuses.clear();
        let output = match std::process::Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&self.root)
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.len() < 4 {
                continue;
            }
            let x = line.as_bytes()[0];
            let y = line.as_bytes()[1];
            let path_str = &line[3..];
            // Handle renames: "R  old -> new"
            let path_str = if let Some(arrow) = path_str.find(" -> ") {
                &path_str[arrow + 4..]
            } else {
                path_str
            };
            let full_path = self.root.join(path_str);
            let status = match (x, y) {
                (b'?', b'?') => FileGitStatus::Untracked,
                (b'A', _) => FileGitStatus::Added,
                (b'R', _) => FileGitStatus::Renamed,
                (b'D', _) | (_, b'D') => FileGitStatus::Deleted,
                (b'U', _) | (_, b'U') => FileGitStatus::Conflicted,
                (b'M', _) | (b'T', _) => FileGitStatus::Staged,
                (_, b'M') | (_, b'T') => FileGitStatus::Modified,
                _ => continue,
            };
            self.git_statuses.insert(full_path, status);
        }
    }

    /// Compute git status for a directory by finding the most notable child status.
    fn dir_git_status(&self, dir: &Path) -> Option<FileGitStatus> {
        let mut result: Option<FileGitStatus> = None;
        for (path, status) in &self.git_statuses {
            if path.starts_with(dir) {
                let dominated = match (result, *status) {
                    (None, s) => s,
                    (Some(_), FileGitStatus::Conflicted) => FileGitStatus::Conflicted,
                    (Some(FileGitStatus::Conflicted), _) => FileGitStatus::Conflicted,
                    (Some(_), FileGitStatus::Modified) => FileGitStatus::Modified,
                    (Some(FileGitStatus::Modified), _) => FileGitStatus::Modified,
                    (Some(_), s) => s,
                };
                result = Some(dominated);
            }
        }
        result
    }

    /// Toggle expand/collapse of the selected directory.
    pub fn toggle_expand(&mut self) {
        if self.selected >= self.entries.len() {
            return;
        }
        let entry = &self.entries[self.selected];
        if !entry.is_dir {
            return;
        }
        let path = entry.path.clone();
        if self.expanded_dirs.contains(&path) {
            self.expanded_dirs.remove(&path);
        } else {
            self.expanded_dirs.insert(path);
        }
        self.refresh();
    }

    /// Return the path of the currently selected entry.
    pub fn selected_path(&self) -> Option<&Path> {
        self.entries.get(self.selected).map(|e| e.path.as_path())
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the first entry.
    pub fn move_to_first(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Move selection to the last entry.
    pub fn move_to_last(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Collapse the parent directory of the selected entry and move selection to it.
    pub fn close_parent(&mut self) {
        if self.selected >= self.entries.len() {
            return;
        }
        let entry = &self.entries[self.selected];
        let parent = entry.path.parent().map(|p| p.to_path_buf());
        if let Some(parent_path) = parent {
            if parent_path == self.root {
                return; // Already at root level
            }
            self.expanded_dirs.remove(&parent_path);
            self.refresh();
            // Move selection to the parent entry
            if let Some(idx) = self.entries.iter().position(|e| e.path == parent_path) {
                self.selected = idx;
            }
        }
    }

    /// Change the tree root to a new directory.
    pub fn change_root(&mut self, new_root: &Path) {
        self.root = new_root.to_path_buf();
        self.expanded_dirs.clear();
        self.expanded_dirs.insert(new_root.to_path_buf());
        self.selected = 0;
        self.scroll_offset = 0;
        self.refresh();
    }

    /// Move the tree root up to the parent directory.
    pub fn go_parent_root(&mut self) {
        if let Some(parent) = self.root.parent().map(|p| p.to_path_buf()) {
            self.change_root(&parent);
        }
    }

    /// Reveal a file path in the tree: expand all ancestor directories and select it.
    /// No-op if the path is not under this tree's root.
    pub fn reveal(&mut self, path: &Path) {
        let rel = match path.strip_prefix(&self.root) {
            Ok(r) => r,
            Err(_) => return,
        };
        // Expand each ancestor directory
        let mut current = self.root.clone();
        for component in rel.parent().into_iter().flat_map(|p| p.components()) {
            current = current.join(component);
            self.expanded_dirs.insert(current.clone());
        }
        self.refresh();
        // Select the entry matching the path
        if let Some(idx) = self.entries.iter().position(|e| e.path == path) {
            self.selected = idx;
        }
    }

    /// Ensure scroll_offset keeps selected visible within viewport_height.
    pub fn ensure_visible(&mut self, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        if self.selected >= self.scroll_offset + viewport_height {
            self.scroll_offset = self.selected - viewport_height + 1;
        }
    }

    /// Scroll viewport down by `count` lines.
    pub fn scroll_down(&mut self, count: usize, visible_height: usize) {
        let max_offset = self.entries.len().saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + count).min(max_offset);
        // Keep selected within visible range
        if self.selected < self.scroll_offset {
            self.selected = self.scroll_offset;
        }
    }

    /// Scroll viewport up by `count` lines.
    pub fn scroll_up(&mut self, count: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(count);
    }

    /// Scroll half page down.
    pub fn half_page_down(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        self.scroll_down(half, visible_height);
    }

    /// Scroll half page up.
    pub fn half_page_up(&mut self, visible_height: usize) {
        let half = visible_height / 2;
        self.scroll_up(half);
    }

    /// NERDTree-style S-Tab fold cycling: Default → AllClosed → AllExpanded → Default.
    pub fn global_cycle(&mut self) {
        match self.fold_cycle_state {
            FoldCycleState::Default => {
                // Save current state as default_expanded for restore
                self.default_expanded = self.expanded_dirs.clone();
                // Close all: keep only root
                self.expanded_dirs.clear();
                self.expanded_dirs.insert(self.root.clone());
                self.fold_cycle_state = FoldCycleState::AllClosed;
            }
            FoldCycleState::AllClosed => {
                // Expand all directories recursively
                self.expand_all_dirs(&self.root.clone());
                self.fold_cycle_state = FoldCycleState::AllExpanded;
            }
            FoldCycleState::AllExpanded => {
                // Restore default state
                self.expanded_dirs = self.default_expanded.clone();
                self.fold_cycle_state = FoldCycleState::Default;
            }
        }
        self.refresh();
    }

    /// Recursively expand all directories under `dir`.
    fn expand_all_dirs(&mut self, dir: &std::path::Path) {
        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !SKIP_DIRS.contains(&name.as_str()) {
                        self.expanded_dirs.insert(path.clone());
                        self.expand_all_dirs(&path);
                    }
                }
            }
        }
    }
}

/// Return a Unicode icon for a file path based on extension.
pub fn icon_for_path(path: &Path, is_dir: bool, is_expanded: bool) -> &'static str {
    if is_dir {
        return if is_expanded {
            "\u{1F4C2}"
        } else {
            "\u{1F4C1}"
        }; // 📂 📁
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => "\u{1F980}",                                    // 🦀
        "py" => "\u{1F40D}",                                    // 🐍
        "js" | "jsx" => "\u{26A1}",                             // ⚡
        "ts" | "tsx" => "\u{1F535}",                            // 🔵
        "toml" | "yaml" | "yml" | "json" => "\u{2699}\u{FE0F}", // ⚙️
        "md" | "org" | "txt" | "rst" => "\u{1F4DD}",            // 📝
        "sh" | "bash" | "zsh" => "\u{1F41A}",                   // 🐚
        "html" | "css" | "scss" => "\u{1F310}",                 // 🌐
        "lock" => "\u{1F512}",                                  // 🔒
        "scm" | "el" | "lisp" | "clj" => "\u{03BB}",            // λ
        _ => "\u{1F4C4}",                                       // 📄
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn open_scans_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("foo.rs"), "").unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();

        let tree = FileTree::open(dir.path());
        assert!(!tree.entries.is_empty());
        // root level has both entries
        let root_entries: Vec<_> = tree.entries.iter().filter(|e| e.depth == 0).collect();
        assert!(root_entries.len() >= 2);
    }

    #[test]
    fn toggle_expand_adds_children() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        // Find the "src" entry and select it
        let src_idx = tree.entries.iter().position(|e| e.name == "src").unwrap();
        tree.selected = src_idx;
        let len_before = tree.entries.len();
        tree.toggle_expand();
        assert!(tree.entries.len() > len_before); // children added
    }

    #[test]
    fn skip_dirs_filtered() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();

        let tree = FileTree::open(dir.path());
        assert!(!tree.entries.iter().any(|e| e.name == "node_modules"));
        assert!(tree.entries.iter().any(|e| e.name == "src"));
    }

    #[test]
    fn icon_for_extension() {
        assert_eq!(
            icon_for_path(Path::new("main.rs"), false, false),
            "\u{1F980}"
        );
        assert_eq!(
            icon_for_path(Path::new("script.py"), false, false),
            "\u{1F40D}"
        );
        assert_eq!(icon_for_path(Path::new("dir"), true, false), "\u{1F4C1}");
        assert_eq!(icon_for_path(Path::new("dir"), true, true), "\u{1F4C2}");
    }

    #[test]
    fn move_to_first_and_last() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();
        fs::write(dir.path().join("c.txt"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        assert!(tree.entries.len() >= 3);

        tree.move_to_last();
        assert_eq!(tree.selected, tree.entries.len() - 1);

        tree.move_to_first();
        assert_eq!(tree.selected, 0);
        assert_eq!(tree.scroll_offset, 0);
    }

    #[test]
    fn close_parent_collapses_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        // Expand src
        let src_idx = tree.entries.iter().position(|e| e.name == "src").unwrap();
        tree.selected = src_idx;
        tree.toggle_expand();
        // Select the child file
        let lib_idx = tree
            .entries
            .iter()
            .position(|e| e.name == "lib.rs")
            .unwrap();
        tree.selected = lib_idx;
        tree.close_parent();
        // Parent should be collapsed now, selection on "src"
        assert!(!tree.expanded_dirs.contains(&dir.path().join("src")));
        assert_eq!(tree.entries[tree.selected].name, "src");
    }

    #[test]
    fn change_root_resets_tree() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/file.txt"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        let sub = dir.path().join("sub");
        tree.change_root(&sub);
        assert_eq!(tree.root, sub);
        assert_eq!(tree.selected, 0);
        assert!(tree.entries.iter().any(|e| e.name == "file.txt"));
    }

    #[test]
    fn go_parent_root_moves_up() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("other.txt"), "").unwrap();

        let mut tree = FileTree::open(&dir.path().join("sub"));
        tree.go_parent_root();
        assert_eq!(tree.root, dir.path());
        assert!(tree.entries.iter().any(|e| e.name == "sub"));
    }

    #[test]
    fn reveal_expands_ancestors_and_selects() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/util")).unwrap();
        fs::write(dir.path().join("src/util/helpers.rs"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        // Initially only root level visible, src not expanded
        assert!(!tree.expanded_dirs.contains(&dir.path().join("src")));

        let target = dir.path().join("src/util/helpers.rs");
        tree.reveal(&target);

        // Both src and src/util should be expanded now
        assert!(tree.expanded_dirs.contains(&dir.path().join("src")));
        assert!(tree.expanded_dirs.contains(&dir.path().join("src/util")));
        // Selected entry should be helpers.rs
        assert_eq!(tree.entries[tree.selected].name, "helpers.rs");
    }

    #[test]
    fn reveal_outside_root_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        let before = tree.selected;
        tree.reveal(Path::new("/nonexistent/path.txt"));
        assert_eq!(tree.selected, before);
    }

    #[test]
    fn move_up_down() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("b.txt"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        assert!(tree.entries.len() >= 2);
        assert_eq!(tree.selected, 0);
        tree.move_down();
        assert_eq!(tree.selected, 1);
        tree.move_up();
        assert_eq!(tree.selected, 0);
        tree.move_up(); // at 0, stays at 0
        assert_eq!(tree.selected, 0);
    }

    #[test]
    fn git_status_marker_chars() {
        assert_eq!(FileGitStatus::Modified.marker_char(), 'M');
        assert_eq!(FileGitStatus::Staged.marker_char(), 'S');
        assert_eq!(FileGitStatus::Added.marker_char(), 'A');
        assert_eq!(FileGitStatus::Untracked.marker_char(), '?');
        assert_eq!(FileGitStatus::Conflicted.marker_char(), 'C');
        assert_eq!(FileGitStatus::Renamed.marker_char(), 'R');
        assert_eq!(FileGitStatus::Deleted.marker_char(), 'D');
        assert_eq!(FileGitStatus::Clean.marker_char(), ' ');
    }

    #[test]
    fn git_status_theme_keys() {
        assert_eq!(FileGitStatus::Modified.theme_key(), "diff.modified");
        assert_eq!(FileGitStatus::Staged.theme_key(), "diff.added");
        assert_eq!(FileGitStatus::Added.theme_key(), "diff.added");
        assert_eq!(FileGitStatus::Untracked.theme_key(), "diff.added");
        assert_eq!(FileGitStatus::Deleted.theme_key(), "diff.removed");
        assert_eq!(FileGitStatus::Conflicted.theme_key(), "diff.removed");
        assert_eq!(FileGitStatus::Clean.theme_key(), "ui.text");
    }

    #[test]
    fn refresh_git_status_in_repo() {
        let dir = tempfile::tempdir().unwrap();
        // git init
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Create and commit a file
        fs::write(dir.path().join("committed.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "committed.txt"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Modify the committed file
        fs::write(dir.path().join("committed.txt"), "modified").unwrap();
        // Create an untracked file
        fs::write(dir.path().join("untracked.txt"), "new").unwrap();

        let tree = FileTree::open(dir.path());
        // Check statuses
        let committed = tree
            .entries
            .iter()
            .find(|e| e.name == "committed.txt")
            .unwrap();
        assert_eq!(committed.git_status, Some(FileGitStatus::Modified));
        let untracked = tree
            .entries
            .iter()
            .find(|e| e.name == "untracked.txt")
            .unwrap();
        assert_eq!(untracked.git_status, Some(FileGitStatus::Untracked));
    }

    #[test]
    fn dir_propagation_git_status() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let tree = FileTree::open(dir.path());
        let src = tree.entries.iter().find(|e| e.name == "src").unwrap();
        // The directory should inherit the untracked status from its child
        assert!(
            src.git_status.is_some(),
            "directory should have propagated status"
        );
    }

    #[test]
    fn scroll_down_up() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }
        let mut tree = FileTree::open(dir.path());
        assert!(tree.entries.len() >= 20);
        assert_eq!(tree.scroll_offset, 0);
        tree.scroll_down(5, 10);
        assert_eq!(tree.scroll_offset, 5);
        tree.scroll_up(3);
        assert_eq!(tree.scroll_offset, 2);
        tree.scroll_up(100);
        assert_eq!(tree.scroll_offset, 0);
    }

    #[test]
    fn half_page_scroll() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }
        let mut tree = FileTree::open(dir.path());
        tree.half_page_down(10);
        assert_eq!(tree.scroll_offset, 5);
        tree.half_page_up(10);
        assert_eq!(tree.scroll_offset, 0);
    }

    #[test]
    fn global_cycle_three_states() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        fs::create_dir(dir.path().join("tests")).unwrap();
        fs::write(dir.path().join("tests/test.rs"), "").unwrap();

        let mut tree = FileTree::open(dir.path());
        // Expand both dirs
        let src_idx = tree.entries.iter().position(|e| e.name == "src").unwrap();
        tree.selected = src_idx;
        tree.toggle_expand();
        let tests_idx = tree.entries.iter().position(|e| e.name == "tests").unwrap();
        tree.selected = tests_idx;
        tree.toggle_expand();

        // Save initial state
        let initial_expanded = tree.expanded_dirs.clone();
        assert_eq!(tree.fold_cycle_state, FoldCycleState::Default);

        // Cycle 1: Default -> AllClosed
        tree.global_cycle();
        assert_eq!(tree.fold_cycle_state, FoldCycleState::AllClosed);
        // Only root should be expanded
        assert_eq!(tree.expanded_dirs.len(), 1);
        assert!(tree.expanded_dirs.contains(&tree.root));

        // Cycle 2: AllClosed -> AllExpanded
        tree.global_cycle();
        assert_eq!(tree.fold_cycle_state, FoldCycleState::AllExpanded);
        // More dirs should be expanded than just root
        assert!(tree.expanded_dirs.len() > 1);

        // Cycle 3: AllExpanded -> Default (restores original state)
        tree.global_cycle();
        assert_eq!(tree.fold_cycle_state, FoldCycleState::Default);
        assert_eq!(tree.expanded_dirs, initial_expanded);
    }
}
