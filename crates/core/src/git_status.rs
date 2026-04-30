//! Git status buffer data model (Phase 6 M5).

use std::collections::HashMap;
use std::path::PathBuf;

/// A section in the git status buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSection {
    Untracked,
    Unstaged,
    Staged,
    Stashes,
}

/// Semantic line type for rendering and cursor dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum GitLineKind {
    /// "Head: main" or "Merge: feature/x"
    Header,
    /// Section heading: "Untracked files:", "Unstaged changes:", etc.
    SectionHeader(GitSection),
    /// A file entry within a section.
    File {
        section: GitSection,
        status_char: char,
    },
    /// A diff hunk header (`@@ ... @@`).
    DiffHunk,
    /// A diff context/added/removed line.
    DiffLine(DiffLineType),
    /// Blank separator.
    Blank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineType {
    Context,
    Added,
    Removed,
}

/// A line in the git status buffer mapped to an action.
#[derive(Debug, Clone)]
pub struct GitStatusLine {
    pub text: String,
    pub section: Option<GitSection>,
    pub file_path: Option<String>,
    /// If Some, this line represents a specific hunk (start, count)
    pub hunk: Option<(usize, usize)>,
    /// Which hunk within the file (0-based). Set on DiffHunk and DiffLine rows.
    pub hunk_index: Option<usize>,
    /// Raw `@@ ... @@` header for this hunk. Set on DiffHunk rows.
    pub hunk_header: Option<String>,
    pub is_header: bool,
    pub is_collapsed: bool,
    /// Semantic kind for rendering.
    pub kind: GitLineKind,
}

/// Structured state for the `*Git Status*` buffer.
///
/// Collapse keys use structured prefixes:
/// - `"section:Unstaged"` — section-level
/// - `"file:src/main.rs:Unstaged"` — file-level
/// - `"hunk:src/main.rs:Unstaged:0"` — hunk-level
#[derive(Debug, Clone)]
pub struct GitStatusView {
    pub lines: Vec<GitStatusLine>,
    /// Parallel to `lines` — maps each buffer line to its semantic kind.
    pub line_kinds: Vec<GitLineKind>,
    /// Multi-level collapse state. Keys use structured prefixes (see above).
    pub collapsed: HashMap<String, bool>,
    /// Root directory of the repository.
    pub repo_root: PathBuf,
}

impl GitStatusView {
    pub fn new(repo_root: PathBuf) -> Self {
        GitStatusView {
            lines: Vec::new(),
            line_kinds: Vec::new(),
            collapsed: HashMap::new(),
            repo_root,
        }
    }

    /// Toggle collapse state for a key. Default state is "not collapsed" (expanded).
    pub fn toggle(&mut self, key: &str) {
        let collapsed = self.collapsed.entry(key.to_string()).or_insert(false);
        *collapsed = !*collapsed;
    }

    /// Check if a key is collapsed.
    pub fn is_collapsed(&self, key: &str) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }

    /// Build the collapse key for a given line.
    pub fn collapse_key_for_line(line: &GitStatusLine) -> Option<String> {
        match &line.kind {
            GitLineKind::SectionHeader(section) => {
                Some(format!("section:{}", section_name(section)))
            }
            GitLineKind::File { section, .. } => line
                .file_path
                .as_ref()
                .map(|p| format!("file:{}:{}", p, section_name(section))),
            GitLineKind::DiffHunk => {
                if let (Some(path), Some(section), Some(idx)) =
                    (&line.file_path, &line.section, line.hunk_index)
                {
                    Some(format!("hunk:{}:{}:{}", path, section_name(section), idx))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    // Legacy compatibility: check if a file's diff is expanded.
    // Files default to collapsed (diffs hidden). Expanded = collapsed entry is `false`.
    pub fn is_file_expanded(&self, path: &str, section: &GitSection) -> bool {
        let key = format!("file:{}:{}", path, section_name(section));
        self.collapsed.get(&key).copied() == Some(false)
    }

    /// Toggle expansion/collapse of a file's inline diff (legacy helper).
    /// Default is `true` (collapsed); first toggle flips to `false` (expanded).
    pub fn toggle_file_expansion(&mut self, path: &str, section: &GitSection) {
        let key = format!("file:{}:{}", path, section_name(section));
        let collapsed = self.collapsed.entry(key).or_insert(true);
        *collapsed = !*collapsed;
    }
}

/// Convert a `GitSection` to a stable string for collapse keys.
pub fn section_name(section: &GitSection) -> &'static str {
    match section {
        GitSection::Untracked => "Untracked",
        GitSection::Unstaged => "Unstaged",
        GitSection::Staged => "Staged",
        GitSection::Stashes => "Stashes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn git_status_expand_collapse() {
        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
        let section = GitSection::Unstaged;
        // Initially not expanded (file diffs default to collapsed)
        assert!(!view.is_file_expanded("src/main.rs", &section));
        // Toggle to expand
        view.toggle_file_expansion("src/main.rs", &section);
        assert!(view.is_file_expanded("src/main.rs", &section));
        // Toggle to collapse
        view.toggle_file_expansion("src/main.rs", &section);
        assert!(!view.is_file_expanded("src/main.rs", &section));
    }

    #[test]
    fn multi_level_collapse_keys() {
        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
        // Section collapse
        assert!(!view.is_collapsed("section:Unstaged"));
        view.toggle("section:Unstaged");
        assert!(view.is_collapsed("section:Unstaged"));

        // File collapse
        assert!(!view.is_collapsed("file:src/main.rs:Unstaged"));
        view.toggle("file:src/main.rs:Unstaged");
        assert!(view.is_collapsed("file:src/main.rs:Unstaged"));

        // Hunk collapse
        assert!(!view.is_collapsed("hunk:src/main.rs:Unstaged:0"));
        view.toggle("hunk:src/main.rs:Unstaged:0");
        assert!(view.is_collapsed("hunk:src/main.rs:Unstaged:0"));
    }

    #[test]
    fn collapse_key_for_line_variants() {
        let section_line = GitStatusLine {
            text: "Unstaged changes:".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: None,
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::SectionHeader(GitSection::Unstaged),
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&section_line),
            Some("section:Unstaged".to_string())
        );

        let file_line = GitStatusLine {
            text: "  M src/main.rs".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: false,
            is_collapsed: false,
            kind: GitLineKind::File {
                section: GitSection::Unstaged,
                status_char: 'M',
            },
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&file_line),
            Some("file:src/main.rs:Unstaged".to_string())
        );

        let hunk_line = GitStatusLine {
            text: "    @@ -1,3 +1,4 @@".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk: None,
            hunk_index: Some(0),
            hunk_header: Some("@@ -1,3 +1,4 @@".to_string()),
            is_header: false,
            is_collapsed: false,
            kind: GitLineKind::DiffHunk,
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&hunk_line),
            Some("hunk:src/main.rs:Unstaged:0".to_string())
        );
    }

    #[test]
    fn git_status_with_sections() {
        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
        // Simulate populated status with section headers
        view.lines.push(GitStatusLine {
            text: "Head:     main".to_string(),
            section: None,
            file_path: None,
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::Header,
        });
        view.line_kinds.push(GitLineKind::Header);

        view.lines.push(GitStatusLine {
            text: "Unstaged changes:".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: None,
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::SectionHeader(GitSection::Unstaged),
        });
        view.line_kinds
            .push(GitLineKind::SectionHeader(GitSection::Unstaged));

        view.lines.push(GitStatusLine {
            text: "  M src/main.rs".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: false,
            is_collapsed: true,
            kind: GitLineKind::File {
                section: GitSection::Unstaged,
                status_char: 'M',
            },
        });
        view.line_kinds.push(GitLineKind::File {
            section: GitSection::Unstaged,
            status_char: 'M',
        });

        assert_eq!(view.lines.len(), 3);
        assert_eq!(view.line_kinds.len(), 3);
        assert!(view.lines[0].is_header);
        assert_eq!(view.lines[2].file_path.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn git_status_line_mapping() {
        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
        // Header
        view.lines.push(GitStatusLine {
            text: "Head:     main".to_string(),
            section: None,
            file_path: None,
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::Header,
        });
        // Section
        view.lines.push(GitStatusLine {
            text: "Staged changes:".to_string(),
            section: Some(GitSection::Staged),
            file_path: None,
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: true,
            is_collapsed: false,
            kind: GitLineKind::SectionHeader(GitSection::Staged),
        });
        // File
        view.lines.push(GitStatusLine {
            text: "  A new_file.rs".to_string(),
            section: Some(GitSection::Staged),
            file_path: Some("new_file.rs".to_string()),
            hunk: None,
            hunk_index: None,
            hunk_header: None,
            is_header: false,
            is_collapsed: true,
            kind: GitLineKind::File {
                section: GitSection::Staged,
                status_char: 'A',
            },
        });

        // Row 0 = header, no file
        assert!(view.lines[0].file_path.is_none());
        // Row 1 = section header, no file
        assert!(view.lines[1].file_path.is_none());
        // Row 2 = file entry
        assert_eq!(view.lines[2].file_path.as_deref(), Some("new_file.rs"));
        assert_eq!(view.lines[2].section, Some(GitSection::Staged));
    }
}
