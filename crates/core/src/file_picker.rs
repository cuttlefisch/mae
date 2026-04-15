use std::path::{Path, PathBuf};

/// State for the fuzzy file picker overlay.
pub struct FilePicker {
    /// The user's query string.
    pub query: String,
    /// All candidate file paths (relative to root).
    pub candidates: Vec<String>,
    /// Indices into `candidates` that match the current query, ranked by score.
    pub filtered: Vec<usize>,
    /// Currently selected index within `filtered`.
    pub selected: usize,
    /// Root directory we scanned from.
    pub root: PathBuf,
}

/// Directories to skip during recursive scan.
const SKIP_DIRS: &[&str] = &[
    ".git", "target", "node_modules", ".cache", ".next", "__pycache__",
    ".mypy_cache", ".pytest_cache", "dist", "build", ".eggs", ".tox",
    "vendor", ".bundle", "zig-cache", "zig-out",
];

/// Max recursion depth for directory walking.
const MAX_DEPTH: usize = 12;

/// Max number of candidates to collect.
const MAX_CANDIDATES: usize = 50_000;

impl FilePicker {
    /// Scan a directory tree and create a new file picker.
    pub fn scan(root: &Path) -> Self {
        let mut candidates = Vec::new();
        walk_dir(root, root, 0, &mut candidates);
        candidates.sort();

        let filtered: Vec<usize> = (0..candidates.len()).collect();
        FilePicker {
            query: String::new(),
            candidates,
            filtered,
            selected: 0,
            root: root.to_path_buf(),
        }
    }

    /// Re-filter candidates based on current query.
    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.candidates.len()).collect();
        } else {
            let query_lower: Vec<char> = self.query.to_lowercase().chars().collect();
            let mut scored: Vec<(usize, i64)> = self
                .candidates
                .iter()
                .enumerate()
                .filter_map(|(idx, path)| {
                    score_match(path, &query_lower).map(|s| (idx, s))
                })
                .collect();
            // Higher score = better match, sort descending
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
        }
        self.selected = 0;
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Get the full path of the currently selected file, if any.
    pub fn selected_path(&self) -> Option<PathBuf> {
        if self.filtered.is_empty() {
            return None;
        }
        let idx = self.filtered[self.selected];
        Some(self.root.join(&self.candidates[idx]))
    }

    /// Get the currently selected relative path string, if any.
    pub fn selected_name(&self) -> Option<&str> {
        if self.filtered.is_empty() {
            return None;
        }
        let idx = self.filtered[self.selected];
        Some(&self.candidates[idx])
    }
}

/// Fuzzy subsequence scoring. Returns None if no match.
/// Higher score = better match.
fn score_match(path: &str, query: &[char]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let path_lower: Vec<char> = path.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score: i64 = 0;
    let mut last_match_pos: Option<usize> = None;
    let mut first_match_pos: Option<usize> = None;

    for (pi, &pc) in path_lower.iter().enumerate() {
        if qi < query.len() && pc == query[qi] {
            if first_match_pos.is_none() {
                first_match_pos = Some(pi);
            }

            // Bonus for consecutive matches
            if let Some(last) = last_match_pos {
                if pi == last + 1 {
                    score += 10;
                }
            }

            // Bonus for matching at word boundaries (after / or . or _ or -)
            if pi == 0 || matches!(path_lower.get(pi.saturating_sub(1)), Some('/' | '.' | '_' | '-')) {
                score += 8;
            }

            // Bonus for matching filename (after last /)
            let last_slash = path_lower.iter().rposition(|c| *c == '/').unwrap_or(0);
            if pi >= last_slash {
                score += 5;
            }

            last_match_pos = Some(pi);
            qi += 1;
        }
    }

    if qi < query.len() {
        return None; // Not all query chars matched
    }

    // Penalty for longer paths (prefer shorter matches)
    score -= path.len() as i64 / 4;

    // Bonus for prefix match of filename
    if let Some(fp) = first_match_pos {
        let last_slash = path_lower.iter().rposition(|c| *c == '/').map(|p| p + 1).unwrap_or(0);
        if fp == last_slash {
            score += 15; // Query starts matching at filename start
        }
    }

    Some(score)
}

/// Recursively walk a directory tree, collecting file paths.
fn walk_dir(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_CANDIDATES {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut dirs = Vec::new();

    for entry in entries.flatten() {
        if out.len() >= MAX_CANDIDATES {
            return;
        }

        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // Skip hidden files/dirs (starting with .)
        if name.starts_with('.') {
            continue;
        }

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                dirs.push(path);
            }
        } else if file_type.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }

    // Sort directories for deterministic output
    dirs.sort();
    for d in dirs {
        walk_dir(root, &d, depth + 1, out);
    }
}

/// Tab completion helper: list files/dirs matching a prefix path.
pub fn complete_path(input: &str) -> Vec<String> {
    let path = Path::new(input);

    let (dir, prefix) = if input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR) {
        (PathBuf::from(input), String::new())
    } else if let Some(parent) = path.parent() {
        let prefix = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        (if parent.as_os_str().is_empty() { PathBuf::from(".") } else { parent.to_path_buf() }, prefix)
    } else {
        (PathBuf::from("."), input.to_string())
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut matches: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) || prefix.is_empty() {
                let full = if dir == Path::new(".") && !input.starts_with("./") {
                    name.clone()
                } else {
                    format!("{}/{}", dir.display(), name)
                };
                // Append / for directories
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                if is_dir {
                    Some(format!("{}/", full))
                } else {
                    Some(full)
                }
            } else {
                None
            }
        })
        .collect();

    matches.sort();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_test_tree(root: &Path) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("src/utils")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();

        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("src/utils/helpers.rs"), "").unwrap();
        fs::write(root.join("docs/readme.md"), "").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join(".git/objects/abc"), "").unwrap();
        fs::write(root.join("target/debug/binary"), "").unwrap();
        fs::write(root.join("node_modules/foo/index.js"), "").unwrap();
    }

    #[test]
    fn scan_finds_files() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path());
        assert!(picker.candidates.contains(&"src/main.rs".to_string()));
        assert!(picker.candidates.contains(&"Cargo.toml".to_string()));
        assert!(picker.candidates.contains(&"docs/readme.md".to_string()));
    }

    #[test]
    fn scan_skips_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path());
        for c in &picker.candidates {
            assert!(!c.contains(".git"), "should skip .git: {}", c);
        }
    }

    #[test]
    fn scan_skips_target_and_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path());
        for c in &picker.candidates {
            assert!(!c.contains("target/"), "should skip target: {}", c);
            assert!(!c.contains("node_modules/"), "should skip node_modules: {}", c);
        }
    }

    #[test]
    fn scan_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b/c/d/e/f/g/h/i/j/k/l/m/n");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.txt"), "").unwrap();
        let picker = FilePicker::scan(tmp.path());
        // MAX_DEPTH is 12, so depth 14 file should not appear
        assert!(
            !picker.candidates.iter().any(|c| c.contains("deep.txt")),
            "should not find files beyond depth limit"
        );
    }

    #[test]
    fn filter_empty_query_returns_all() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path());
        assert_eq!(picker.filtered.len(), picker.candidates.len());
    }

    #[test]
    fn filter_subsequence_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path());
        picker.query = "mrs".to_string();
        picker.update_filter();
        // "main.rs" matches subsequence m-r-s (via src/main.rs)
        let names: Vec<&str> = picker.filtered.iter().map(|&i| picker.candidates[i].as_str()).collect();
        assert!(names.iter().any(|n| n.contains("main.rs")), "should match main.rs, got: {:?}", names);
    }

    #[test]
    fn filter_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path());
        picker.query = "zzzzzzz".to_string();
        picker.update_filter();
        assert!(picker.filtered.is_empty());
    }

    #[test]
    fn filter_ranking_prefers_filename_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "").unwrap();
        fs::write(tmp.path().join("src/remain.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path());
        picker.query = "main".to_string();
        picker.update_filter();
        let names: Vec<&str> = picker.filtered.iter().map(|&i| picker.candidates[i].as_str()).collect();
        assert!(!names.is_empty());
        assert!(names[0].contains("main.rs"), "main.rs should rank first, got: {:?}", names);
    }

    #[test]
    fn selected_wraps_around() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path());
        let count = picker.filtered.len();
        assert!(count > 0);
        // Wrap down
        for _ in 0..count {
            picker.move_down();
        }
        assert_eq!(picker.selected, 0);
        // Wrap up from 0
        picker.move_up();
        assert_eq!(picker.selected, count - 1);
    }

    #[test]
    fn selected_path_returns_full_path() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path());
        let path = picker.selected_path().unwrap();
        assert!(path.starts_with(tmp.path()));
        assert!(path.exists());
    }

    #[test]
    fn complete_path_finds_matches() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let matches = complete_path(&format!("{}/src/", tmp.path().display()));
        assert!(matches.iter().any(|m| m.contains("main.rs")));
        assert!(matches.iter().any(|m| m.contains("lib.rs")));
    }

    #[test]
    fn complete_path_with_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let matches = complete_path(&format!("{}/src/ma", tmp.path().display()));
        assert!(matches.iter().any(|m| m.contains("main.rs")));
        assert!(!matches.iter().any(|m| m.contains("lib.rs")));
    }
}
