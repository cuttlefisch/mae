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
    pub is_header: bool,
    pub is_collapsed: bool,
    /// Semantic kind for rendering.
    pub kind: GitLineKind,
}

/// Structured state for the `*Git Status*` buffer.
#[derive(Debug, Clone)]
pub struct GitStatusView {
    pub lines: Vec<GitStatusLine>,
    /// Parallel to `lines` — maps each buffer line to its semantic kind.
    pub line_kinds: Vec<GitLineKind>,
    /// Which sections/files are currently collapsed.
    pub collapsed_paths: HashMap<String, bool>,
    /// Root directory of the repository.
    pub repo_root: PathBuf,
}

impl GitStatusView {
    pub fn new(repo_root: PathBuf) -> Self {
        GitStatusView {
            lines: Vec::new(),
            line_kinds: Vec::new(),
            collapsed_paths: HashMap::new(),
            repo_root,
        }
    }

    /// Toggle expansion/collapse of a file's inline diff.
    pub fn toggle_file_expansion(&mut self, path: &str) {
        let collapsed = self.collapsed_paths.entry(path.to_string()).or_insert(true);
        *collapsed = !*collapsed;
    }

    /// Check if a file's diff is expanded (not collapsed).
    pub fn is_expanded(&self, path: &str) -> bool {
        self.collapsed_paths.get(path).copied() == Some(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn git_status_expand_collapse() {
        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
        // Initially not expanded
        assert!(!view.is_expanded("src/main.rs"));
        // Toggle to expand
        view.toggle_file_expansion("src/main.rs");
        assert!(view.is_expanded("src/main.rs"));
        // Toggle to collapse
        view.toggle_file_expansion("src/main.rs");
        assert!(!view.is_expanded("src/main.rs"));
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
