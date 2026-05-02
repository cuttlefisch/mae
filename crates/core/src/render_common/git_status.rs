//! Shared git status rendering logic — theme key mapping for semantic line types.
//!
//! `compute_git_status_spans()` produces `HighlightSpan`s consumed by both the
//! GUI and TUI renderers, following the same pattern as `compute_help_spans()`.

use crate::buffer::Buffer;
use crate::git_status::{DiffLineType, GitLineKind, GitSection};
use crate::syntax::HighlightSpan;

/// Map a `GitLineKind` to a theme key for rendering.
pub fn git_line_theme_key(kind: &GitLineKind) -> &'static str {
    match kind {
        GitLineKind::Header => "git.header",
        GitLineKind::SectionHeader(_) => "git.section",
        GitLineKind::File {
            section: GitSection::Staged,
            ..
        } => "diff.added",
        GitLineKind::File {
            section: GitSection::Unstaged,
            ..
        } => "diff.removed",
        GitLineKind::File {
            section: GitSection::Untracked,
            ..
        } => "git.untracked",
        GitLineKind::File {
            section: GitSection::Stashes,
            ..
        } => "comment",
        GitLineKind::DiffHunk => "diff.hunk",
        GitLineKind::DiffLine(DiffLineType::Added) => "diff.added",
        GitLineKind::DiffLine(DiffLineType::Removed) => "diff.removed",
        GitLineKind::DiffLine(DiffLineType::Context) => "comment",
        GitLineKind::Blank => "ui.text",
    }
}

/// Compute highlight spans for a GitStatus buffer by iterating `lines`.
/// Each non-blank line gets a full-line span with the theme key from
/// `git_line_theme_key()`. This is the git-status equivalent of
/// `compute_help_spans()`.
pub fn compute_git_status_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let view = match buf.git_status_view() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let rope = buf.rope();
    let mut spans = Vec::new();

    for (line_idx, line) in view.lines.iter().enumerate() {
        if matches!(line.kind, GitLineKind::Blank) {
            continue;
        }
        let theme_key = git_line_theme_key(&line.kind);
        if theme_key == "ui.text" {
            continue; // default color, no span needed
        }
        if line_idx >= rope.len_lines() {
            break;
        }
        let line_start_char = rope.line_to_char(line_idx);
        let line = rope.line(line_idx);
        let line_len = line.len_chars();
        // Exclude trailing newline from span
        let text_len = if line_idx + 1 < rope.len_lines() {
            line_len.saturating_sub(1)
        } else {
            line_len
        };
        if text_len == 0 {
            continue;
        }
        let byte_start = rope.char_to_byte(line_start_char);
        let byte_end = rope.char_to_byte(line_start_char + text_len);
        spans.push(HighlightSpan {
            byte_start,
            byte_end,
            theme_key,
        });
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_line_kind_theme_keys() {
        assert_eq!(git_line_theme_key(&GitLineKind::Header), "git.header");
        assert_eq!(
            git_line_theme_key(&GitLineKind::SectionHeader(GitSection::Staged)),
            "git.section"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::File {
                section: GitSection::Staged,
                status_char: 'A',
            }),
            "diff.added"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::File {
                section: GitSection::Unstaged,
                status_char: 'M',
            }),
            "diff.removed"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::File {
                section: GitSection::Untracked,
                status_char: '?',
            }),
            "git.untracked"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::File {
                section: GitSection::Stashes,
                status_char: 'S',
            }),
            "comment"
        );
        assert_eq!(git_line_theme_key(&GitLineKind::DiffHunk), "diff.hunk");
        assert_eq!(
            git_line_theme_key(&GitLineKind::DiffLine(DiffLineType::Added)),
            "diff.added"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::DiffLine(DiffLineType::Removed)),
            "diff.removed"
        );
        assert_eq!(
            git_line_theme_key(&GitLineKind::DiffLine(DiffLineType::Context)),
            "comment"
        );
        assert_eq!(git_line_theme_key(&GitLineKind::Blank), "ui.text");
    }

    #[test]
    fn compute_git_status_spans_empty() {
        let buf = Buffer::new();
        let spans = compute_git_status_spans(&buf);
        assert!(spans.is_empty(), "non-git buffer should produce no spans");
    }

    #[test]
    fn compute_git_status_spans_produces_themed_lines() {
        use crate::buffer_view::BufferView;
        use crate::git_status::{GitStatusLine, GitStatusView};
        use std::path::PathBuf;

        let mut buf = Buffer::new();
        buf.kind = crate::buffer::BufferKind::GitStatus;

        let mut view = GitStatusView::new(PathBuf::from("/tmp"));
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

        buf.view = BufferView::GitStatus(Box::new(view));
        buf.insert_text_at(0, "Head:     main\nUnstaged changes:\n");

        let spans = compute_git_status_spans(&buf);
        assert!(
            spans.len() >= 2,
            "should produce spans for header + section"
        );
        assert_eq!(spans[0].theme_key, "git.header");
        assert_eq!(spans[1].theme_key, "git.section");
    }
}
