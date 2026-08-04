//! Shared "line-kind status buffer" span computation.
//!
//! `*Git Status*` (`git_status.rs`), `*Notifications*` (`notifications.rs`),
//! and `*KB Sharing*` (`kb_sharing.rs`) each render by mapping a per-line
//! `Kind` enum to a theme key and highlighting the whole line — before this
//! module, that loop was copied byte-for-byte three times (only the `Kind`
//! type and its `theme_key_of` match differed). [`compute_line_kind_spans`]
//! is now the one implementation; each buffer kind's `compute_*_spans`
//! becomes a thin adapter that supplies its `Kind` iterator and theme-key
//! function.
//!
//! ADR-087 domain note: this walks the rope in the **char** domain
//! (`rope.line_to_char` / `len_chars`) and converts to byte offsets only at
//! the boundary (`rope.char_to_byte`), which is safe for any content
//! (CJK, combining marks, …) because `char_to_byte` is an exact,
//! version-stable conversion — unlike a byte-length assumption borrowed
//! from a different string.
//!
//! **Do not route `*Agenda*` (`agenda.rs`) through this.**
//! `compute_agenda_spans` is a structurally different shape: it produces
//! *multiple sub-line spans per line* (the TODO-state keyword and the
//! priority marker are each highlighted separately within a line), not one
//! whole-line span — so it isn't a fourth copy of this loop, it's a
//! different algorithm that happens to also produce `HighlightSpan`s. It
//! also works in a pure byte domain, using `rope.line(i).len_bytes()` to
//! step between lines and `str::find` (byte offsets) *within the view's own
//! `AgendaLine::text` string* to locate the keyword — correct only because
//! that string is the exact source the rope's line content was built from
//! (`render_agenda_text` joins the same `line.text` fields the rope is
//! populated with). Folding it into this generic byte-vs-char-domain-agnostic
//! walk would silently downgrade it from sub-line to whole-line
//! highlighting, which is why it stays separate rather than becoming a
//! fourth call site here.
use crate::syntax::HighlightSpan;

/// Compute one full-line `HighlightSpan` per non-blank, themed line.
///
/// `kinds` yields one `&K` per rope line, in order (typically
/// `view.lines.iter().map(|l| &l.kind)`). `is_blank` and `theme_key_of`
/// classify each `K`; a line is skipped (no span emitted) when `is_blank`
/// returns true, when its theme key is `"ui.text"` (the sentinel these
/// three buffer kinds use for "default color, no span needed"), or when its
/// content (excluding the trailing newline) is empty.
pub fn compute_line_kind_spans<'a, K: 'a>(
    kinds: impl Iterator<Item = &'a K>,
    rope: &ropey::Rope,
    is_blank: impl Fn(&K) -> bool,
    theme_key_of: impl Fn(&K) -> &'static str,
) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();

    for (line_idx, kind) in kinds.enumerate() {
        if is_blank(kind) {
            continue;
        }
        let theme_key = theme_key_of(kind);
        if theme_key == "ui.text" {
            continue; // default color, no span needed
        }
        if line_idx >= rope.len_lines() {
            break;
        }
        let line_start_char = rope.line_to_char(line_idx);
        let line_len = rope.line(line_idx).len_chars();
        // Exclude trailing newline from the span.
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
    use ropey::Rope;

    #[derive(PartialEq)]
    enum TestKind {
        Header,
        Item,
        Blank,
        Muted, // maps to "ui.text" — should be skipped like blank
    }

    fn theme_key(k: &TestKind) -> &'static str {
        match k {
            TestKind::Header => "git.header",
            TestKind::Item => "diagnostic.warn",
            TestKind::Blank => "ui.text",
            TestKind::Muted => "ui.text",
        }
    }

    #[test]
    fn full_line_spans_for_themed_lines() {
        let rope = Rope::from_str("Header\nitem one\n\n");
        let kinds = [TestKind::Header, TestKind::Item, TestKind::Blank];
        let spans =
            compute_line_kind_spans(kinds.iter(), &rope, |k| *k == TestKind::Blank, theme_key);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].theme_key, "git.header");
        assert_eq!(
            &"Header\nitem one\n\n"[spans[0].byte_start..spans[0].byte_end],
            "Header"
        );
        assert_eq!(spans[1].theme_key, "diagnostic.warn");
        assert_eq!(
            &"Header\nitem one\n\n"[spans[1].byte_start..spans[1].byte_end],
            "item one"
        );
    }

    #[test]
    fn muted_ui_text_lines_produce_no_span() {
        let rope = Rope::from_str("plain\n");
        let kinds = [TestKind::Muted];
        let spans = compute_line_kind_spans(kinds.iter(), &rope, |_| false, theme_key);
        assert!(spans.is_empty());
    }

    #[test]
    fn out_of_range_line_index_stops_without_panicking() {
        // More kinds than actual rope lines — must break, not panic.
        let rope = Rope::from_str("only one line\n");
        let kinds = [TestKind::Header, TestKind::Item, TestKind::Item];
        let spans =
            compute_line_kind_spans(kinds.iter(), &rope, |k| *k == TestKind::Blank, theme_key);
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn non_ascii_line_content_has_correct_byte_span() {
        // Multi-byte content: char-domain walk + char_to_byte must still
        // land on exact byte boundaries (ADR-087).
        let text = "日本語ヘッダー\nsecond line\n";
        let rope = Rope::from_str(text);
        let kinds = [TestKind::Header, TestKind::Item];
        let spans =
            compute_line_kind_spans(kinds.iter(), &rope, |k| *k == TestKind::Blank, theme_key);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            &text[spans[0].byte_start..spans[0].byte_end],
            "日本語ヘッダー"
        );
        assert_eq!(&text[spans[1].byte_start..spans[1].byte_end], "second line");
    }
}
