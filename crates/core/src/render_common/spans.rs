//! Shared highlight span selection for buffer kinds using the standard text pipeline.

use crate::buffer::Buffer;
use crate::syntax::HighlightSpan;

/// Layer inline markup spans onto base spans for a buffer.
pub fn enrich_spans_with_markup(
    base: &mut Vec<HighlightSpan>,
    buf: &Buffer,
    flavor: crate::syntax::MarkupFlavor,
) {
    if flavor == crate::syntax::MarkupFlavor::None {
        return;
    }
    let source: String = buf.rope().chars().collect();
    base.extend(crate::syntax::compute_markup_spans(&source, flavor));
    base.sort_by_key(|s| s.byte_start);
}

/// Compute highlight spans for buffer kinds that use the standard text pipeline.
/// Returns `None` for kinds with specialized renderers (Shell, Debug, Messages, etc.)
/// — the caller should delegate to their dedicated render function.
pub fn highlight_spans_for_buffer(buf: &Buffer) -> Option<Vec<HighlightSpan>> {
    match buf.kind {
        crate::buffer::BufferKind::Help => Some(super::help::compute_help_spans(buf)),
        crate::buffer::BufferKind::GitStatus => {
            Some(super::git_status::compute_git_status_spans(buf))
        }
        crate::buffer::BufferKind::Conversation => Some(
            buf.conversation()
                .map(|c| c.highlight_spans_with_markup(buf.rope()))
                .unwrap_or_default(),
        ),
        _ if buf.name == "*AI-Diff*" => Some(crate::diff::diff_highlight_spans(buf.rope())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, BufferKind};

    #[test]
    fn highlight_spans_help_returns_some() {
        let mut buf = Buffer::new();
        buf.kind = BufferKind::Help;
        assert!(highlight_spans_for_buffer(&buf).is_some());
    }

    #[test]
    fn highlight_spans_text_returns_none() {
        let buf = Buffer::new();
        assert!(highlight_spans_for_buffer(&buf).is_none());
    }

    #[test]
    fn highlight_spans_git_status_returns_some() {
        let mut buf = Buffer::new();
        buf.kind = BufferKind::GitStatus;
        assert!(highlight_spans_for_buffer(&buf).is_some());
    }

    #[test]
    fn enrich_spans_adds_markup() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "**bold** text");
        let mut spans = Vec::new();
        enrich_spans_with_markup(&mut spans, &buf, crate::syntax::MarkupFlavor::Markdown);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "expected markup.bold span after enrichment"
        );
    }

    #[test]
    fn enrich_spans_preserves_existing() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "**bold** text");
        let mut spans = vec![HighlightSpan {
            byte_start: 9,
            byte_end: 13,
            theme_key: "keyword",
        }];
        enrich_spans_with_markup(&mut spans, &buf, crate::syntax::MarkupFlavor::Markdown);
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "existing spans must be preserved"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.bold"),
            "markup spans must be added"
        );
    }

    #[test]
    fn enrich_spans_none_flavor_noop() {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, "**bold** text");
        let mut spans = Vec::new();
        enrich_spans_with_markup(&mut spans, &buf, crate::syntax::MarkupFlavor::None);
        assert!(spans.is_empty(), "None flavor should not add spans");
    }
}
