//! File tree sidebar — project-level directory browser with icons.

use std::collections::HashSet;
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

/// A single entry in the file tree.
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
}

/// Persistent file tree state for the sidebar.
#[derive(Debug, Clone)]
pub struct FileTree {
    pub root: PathBuf,
    pub entries: Vec<FileTreeEntry>,
    pub expanded_dirs: HashSet<PathBuf>,
    pub selected: usize,
    pub scroll_offset: usize,
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
        };
        tree.expanded_dirs.insert(root.to_path_buf());
        tree.refresh();
        tree
    }

    /// Re-scan the filesystem and rebuild the flat entry list.
    pub fn refresh(&mut self) {
        self.entries.clear();
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
            self.entries.push(FileTreeEntry {
                path: path.clone(),
                name,
                is_dir,
                depth,
            });
            if expanded {
                self.scan_dir(&path, depth + 1);
            }
        }
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
}
