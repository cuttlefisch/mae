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
    /// Display label for the root (uses `~` when under $HOME).
    pub root_label: String,
    /// When true, the query line itself is selected (Emacs-style: the literal
    /// text is the chosen value, not a candidate). Set by C-p/Up past the
    /// first candidate, or automatically when `filtered` is empty.
    pub query_selected: bool,
    /// Max recursion depth for directory walking.
    pub max_depth: usize,
    /// Max number of candidates to collect.
    pub max_candidates: usize,
}

/// Directories to skip during recursive scan.
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

/// Default max recursion depth for directory walking.
pub const DEFAULT_MAX_DEPTH: usize = 12;

/// Default max number of candidates to collect.
pub const DEFAULT_MAX_CANDIDATES: usize = 50_000;

impl FilePicker {
    /// Scan a directory tree and create a new file picker.
    pub fn scan(root: &Path, max_depth: usize, max_candidates: usize) -> Self {
        let mut candidates = Vec::new();
        walk_dir(root, root, 0, &mut candidates, max_depth, max_candidates);
        candidates.sort();

        let filtered: Vec<usize> = (0..candidates.len()).collect();
        let root_label = unexpand_tilde(&root.to_string_lossy());
        FilePicker {
            query: String::new(),
            candidates,
            filtered,
            selected: 0,
            root: root.to_path_buf(),
            root_label,
            query_selected: false,
            max_depth,
            max_candidates,
        }
    }

    /// Re-filter candidates based on current query.
    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.candidates.len()).collect();
        } else {
            // Directory-prefix filtering: if query contains '/' and the
            // prefix matches a directory path, restrict search to that subtree.
            let (dir_prefix, remainder) = split_directory_prefix(&self.query, &self.candidates);
            let query_lower: Vec<char> = remainder.to_lowercase().chars().collect();

            let mut scored: Vec<(usize, i64)> = self
                .candidates
                .iter()
                .enumerate()
                .filter(|(_, path)| {
                    if let Some(dp) = dir_prefix {
                        path.to_lowercase().starts_with(&dp.to_lowercase())
                    } else {
                        true
                    }
                })
                .filter_map(|(idx, path)| {
                    // Score only the portion after the directory prefix
                    let score_target = if let Some(dp) = dir_prefix {
                        &path[dp.len().min(path.len())..]
                    } else {
                        path.as_str()
                    };
                    if query_lower.is_empty() {
                        Some((idx, 0i64 - path.len() as i64))
                    } else {
                        score_match(score_target, &query_lower).map(|s| (idx, s))
                    }
                })
                .collect();
            // Higher score = better match, sort descending
            scored.sort_by_key(|b| std::cmp::Reverse(b.1));
            self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
        }
        self.selected = 0;
        // Auto-select the query line when there are no matches and a
        // non-empty query — this lets the user press Enter to open/create
        // the literal path they typed.
        self.query_selected = !self.query.is_empty() && self.filtered.is_empty();
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.query_selected {
            // Leave query-line selection and enter the candidate list.
            self.query_selected = false;
            self.selected = 0;
        } else if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    /// Move selection up (C-p / Up). When already at the first candidate,
    /// moves to the query line itself (Emacs minibuffer pattern).
    pub fn move_up(&mut self) {
        if self.query_selected {
            return; // already at query line
        }
        if self.filtered.is_empty() || self.selected == 0 {
            self.query_selected = true;
        } else {
            self.selected -= 1;
        }
    }

    /// Get the full path of the currently selected file.
    ///
    /// When `query_selected` is true (the user navigated to the query line
    /// via C-p/Up, or there are no matches), returns the query text as a
    /// literal path — enabling file creation for paths that don't exist yet.
    pub fn selected_path(&self) -> Option<PathBuf> {
        if self.query_selected && !self.query.is_empty() {
            let q = &self.query;
            // Absolute or home-relative paths are used as-is.
            if q.starts_with('/') {
                return Some(PathBuf::from(q));
            }
            if let Some(rest) = q.strip_prefix("~/") {
                if let Ok(home) = std::env::var("HOME") {
                    return Some(PathBuf::from(home).join(rest));
                }
            }
            return Some(self.root.join(q));
        }
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

    /// Doom-style Tab completion: expand the query to the longest
    /// common prefix shared by all currently filtered candidates.
    ///
    /// Returns `true` if the query was extended (i.e. the prefix was
    /// strictly longer than the current query), `false` otherwise. In
    /// the latter case the caller can fall back to a different action
    /// (e.g. descend into the selected directory in a future extension).
    ///
    /// This effectively narrows the picker to a single sub-tree without
    /// having to type every path component: typing `ed` then Tab jumps
    /// to `crates/core/src/editor/` when all matches live there.
    pub fn complete_longest_prefix(&mut self) -> bool {
        if self.filtered.is_empty() {
            return false;
        }
        // Compute the longest common byte-prefix across all filtered
        // candidates. Paths are UTF-8; we need to snap back to a char
        // boundary before using the prefix as a string.
        let first = &self.candidates[self.filtered[0]];
        let mut prefix_bytes = first.len();
        for &idx in &self.filtered[1..] {
            let other = &self.candidates[idx];
            prefix_bytes = common_prefix_bytes(first, other).min(prefix_bytes);
            if prefix_bytes == 0 {
                return false;
            }
        }
        // Snap to the last char boundary at or below prefix_bytes.
        while prefix_bytes > 0 && !first.is_char_boundary(prefix_bytes) {
            prefix_bytes -= 1;
        }
        let prefix = &first[..prefix_bytes];
        // Only commit if the prefix is strictly longer than what the
        // user already typed. Matching is case-insensitive, so we can't
        // just compare strings; instead check prefix_bytes > query.len().
        if prefix_bytes > self.query.len() {
            self.query = prefix.to_string();
            self.update_filter();
            true
        } else {
            false
        }
    }

    /// If the query is an absolute path ending in `/` that resolves to a
    /// directory, rescan from that directory. Emacs/Doom-style: typing
    /// `~/RoamNotes/` switches the picker root to that directory.
    ///
    /// Also handles relative paths: if `root.join(query)` is a directory,
    /// switch to it (e.g. after root-switching to `/`, typing `tmp/`
    /// descends into `/tmp/`).
    ///
    /// Returns true if the root was switched.
    pub fn maybe_switch_root(&mut self) -> bool {
        let expanded = expand_tilde(&self.query);
        if expanded.starts_with('/') {
            let path = Path::new(&expanded);
            // Don't rescan from filesystem root — it's too broad and slow.
            // Wait until the user has typed at least one directory component
            // (e.g. `/tmp/` not just `/`).
            if path != Path::new("/") && path.is_dir() {
                self.rescan(path);
                self.query.clear();
                return true;
            }
        }
        // Try as a relative path under the current root.
        // Skip if the query starts with '/' (already handled above) or is
        // just a bare '/'.
        if self.query.ends_with('/') && !self.query.starts_with('/') {
            let joined = self.root.join(&self.query);
            if joined.is_dir() {
                let canonical = joined.canonicalize().unwrap_or(joined);
                self.rescan(&canonical);
                self.query.clear();
                return true;
            }
        }
        false
    }

    /// Tab completion for absolute/home-relative paths. Uses filesystem
    /// listing to complete to the longest common prefix, then switches
    /// root if the result is a directory. Returns true if anything happened.
    pub fn complete_path_tab(&mut self) -> bool {
        let expanded = expand_tilde(&self.query);
        if !expanded.starts_with('/') {
            return false;
        }
        let completions = complete_path(&expanded);
        if completions.is_empty() {
            return false;
        }
        if completions.len() == 1 {
            let completed = unexpand_tilde(&completions[0]);
            if completed != self.query {
                self.query = completed;
                // If the completion is a directory, switch root immediately.
                self.maybe_switch_root();
                return true;
            }
            return false;
        }
        // Multiple completions: extend to longest common prefix.
        let first = &completions[0];
        let mut prefix_len = first.len();
        for c in &completions[1..] {
            prefix_len = common_prefix_bytes(first, c).min(prefix_len);
        }
        while prefix_len > 0 && !first.is_char_boundary(prefix_len) {
            prefix_len -= 1;
        }
        let prefix = &first[..prefix_len];
        if prefix.len() > expanded.len() {
            self.query = unexpand_tilde(prefix);
            self.maybe_switch_root();
            true
        } else {
            false
        }
    }

    /// Rescan from a new root directory.
    fn rescan(&mut self, new_root: &Path) {
        self.root = new_root.to_path_buf();
        self.root_label = unexpand_tilde(&new_root.to_string_lossy());
        self.candidates.clear();
        walk_dir(
            new_root,
            new_root,
            0,
            &mut self.candidates,
            self.max_depth,
            self.max_candidates,
        );
        self.candidates.sort();
        self.filtered = (0..self.candidates.len()).collect();
        self.selected = 0;
    }

    /// Clear the query (Ctrl-U style).
    pub fn clear_query(&mut self) {
        self.query.clear();
        self.update_filter();
    }
}

/// Expand `~` or `~/...` to the user's home directory.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    } else if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Replace the home directory prefix with `~` for display.
pub fn unexpand_tilde(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            if rest.is_empty() {
                return "~".to_string();
            }
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Split a query into an optional directory prefix and the remainder.
/// If the query contains '/' and the prefix up to the last '/' matches
/// candidates, returns (Some(dir_prefix), remainder). Otherwise returns
/// (None, full_query).
fn split_directory_prefix<'a>(query: &'a str, candidates: &[String]) -> (Option<&'a str>, &'a str) {
    if let Some(last_slash) = query.rfind('/') {
        let dir_prefix = &query[..=last_slash];
        let lower = dir_prefix.to_lowercase();
        if candidates
            .iter()
            .any(|c| c.to_lowercase().starts_with(&lower))
        {
            return (Some(dir_prefix), &query[last_slash + 1..]);
        }
    }
    (None, query)
}

/// Byte length of the longest common ASCII/UTF-8 byte prefix of two strings.
pub(crate) fn common_prefix_bytes(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Tiered match scoring. Returns `None` if the query fails to match at all,
/// otherwise a score where higher = better. Tiers (roughly):
///
/// 1. **Exact equality** — `path == query`. Top tier, always wins.
/// 2. **Boundary-aligned suffix** — query equals the tail of the path and
///    the preceding char is `/` (path segment boundary). This is the
///    "typing a relative path works" case: `editor/mod.rs` →
///    `crates/core/src/editor/mod.rs` is a #1 hit.
/// 3. **Plain suffix** — query equals the tail of the path without
///    boundary alignment (e.g. `ain.rs` → `main.rs`).
/// 4. **Contiguous substring** — query appears as a continuous substring.
///    Boundary-aligned hits (after `/._-` or start) beat mid-word hits.
/// 5. **Fuzzy subsequence** — the legacy scoring with word-boundary
///    bonuses. Still useful for commands like `sb` → `switch-buffer`.
///
/// Each higher tier is biased several orders of magnitude above the next
/// so tier collisions can't happen: a substring match always outranks
/// any fuzzy-subsequence hit. Within a tier we subtract a length
/// penalty so shorter matches win on ties.
///
/// Shared by the file picker and the command palette — the fuzzy fallback
/// still serves commands where the query is a short abbreviation.
pub fn score_match(path: &str, query: &[char]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let path_lower = path.to_lowercase();
    let query_str: String = query.iter().collect();
    let path_len = path.len() as i64;

    // ---- Tier 1: exact equality ----
    if path_lower == query_str {
        return Some(1_000_000);
    }

    // ---- Tier 1.5: query exactly matches the basename ----
    let basename_start = path_lower.rfind('/').map(|p| p + 1).unwrap_or(0);
    let basename = &path_lower[basename_start..];
    if basename == query_str {
        return Some(750_000 - path_len);
    }

    // ---- Tier 2/3: suffix match ----
    if path_lower.ends_with(&query_str) && path_lower.len() > query_str.len() {
        let rest_len = path_lower.len() - query_str.len();
        let boundary_aligned = path_lower.as_bytes()[rest_len - 1] == b'/';
        let base = if boundary_aligned { 500_000 } else { 100_000 };
        return Some(base - path_len);
    }

    // ---- Tier 4: contiguous substring ----
    if let Some(pos) = path_lower.find(&query_str) {
        let boundary_aligned = pos == 0
            || matches!(
                path_lower.as_bytes().get(pos - 1),
                Some(b'/' | b'.' | b'_' | b'-')
            );
        let base = if boundary_aligned { 50_000 } else { 10_000 };
        // Tie-breaker: matches earlier in the filename portion rank above
        // the same substring appearing deep inside a parent dir name.
        let last_slash = path_lower.rfind('/').map(|p| p + 1).unwrap_or(0);
        let filename_bonus = if pos >= last_slash { 1_000 } else { 0 };
        return Some(base + filename_bonus - path_len);
    }

    // ---- Tier 5: fuzzy subsequence (legacy) ----
    // Skip fuzzy matching when the query contains '/' — it's clearly a
    // path, and scattered subsequence matches (t-m-p-b-u-t-s across a
    // 120-char path) produce garbage. Only tiers 1-4 make sense for paths.
    if query_str.contains('/') {
        return None;
    }

    let path_chars: Vec<char> = path_lower.chars().collect();
    let mut qi = 0;
    let mut score: i64 = 0;
    let mut last_match_pos: Option<usize> = None;
    let mut first_match_pos: Option<usize> = None;

    for (pi, &pc) in path_chars.iter().enumerate() {
        if qi < query.len() && pc == query[qi] {
            if first_match_pos.is_none() {
                first_match_pos = Some(pi);
            }
            if let Some(last) = last_match_pos {
                if pi == last + 1 {
                    score += 10;
                }
            }
            if pi == 0
                || matches!(
                    path_chars.get(pi.saturating_sub(1)),
                    Some('/' | '.' | '_' | '-')
                )
            {
                score += 8;
            }
            let last_slash = path_chars.iter().rposition(|c| *c == '/').unwrap_or(0);
            if pi >= last_slash {
                score += 5;
            }
            last_match_pos = Some(pi);
            qi += 1;
        }
    }

    if qi < query.len() {
        return None;
    }

    score -= path_len / 4;

    if let Some(fp) = first_match_pos {
        let last_slash = path_chars
            .iter()
            .rposition(|c| *c == '/')
            .map(|p| p + 1)
            .unwrap_or(0);
        if fp == last_slash {
            score += 15;
        }
    }

    Some(score)
}

/// Recursively walk a directory tree, collecting file paths.
fn walk_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<String>,
    max_depth: usize,
    max_candidates: usize,
) {
    if depth > max_depth || out.len() >= max_candidates {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut dirs = Vec::new();

    for entry in entries.flatten() {
        if out.len() >= max_candidates {
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
        walk_dir(root, &d, depth + 1, out, max_depth, max_candidates);
    }
}

/// Tab completion helper: list files/dirs matching a prefix path.
pub fn complete_path(input: &str) -> Vec<String> {
    let path = Path::new(input);

    let (dir, prefix) = if input.ends_with('/') || input.ends_with(std::path::MAIN_SEPARATOR) {
        (PathBuf::from(input), String::new())
    } else if let Some(parent) = path.parent() {
        let prefix = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        (
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            },
            prefix,
        )
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
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        assert!(picker.candidates.contains(&"src/main.rs".to_string()));
        assert!(picker.candidates.contains(&"Cargo.toml".to_string()));
        assert!(picker.candidates.contains(&"docs/readme.md".to_string()));
    }

    #[test]
    fn scan_skips_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        for c in &picker.candidates {
            assert!(!c.contains(".git"), "should skip .git: {}", c);
        }
    }

    #[test]
    fn scan_skips_target_and_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        for c in &picker.candidates {
            assert!(!c.contains("target/"), "should skip target: {}", c);
            assert!(
                !c.contains("node_modules/"),
                "should skip node_modules: {}",
                c
            );
        }
    }

    #[test]
    fn scan_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b/c/d/e/f/g/h/i/j/k/l/m/n");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.txt"), "").unwrap();
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
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
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        assert_eq!(picker.filtered.len(), picker.candidates.len());
    }

    #[test]
    fn filter_subsequence_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "mrs".to_string();
        picker.update_filter();
        // "main.rs" matches subsequence m-r-s (via src/main.rs)
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.candidates[i].as_str())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("main.rs")),
            "should match main.rs, got: {:?}",
            names
        );
    }

    #[test]
    fn filter_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
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
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "main".to_string();
        picker.update_filter();
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.candidates[i].as_str())
            .collect();
        assert!(!names.is_empty());
        assert!(
            names[0].contains("main.rs"),
            "main.rs should rank first, got: {:?}",
            names
        );
    }

    #[test]
    fn selected_wraps_around() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        let count = picker.filtered.len();
        assert!(count > 0);
        // Wrap down
        for _ in 0..count {
            picker.move_down();
        }
        assert_eq!(picker.selected, 0);
        // Up from 0 goes to query line (Emacs minibuffer pattern)
        picker.move_up();
        assert!(picker.query_selected);
        // Down from query line returns to first candidate
        picker.move_down();
        assert!(!picker.query_selected);
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn selected_path_returns_full_path() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_tree(tmp.path());
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
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

    fn q(s: &str) -> Vec<char> {
        s.to_lowercase().chars().collect()
    }

    #[test]
    fn score_tier1_exact_match_wins() {
        let exact = score_match("src/main.rs", &q("src/main.rs")).unwrap();
        let suffix = score_match("crates/core/src/main.rs", &q("src/main.rs")).unwrap();
        assert!(exact > suffix, "exact should outrank suffix");
        assert_eq!(exact, 1_000_000);
    }

    #[test]
    fn score_tier2_boundary_suffix_beats_inner_substring() {
        // Typing a relative path should make that file the top hit.
        let query = q("editor/mod.rs");
        let target = score_match("crates/core/src/editor/mod.rs", &query).unwrap();
        let other = score_match("crates/core/src/editor/dispatch.rs", &query);
        // The other doesn't even match (no "editor/mod.rs" substring).
        assert!(other.is_none());
        // Confirm boundary-suffix tier base (500_000 - path_len).
        assert!(
            target > 400_000,
            "boundary suffix should be tier 2: {}",
            target
        );
    }

    #[test]
    fn score_tier2_outranks_tier4_substring() {
        // Path A contains "main.rs" as a suffix after a /.  Path B contains
        // it mid-word ("remain.rs"). A must win by a landslide.
        let a = score_match("src/main.rs", &q("main.rs")).unwrap();
        let b = score_match("src/remain.rs", &q("main.rs")).unwrap();
        assert!(a > b, "boundary-suffix {} should beat substring {}", a, b);
    }

    #[test]
    fn score_tier3_plain_suffix() {
        // "ain.rs" ends the path but the preceding char is 'm', not '/'.
        let s = score_match("src/main.rs", &q("ain.rs")).unwrap();
        assert!((90_000..500_000).contains(&s), "plain suffix tier: {}", s);
    }

    #[test]
    fn score_tier4_substring_boundary_beats_midword() {
        let boundary = score_match("src/main.rs", &q("main")).unwrap();
        let midword = score_match("src/remainder.rs", &q("main")).unwrap();
        assert!(boundary > midword);
    }

    #[test]
    fn score_tier5_fuzzy_still_works_for_abbreviations() {
        // Commands need "sb" -> "switch-buffer" to still match via tier 5.
        let hit = score_match("switch-buffer", &q("sb"));
        assert!(hit.is_some());
        let miss = score_match("switch-buffer", &q("xyz"));
        assert!(miss.is_none());
    }

    #[test]
    fn tab_completes_to_longest_common_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("crates/core/src/editor")).unwrap();
        fs::write(tmp.path().join("crates/core/src/editor/mod.rs"), "").unwrap();
        fs::write(tmp.path().join("crates/core/src/editor/dispatch.rs"), "").unwrap();
        fs::write(tmp.path().join("crates/core/src/editor/macros.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "editor/".to_string();
        picker.update_filter();
        let expanded = picker.complete_longest_prefix();
        assert!(expanded, "should extend query when prefix is shared");
        assert_eq!(picker.query, "crates/core/src/editor/");
    }

    #[test]
    fn tab_completion_returns_false_when_no_shared_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a")).unwrap();
        fs::create_dir_all(tmp.path().join("b")).unwrap();
        fs::write(tmp.path().join("a/foo.rs"), "").unwrap();
        fs::write(tmp.path().join("b/foo.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        // Fuzzy match both on "f" — common prefix is "", so no extension.
        picker.query = "f".to_string();
        picker.update_filter();
        assert!(!picker.complete_longest_prefix());
    }

    #[test]
    fn tab_completion_is_idempotent_on_single_match() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("single.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "si".to_string();
        picker.update_filter();
        assert!(picker.complete_longest_prefix(), "single match extends");
        assert_eq!(picker.query, "single.rs");
        // Second press: nothing left to complete.
        assert!(!picker.complete_longest_prefix());
    }

    #[test]
    fn filter_typing_path_promotes_exact_file() {
        // Regression: "too fuzzy" search. Typing `editor/mod.rs` should
        // place `crates/core/src/editor/mod.rs` at position 0.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("crates/core/src/editor")).unwrap();
        fs::create_dir_all(tmp.path().join("crates/mae/src")).unwrap();
        fs::write(tmp.path().join("crates/core/src/editor/mod.rs"), "").unwrap();
        fs::write(tmp.path().join("crates/core/src/editor/dispatch.rs"), "").unwrap();
        fs::write(tmp.path().join("crates/mae/src/main.rs"), "").unwrap();
        // Decoys that share many letters via fuzzy subsequence but aren't
        // the path the user typed.
        fs::write(tmp.path().join("crates/core/src/commands.rs"), "").unwrap();

        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "editor/mod.rs".to_string();
        picker.update_filter();
        let top = picker.candidates[picker.filtered[0]].as_str();
        assert_eq!(top, "crates/core/src/editor/mod.rs");
    }

    #[test]
    fn switch_root_to_absolute_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("inner.txt"), "").unwrap();
        fs::write(tmp.path().join("outer.txt"), "").unwrap();

        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        assert!(picker.candidates.iter().any(|c| c == "outer.txt"));
        assert!(picker.candidates.iter().any(|c| c == "subdir/inner.txt"));

        // Simulate typing an absolute path to subdir
        picker.query = format!("{}/", sub.display());
        assert!(picker.maybe_switch_root());
        assert_eq!(picker.root, sub);
        assert_eq!(picker.query, "");
        assert!(picker.candidates.iter().any(|c| c == "inner.txt"));
        assert!(!picker.candidates.iter().any(|c| c.contains("outer")));
    }

    #[test]
    fn switch_root_ignores_non_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("file.txt"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = format!("{}", tmp.path().join("file.txt").display());
        assert!(!picker.maybe_switch_root());
    }

    #[test]
    fn clear_query_resets_filter() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.rs"), "").unwrap();
        fs::write(tmp.path().join("b.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "zzz".to_string();
        picker.update_filter();
        assert!(picker.filtered.is_empty());
        picker.clear_query();
        assert_eq!(picker.filtered.len(), 2);
    }

    #[test]
    fn tilde_expansion() {
        let expanded = expand_tilde("~/foo/bar");
        assert!(!expanded.starts_with('~'));
        assert!(expanded.ends_with("/foo/bar"));

        let round_trip = unexpand_tilde(&expanded);
        assert_eq!(round_trip, "~/foo/bar");
    }

    #[test]
    fn root_label_uses_tilde() {
        if let Some(home) = std::env::var_os("HOME") {
            let home_path = Path::new(&home);
            let picker = FilePicker::scan(home_path, DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
            assert_eq!(picker.root_label, "~");
        }
    }

    // ---- WU3: Directory prefix filtering + basename scoring ----

    #[test]
    fn dir_prefix_filters_to_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src/editor")).unwrap();
        fs::create_dir_all(tmp.path().join("docs")).unwrap();
        fs::write(tmp.path().join("src/editor/mod.rs"), "").unwrap();
        fs::write(tmp.path().join("src/editor/dispatch.rs"), "").unwrap();
        fs::write(tmp.path().join("docs/readme.md"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "src/editor/".to_string();
        picker.update_filter();
        // Should only show files under src/editor/
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.candidates[i].as_str())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| n.starts_with("src/editor/")));
    }

    #[test]
    fn dir_prefix_with_remainder_finds_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src/editor")).unwrap();
        fs::write(tmp.path().join("src/editor/mod.rs"), "").unwrap();
        fs::write(tmp.path().join("src/editor/dispatch.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "src/editor/mod".to_string();
        picker.update_filter();
        assert!(!picker.filtered.is_empty());
        assert_eq!(picker.candidates[picker.filtered[0]], "src/editor/mod.rs");
    }

    #[test]
    fn basename_exact_ranks_highest() {
        // "mod.rs" should rank editor/mod.rs above module_helper.rs
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("editor")).unwrap();
        fs::write(tmp.path().join("editor/mod.rs"), "").unwrap();
        fs::write(tmp.path().join("module_helper.rs"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "mod.rs".to_string();
        picker.update_filter();
        assert!(!picker.filtered.is_empty());
        assert_eq!(
            picker.candidates[picker.filtered[0]],
            "editor/mod.rs",
            "exact basename should win, got: {:?}",
            picker
                .filtered
                .iter()
                .map(|&i| &picker.candidates[i])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn switch_root_skips_filesystem_root() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        // Typing just "/" should NOT switch root to /
        picker.query = "/".to_string();
        assert!(!picker.maybe_switch_root());
        assert_eq!(picker.root, tmp.path());
    }

    #[test]
    fn switch_root_relative_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("mydir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("inner.txt"), "").unwrap();
        let mut picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        picker.query = "mydir/".to_string();
        assert!(picker.maybe_switch_root());
        assert!(picker.root.ends_with("mydir"));
        assert_eq!(picker.query, "");
        assert!(picker.candidates.iter().any(|c| c == "inner.txt"));
    }

    #[test]
    fn fuzzy_skip_for_path_queries() {
        // Query with '/' should not fuzzy-subsequence match
        let hit = score_match(
            "home/heimdall/Downloads/jupyter_widgets.html.j2",
            &q("tmp/butts"),
        );
        assert!(
            hit.is_none(),
            "path-like query should not fuzzy-match unrelated paths"
        );
    }

    #[test]
    fn empty_query_shows_all_no_regression() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.rs"), "").unwrap();
        fs::write(tmp.path().join("b.rs"), "").unwrap();
        let picker = FilePicker::scan(tmp.path(), DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATES);
        assert_eq!(picker.filtered.len(), 2);
    }
}
