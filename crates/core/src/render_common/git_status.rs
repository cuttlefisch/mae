//! Shared git status rendering logic — theme key mapping for semantic line types.

use crate::git_status::{DiffLineType, GitLineKind, GitSection};

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
        GitLineKind::File { .. } => "ui.text",
        GitLineKind::DiffHunk => "diff.hunk",
        GitLineKind::DiffLine(DiffLineType::Added) => "diff.added",
        GitLineKind::DiffLine(DiffLineType::Removed) => "diff.removed",
        GitLineKind::DiffLine(DiffLineType::Context) => "comment",
        GitLineKind::Blank => "ui.text",
    }
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
}
