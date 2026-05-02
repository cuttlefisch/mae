//! Shared file tree entry formatting for both GUI and TUI renderers.
//!
//! Computes the display string (indent + icon + name + git marker) for each
//! visible file tree entry. Backend-specific code converts these into
//! draw calls or ratatui Spans.

use crate::file_tree::{icon_for_path, FileGitStatus, FileTree};

/// A pre-formatted file tree line ready for rendering.
pub struct FileTreeLine {
    /// The full display string: indent + icon + name + git suffix.
    pub display: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether this entry is the selected row.
    pub is_selected: bool,
    /// Git status theme key (e.g. `"diff.modified"`), if non-clean.
    pub git_theme_key: Option<&'static str>,
}

/// Compute visible file tree lines with scroll clamping.
///
/// Returns `(lines, effective_scroll_offset)`.
pub fn format_file_tree_lines(ft: &FileTree, viewport_height: usize) -> (Vec<FileTreeLine>, usize) {
    let mut scroll = ft.scroll_offset;
    if ft.selected < scroll {
        scroll = ft.selected;
    }
    if viewport_height > 0 && ft.selected >= scroll + viewport_height {
        scroll = ft.selected.saturating_sub(viewport_height - 1);
    }

    let lines: Vec<FileTreeLine> = ft
        .entries
        .iter()
        .skip(scroll)
        .take(viewport_height)
        .enumerate()
        .map(|(i, entry)| {
            let global_idx = scroll + i;
            let indent = "  ".repeat(entry.depth);
            let is_expanded = entry.is_dir && ft.expanded_dirs.contains(&entry.path);
            let icon = icon_for_path(&entry.path, entry.is_dir, is_expanded);
            let git_suffix = match entry.git_status {
                Some(gs) if gs != FileGitStatus::Clean => format!(" [{}]", gs.marker_char()),
                _ => String::new(),
            };
            let display = format!("{}{} {}{}", indent, icon, entry.name, git_suffix);
            let git_theme_key = entry
                .git_status
                .filter(|gs| *gs != FileGitStatus::Clean)
                .map(|gs| gs.theme_key());

            FileTreeLine {
                display,
                is_dir: entry.is_dir,
                is_selected: global_idx == ft.selected,
                git_theme_key,
            }
        })
        .collect();

    (lines, scroll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn make_tree(entries: Vec<crate::file_tree::FileTreeEntry>) -> FileTree {
        FileTree {
            root: PathBuf::from("/test"),
            entries,
            expanded_dirs: HashSet::new(),
            selected: 0,
            scroll_offset: 0,
            git_statuses: std::collections::HashMap::new(),
            fold_cycle_state: crate::file_tree::FoldCycleState::Default,
            default_expanded: HashSet::new(),
        }
    }

    fn entry(name: &str, depth: usize, is_dir: bool) -> crate::file_tree::FileTreeEntry {
        crate::file_tree::FileTreeEntry {
            path: PathBuf::from(name),
            name: name.to_string(),
            is_dir,
            depth,
            git_status: None,
        }
    }

    #[test]
    fn file_tree_indent_depth() {
        let ft = make_tree(vec![
            entry("src", 0, true),
            entry("main.rs", 1, false),
            entry("lib.rs", 2, false),
        ]);
        let (lines, _) = format_file_tree_lines(&ft, 10);
        assert_eq!(lines.len(), 3);
        // depth 0 = no indent, depth 1 = 2 spaces, depth 2 = 4 spaces
        assert!(!lines[0].display.starts_with(' '));
        assert!(lines[1].display.starts_with("  "));
        assert!(lines[2].display.starts_with("    "));
    }

    #[test]
    fn file_tree_git_markers() {
        let mut e = entry("modified.rs", 0, false);
        e.git_status = Some(FileGitStatus::Modified);
        let ft = make_tree(vec![e]);
        let (lines, _) = format_file_tree_lines(&ft, 10);
        assert!(lines[0].display.contains("[M]"));
        assert_eq!(lines[0].git_theme_key, Some("diff.modified"));
    }

    #[test]
    fn file_tree_selected() {
        let ft = make_tree(vec![entry("a", 0, false), entry("b", 0, false)]);
        let (lines, _) = format_file_tree_lines(&ft, 10);
        assert!(lines[0].is_selected);
        assert!(!lines[1].is_selected);
    }
}
