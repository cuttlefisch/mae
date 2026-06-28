//! Shared `*KB Sharing*` buffer rendering — theme key mapping for semantic
//! line types. `compute_kb_sharing_spans()` produces `HighlightSpan`s consumed
//! by both the GUI and TUI renderers (mirrors `compute_git_status_spans` /
//! `compute_notif_spans`). Without this, `BufferKind::KbSharing` falls through
//! to the uncolored path in `spans::highlight_spans_for_buffer`.

use crate::buffer::Buffer;
use crate::kb_sharing::KbSharingLineKind;
use crate::syntax::HighlightSpan;

/// Map a `KbSharingLineKind` to a theme key. Reuses the widely-defined `git.*`
/// and `diagnostic.*` keys so the buffer is colored across every theme.
pub fn kb_sharing_line_theme_key(kind: &KbSharingLineKind) -> &'static str {
    match kind {
        KbSharingLineKind::Header => "git.header",
        KbSharingLineKind::ConnectionLine => "comment",
        KbSharingLineKind::KbHeader { .. } => "git.section",
        KbSharingLineKind::RoleLine { .. } => "diagnostic.hint",
        KbSharingLineKind::PolicyLine { .. } => "diagnostic.hint",
        KbSharingLineKind::MembersHeader { .. } => "git.section",
        KbSharingLineKind::Member { .. } => "ui.text",
        KbSharingLineKind::PendingHeader { .. } => "git.section",
        KbSharingLineKind::Pending { .. } => "diagnostic.warn",
        KbSharingLineKind::BlockedHeader { .. } => "git.section",
        KbSharingLineKind::Blocked { .. } => "diagnostic.error",
        KbSharingLineKind::Blank => "ui.text",
    }
}

/// Compute highlight spans for a `*KB Sharing*` buffer by iterating `lines`.
pub fn compute_kb_sharing_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let view = match buf.kb_sharing_view() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let rope = buf.rope();
    let mut spans = Vec::new();

    for (line_idx, line) in view.lines.iter().enumerate() {
        if matches!(line.kind, KbSharingLineKind::Blank) {
            continue;
        }
        let theme_key = kb_sharing_line_theme_key(&line.kind);
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
        assert_eq!(
            kb_sharing_line_theme_key(&KbSharingLineKind::Header),
            "git.header"
        );
        assert_eq!(
            kb_sharing_line_theme_key(&KbSharingLineKind::KbHeader { kb_id: "kb".into() }),
            "git.section"
        );
        assert_eq!(
            kb_sharing_line_theme_key(&KbSharingLineKind::Pending {
                kb_id: "kb".into(),
                fingerprint: "SHA256:ab".into()
            }),
            "diagnostic.warn"
        );
        assert_eq!(
            kb_sharing_line_theme_key(&KbSharingLineKind::Member {
                kb_id: "kb".into(),
                fingerprint: "SHA256:ab".into()
            }),
            "ui.text"
        );
    }

    #[test]
    fn empty_for_non_kb_sharing_buffer() {
        let buf = Buffer::new();
        assert!(compute_kb_sharing_spans(&buf).is_empty());
    }
}
