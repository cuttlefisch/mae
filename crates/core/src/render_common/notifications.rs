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

/// Compute highlight spans for a `*Notifications*` buffer by iterating `lines`.
pub fn compute_notif_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let view = match buf.notif_view() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let rope = buf.rope();
    let mut spans = Vec::new();

    for (line_idx, line) in view.lines.iter().enumerate() {
        if matches!(line.kind, NotifLineKind::Blank) {
            continue;
        }
        let theme_key = notif_line_theme_key(&line.kind);
        if theme_key == "ui.text" {
            continue;
        }
        if line_idx >= rope.len_lines() {
            break;
        }
        let line_start_char = rope.line_to_char(line_idx);
        let rope_line = rope.line(line_idx);
        let line_len = rope_line.len_chars();
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
