//! Git status buffer data model (Phase 6 M5).

use std::collections::HashMap;
use std::path::PathBuf;

/// A section in the git status buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Which hunk within the file (0-based). Set on DiffHunk and DiffLine rows.
    pub hunk_index: Option<usize>,
    /// Raw `@@ ... @@` header for this hunk. Set on DiffHunk rows.
    pub hunk_header: Option<String>,
    /// Semantic kind for rendering.
    pub kind: GitLineKind,
}

impl GitStatusLine {
    /// A blank separator line.
    pub fn blank() -> Self {
        GitStatusLine {
            text: String::new(),
            section: None,
            file_path: None,
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::Blank,
        }
    }
}

/// Type-safe collapse key for multi-level fold in git status buffers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CollapseKey {
    Section(GitSection),
    File {
        path: String,
        section: GitSection,
    },
    Hunk {
        path: String,
        section: GitSection,
        index: usize,
    },
}

/// Structured state for the `*Git Status*` buffer.
#[derive(Debug, Clone)]
pub struct GitStatusView {
    pub lines: Vec<GitStatusLine>,
    /// Multi-level collapse state.
    pub collapsed: HashMap<CollapseKey, bool>,
    /// Root directory of the repository.
    pub repo_root: PathBuf,
}

impl GitStatusView {
    pub fn new(repo_root: PathBuf) -> Self {
        GitStatusView {
            lines: Vec::new(),
            collapsed: HashMap::new(),
            repo_root,
        }
    }

    /// Get the line kind at a given row index.
    pub fn kind_at(&self, row: usize) -> Option<&GitLineKind> {
        self.lines.get(row).map(|l| &l.kind)
    }

    /// Toggle collapse state for a key. Default state is "not collapsed" (expanded).
    pub fn toggle(&mut self, key: CollapseKey) {
        let collapsed = self.collapsed.entry(key).or_insert(false);
        *collapsed = !*collapsed;
    }

    /// Check if a key is collapsed.
    pub fn is_collapsed(&self, key: &CollapseKey) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }

    /// Build the collapse key for a given line.
    pub fn collapse_key_for_line(line: &GitStatusLine) -> Option<CollapseKey> {
        match &line.kind {
            GitLineKind::SectionHeader(section) => Some(CollapseKey::Section(*section)),
            GitLineKind::File { section, .. } => {
                line.file_path.as_ref().map(|p| CollapseKey::File {
                    path: p.clone(),
                    section: *section,
                })
            }
            GitLineKind::DiffHunk => {
                if let (Some(path), Some(section), Some(idx)) =
                    (&line.file_path, &line.section, line.hunk_index)
                {
                    Some(CollapseKey::Hunk {
                        path: path.clone(),
                        section: *section,
                        index: idx,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check if a file's diff is expanded.
    /// Files default to collapsed (diffs hidden). Expanded = collapsed entry is `false`.
    pub fn is_file_expanded(&self, path: &str, section: &GitSection) -> bool {
        let key = CollapseKey::File {
            path: path.to_string(),
            section: *section,
        };
        self.collapsed.get(&key).copied() == Some(false)
    }

    /// Toggle expansion/collapse of a file's inline diff.
    /// Default is `true` (collapsed); first toggle flips to `false` (expanded).
    pub fn toggle_file_expansion(&mut self, path: &str, section: &GitSection) {
        let key = CollapseKey::File {
            path: path.to_string(),
            section: *section,
        };
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
        let section_key = CollapseKey::Section(GitSection::Unstaged);
        assert!(!view.is_collapsed(&section_key));
        view.toggle(section_key.clone());
        assert!(view.is_collapsed(&section_key));

        // File collapse
        let file_key = CollapseKey::File {
            path: "src/main.rs".to_string(),
            section: GitSection::Unstaged,
        };
        assert!(!view.is_collapsed(&file_key));
        view.toggle(file_key.clone());
        assert!(view.is_collapsed(&file_key));

        // Hunk collapse
        let hunk_key = CollapseKey::Hunk {
            path: "src/main.rs".to_string(),
            section: GitSection::Unstaged,
            index: 0,
        };
        assert!(!view.is_collapsed(&hunk_key));
        view.toggle(hunk_key.clone());
        assert!(view.is_collapsed(&hunk_key));
    }

    #[test]
    fn collapse_key_for_line_variants() {
        let section_line = GitStatusLine {
            text: "Unstaged changes:".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: None,
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::SectionHeader(GitSection::Unstaged),
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&section_line),
            Some(CollapseKey::Section(GitSection::Unstaged))
        );

        let file_line = GitStatusLine {
            text: "  M src/main.rs".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::File {
                section: GitSection::Unstaged,
                status_char: 'M',
            },
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&file_line),
            Some(CollapseKey::File {
                path: "src/main.rs".to_string(),
                section: GitSection::Unstaged,
            })
        );

        let hunk_line = GitStatusLine {
            text: "    @@ -1,3 +1,4 @@".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk_index: Some(0),
            hunk_header: Some("@@ -1,3 +1,4 @@".to_string()),
            kind: GitLineKind::DiffHunk,
        };
        assert_eq!(
            GitStatusView::collapse_key_for_line(&hunk_line),
            Some(CollapseKey::Hunk {
                path: "src/main.rs".to_string(),
                section: GitSection::Unstaged,
                index: 0,
            })
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
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::Header,
        });

        view.lines.push(GitStatusLine {
            text: "Unstaged changes:".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: None,
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::SectionHeader(GitSection::Unstaged),
        });

        view.lines.push(GitStatusLine {
            text: "  M src/main.rs".to_string(),
            section: Some(GitSection::Unstaged),
            file_path: Some("src/main.rs".to_string()),
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::File {
                section: GitSection::Unstaged,
                status_char: 'M',
            },
        });

        assert_eq!(view.lines.len(), 3);
        assert!(matches!(view.lines[0].kind, GitLineKind::Header));
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
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::Header,
        });
        // Section
        view.lines.push(GitStatusLine {
            text: "Staged changes:".to_string(),
            section: Some(GitSection::Staged),
            file_path: None,
            hunk_index: None,
            hunk_header: None,
            kind: GitLineKind::SectionHeader(GitSection::Staged),
        });
        // File
        view.lines.push(GitStatusLine {
            text: "  A new_file.rs".to_string(),
            section: Some(GitSection::Staged),
            file_path: Some("new_file.rs".to_string()),
            hunk_index: None,
            hunk_header: None,
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
