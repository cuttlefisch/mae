//! Shared `*Notifications*` buffer rendering — theme key mapping for semantic
//! line types. `compute_notif_spans()` produces `HighlightSpan`s consumed by both
//! the GUI and TUI renderers (mirrors `compute_git_status_spans`).

use crate::buffer::Buffer;
use crate::notifications_view::NotifLineKind;
use crate::syntax::HighlightSpan;

/// Map a `NotifLineKind` to a theme key. Reuses the widely-defined `git.*` and
/// `diagnostic.*` keys so the buffer is colored across every theme.
pub fn notif_line_theme_key(kind: &NotifLineKind) -> &'static str {
    match kind {
        NotifLineKind::Header => "git.header",
        NotifLineKind::CategoryHeader(_) => "git.section",
        NotifLineKind::Item { .. } => "diagnostic.warn",
        NotifLineKind::ActionRow { .. } => "diagnostic.hint",
        NotifLineKind::ResolvedItem { .. } => "comment",
        NotifLineKind::Blank => "ui.text",
    }
}

/// Compute highlight spans for a `*Notifications*` buffer by iterating
/// `lines`. Delegates to the shared
/// `line_kind_spans::compute_line_kind_spans` (see that module's doc for
/// why `*Agenda*` is not a fourth caller of the same helper).
pub fn compute_notif_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let Some(view) = buf.notif_view() else {
        return Vec::new();
    };
    crate::render_common::line_kind_spans::compute_line_kind_spans(
        view.lines.iter().map(|l| &l.kind),
        buf.rope(),
        |k| matches!(k, NotifLineKind::Blank),
        notif_line_theme_key,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_keys_per_kind() {
        assert_eq!(notif_line_theme_key(&NotifLineKind::Header), "git.header");
        assert_eq!(
            notif_line_theme_key(&NotifLineKind::CategoryHeader("collab".into())),
            "git.section"
        );
        assert_eq!(
            notif_line_theme_key(&NotifLineKind::Item { notif_id: 1 }),
            "diagnostic.warn"
        );
        assert_eq!(
            notif_line_theme_key(&NotifLineKind::ActionRow {
                notif_id: 1,
                action_idx: 0
            }),
            "diagnostic.hint"
        );
        assert_eq!(
            notif_line_theme_key(&NotifLineKind::ResolvedItem { notif_id: 1 }),
            "comment"
        );
    }

    #[test]
    fn empty_for_non_notif_buffer() {
        let buf = Buffer::new();
        assert!(compute_notif_spans(&buf).is_empty());
    }
}
