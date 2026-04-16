//! Ranger/dired-style single-pane directory browser.
//!
//! The existing `FilePicker` is the Telescope/Helm experience: a fuzzy
//! search over every file in the workspace. This is complementary — a
//! traversal-based browser for the times you don't know what you're
//! looking for, or want to move a file around directory-by-directory.
//!
//! Design choices:
//!
//! - **Single-pane** to start. Ranger's three-pane preview is a big UX
//!   commitment; a single pane gets us most of the value with a tenth
//!   of the code. Cross-crate preview can come later.
//! - **Entries are scoped to the current working directory** — we list
//!   `cwd`, not the entire repo, so the selection stays spatially
//!   meaningful.
//! - **Inline query filters within-directory** (typed chars narrow the
//!   listing). Descending into a dir clears the query, matching ranger
//!   and dirvish conventions.
//! - **Hidden files** (names starting with `.`) are skipped by default;
//!   `SKIP_DIRS` from the fuzzy picker is reused so `.git`/`target`/etc.
//!   don't clutter the view.
//!
//! Key interactions are driven by the binary's key handler; this module
//! only owns state and the traversal primitives.

use std::path::{Path, PathBuf};

use crate::file_picker::score_match;

/// One directory entry. Symlinks and unknown file types are treated as files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEntry {
    pub name: String,
    pub is_dir: bool,
}

impl BrowserEntry {
    /// Display name with a trailing `/` for directories.
    pub fn display(&self) -> String {
        if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Directories whose contents we never list — these are the same
/// build-artifact / VCS dirs the fuzzy picker skips.
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
    ".eggs",
    ".tox",
    "vendor",
    ".bundle",
    "zig-cache",
    "zig-out",
];

/// State for the ranger-style directory browser overlay.
pub struct FileBrowser {
    /// Absolute path of the currently displayed directory.
    pub cwd: PathBuf,
    /// All entries in `cwd`, sorted (dirs first, then files, each alpha).
    pub entries: Vec<BrowserEntry>,
    /// Indices into `entries` that match the current query, in filter order.
    pub filtered: Vec<usize>,
    /// Currently selected index within `filtered`.
    pub selected: usize,
    /// Inline filter query.
    pub query: String,
}

impl FileBrowser {
    /// Open a browser rooted at `path`. If `path` is a file, its parent
    /// directory is used. If neither exists, falls back to the process's
    /// current working directory, and ultimately to `/`.
    pub fn open(path: &Path) -> Self {
        let dir = resolve_initial_dir(path);
        let mut browser = FileBrowser {
            cwd: dir,
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            query: String::new(),
        };
        browser.refresh();
        browser
    }

    /// Re-read `cwd` into `entries`, reset the query and selection.
    pub fn refresh(&mut self) {
        self.entries = read_dir_entries(&self.cwd);
        self.query.clear();
        self.update_filter();
    }

    /// Apply the current query to produce `filtered`.
    ///
    /// Empty query → all entries in source order. Non-empty query →
    /// fuzzy-score each entry's display name; higher is better.
    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            let q_chars: Vec<char> = self.query.to_lowercase().chars().collect();
            let mut scored: Vec<(usize, i64)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, e)| score_match(&e.name, &q_chars).map(|s| (idx, s)))
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.1));
            self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
        }
        self.selected = 0;
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// The entry currently highlighted, if any.
    pub fn selected_entry(&self) -> Option<&BrowserEntry> {
        let idx = *self.filtered.get(self.selected)?;
        self.entries.get(idx)
    }

    /// Absolute path of the selected entry.
    pub fn selected_path(&self) -> Option<PathBuf> {
        Some(self.cwd.join(&self.selected_entry()?.name))
    }

    /// Enter the selected entry: descend into dirs, return file paths.
    ///
    /// Descending resets the query so typing narrows the new directory
    /// rather than carrying leftover input forward.
    pub fn activate(&mut self) -> Activation {
        let path = match self.selected_path() {
            Some(p) => p,
            None => return Activation::Nothing,
        };
        let is_dir = self.selected_entry().map(|e| e.is_dir).unwrap_or(false);
        if is_dir {
            self.cwd = path;
            self.refresh();
            Activation::Descended
        } else {
            Activation::OpenFile(path)
        }
    }

    /// Move to the parent directory.
    ///
    /// No-op at the filesystem root. After ascending, selection is placed
    /// on the child we came from, so `h l` round-trips to the same entry.
    pub fn ascend(&mut self) {
        let child_name = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let parent = match self.cwd.parent() {
            Some(p) if p != self.cwd => p.to_path_buf(),
            _ => return,
        };
        self.cwd = parent;
        self.refresh();
        // Try to re-select the directory we just came from.
        if let Some(name) = child_name {
            if let Some(pos) = self
                .filtered
                .iter()
                .position(|&i| self.entries[i].name == name)
            {
                self.selected = pos;
            }
        }
    }
}

/// Resolve the starting directory: if `path` is a dir, use it; if a file,
/// use its parent; otherwise fall back to `.` and `/`.
fn resolve_initial_dir(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    if path.is_file() {
        if let Some(parent) = path.parent() {
            if parent.is_dir() {
                return parent.to_path_buf();
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Read entries of `dir`, skipping hidden and build-artifact dirs.
///
/// Sort: directories first (alpha), then files (alpha). This is the
/// convention ranger / dired / most file managers follow, and it keeps
/// traversal targets at the top where `j` lands first.
fn read_dir_entries(dir: &Path) -> Vec<BrowserEntry> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();

        // Skip dotfiles and known build artifacts.
        if name.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            dirs.push(BrowserEntry { name, is_dir: true });
        } else {
            // Treat symlinks and regular files the same — follow on enter.
            files.push(BrowserEntry {
                name,
                is_dir: false,
            });
        }
    }

    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    files.sort_by(|a, b| a.name.cmp(&b.name));
    dirs.append(&mut files);
    dirs
}

/// Outcome of attempting to "activate" (Enter / `l`) the selected entry.
#[derive(Debug)]
pub enum Activation {
    /// Descended into a subdirectory; listing was refreshed.
    Descended,
    /// Activated a file — caller should open this path.
    OpenFile(PathBuf),
    /// No selection, or the listing is empty.
    Nothing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tree(root: &Path) {
        fs::create_dir_all(root.join("src/editor")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("src/editor/mod.rs"), "").unwrap();
    }

    #[test]
    fn open_lists_directory_entries() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let browser = FileBrowser::open(tmp.path());
        let names: Vec<&str> = browser.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"docs"));
        assert!(names.contains(&"Cargo.toml"));
        assert!(names.contains(&"README.md"));
        // .git and target are hidden / skipped
        assert!(!names.contains(&".git"));
        assert!(!names.contains(&"target"));
    }

    #[test]
    fn dirs_sort_before_files() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let browser = FileBrowser::open(tmp.path());
        // Find first file (non-dir); every preceding entry must be a dir.
        let first_file = browser.entries.iter().position(|e| !e.is_dir).unwrap();
        for e in &browser.entries[..first_file] {
            assert!(
                e.is_dir,
                "{} should be a dir (appears before first file)",
                e.name
            );
        }
        // And no dir appears after the first file.
        for e in &browser.entries[first_file..] {
            assert!(!e.is_dir, "{} should be a file", e.name);
        }
    }

    #[test]
    fn descend_into_directory() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(tmp.path());
        // src should be the first entry (dirs sort first, alpha)
        let idx = browser
            .entries
            .iter()
            .position(|e| e.name == "src")
            .unwrap();
        browser.selected = browser.filtered.iter().position(|&i| i == idx).unwrap();
        match browser.activate() {
            Activation::Descended => {}
            _ => panic!("expected descent into src"),
        }
        assert_eq!(browser.cwd, tmp.path().join("src"));
        let names: Vec<&str> = browser.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"main.rs"));
        assert!(names.contains(&"editor"));
    }

    #[test]
    fn activate_file_returns_path() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(tmp.path());
        let idx = browser
            .entries
            .iter()
            .position(|e| e.name == "Cargo.toml")
            .unwrap();
        browser.selected = browser.filtered.iter().position(|&i| i == idx).unwrap();
        match browser.activate() {
            Activation::OpenFile(p) => assert_eq!(p, tmp.path().join("Cargo.toml")),
            _ => panic!("expected OpenFile"),
        }
        // cwd must not have changed.
        assert_eq!(browser.cwd, tmp.path());
    }

    #[test]
    fn ascend_returns_to_parent_and_selects_child() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(&tmp.path().join("src"));
        assert_eq!(browser.cwd, tmp.path().join("src"));
        browser.ascend();
        assert_eq!(browser.cwd, tmp.path());
        // Selected entry should be "src" since that's the child we came from.
        let sel = browser.selected_entry().unwrap();
        assert_eq!(sel.name, "src");
        assert!(sel.is_dir);
    }

    #[test]
    fn ascend_at_root_is_noop() {
        // `/` has no parent (or parent == self). Should not panic.
        let mut browser = FileBrowser::open(Path::new("/"));
        let before = browser.cwd.clone();
        browser.ascend();
        assert_eq!(browser.cwd, before);
    }

    #[test]
    fn descend_resets_query() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(tmp.path());
        browser.query = "src".to_string();
        browser.update_filter();
        let idx = browser
            .entries
            .iter()
            .position(|e| e.name == "src")
            .unwrap();
        browser.selected = browser.filtered.iter().position(|&i| i == idx).unwrap();
        browser.activate();
        assert_eq!(
            browser.query, "",
            "descending should reset the filter query"
        );
    }

    #[test]
    fn move_wraps_around() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(tmp.path());
        let n = browser.filtered.len();
        assert!(n >= 2);
        browser.selected = 0;
        browser.move_up();
        assert_eq!(browser.selected, n - 1);
        browser.move_down();
        assert_eq!(browser.selected, 0);
    }

    #[test]
    fn filter_narrows_entries() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let mut browser = FileBrowser::open(tmp.path());
        let total = browser.entries.len();
        browser.query = "cargo".to_string();
        browser.update_filter();
        assert!(browser.filtered.len() < total);
        let top = &browser.entries[browser.filtered[0]].name;
        assert_eq!(top, "Cargo.toml");
    }

    #[test]
    fn open_with_file_path_uses_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        make_tree(tmp.path());
        let browser = FileBrowser::open(&tmp.path().join("src/main.rs"));
        assert_eq!(browser.cwd, tmp.path().join("src"));
    }

    #[test]
    fn entry_display_appends_slash_for_dirs() {
        let file = BrowserEntry {
            name: "foo.rs".to_string(),
            is_dir: false,
        };
        let dir = BrowserEntry {
            name: "src".to_string(),
            is_dir: true,
        };
        assert_eq!(file.display(), "foo.rs");
        assert_eq!(dir.display(), "src/");
    }
}
