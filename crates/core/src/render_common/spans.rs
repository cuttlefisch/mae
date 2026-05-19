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
        crate::buffer::BufferKind::Kb => Some(super::kb::compute_kb_spans(buf)),
        crate::buffer::BufferKind::GitStatus => {
            Some(super::git_status::compute_git_status_spans(buf))
        }
        crate::buffer::BufferKind::Conversation => Some(
            buf.conversation()
                .map(|c| c.highlight_spans_with_markup(buf.rope()))
                .unwrap_or_default(),
        ),
        crate::buffer::BufferKind::Diff => Some(crate::diff::diff_highlight_spans(buf.rope())),
        crate::buffer::BufferKind::Agenda => Some(super::agenda::compute_agenda_spans(buf)),
        crate::buffer::BufferKind::Text => None,
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
        buf.kind = BufferKind::Kb;
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

    /// Regression test: org Text buffers must return None so the syntax cache
    /// provides full structural spans (TODO/DONE, checkboxes, etc.) instead
    /// of the heading-only shortcut that was silently dropping all other org spans.
    #[test]
    fn org_text_buffer_returns_none_for_syntax_pipeline() {
        let mut buf = Buffer::new();
        buf.kind = BufferKind::Text;
        buf.set_file_path(std::path::PathBuf::from("/tmp/test.org"));
        buf.insert_text_at(0, "* TODO Heading\n- [ ] item\n");
        assert!(
            highlight_spans_for_buffer(&buf).is_none(),
            "org Text buffer must return None to use syntax cache pipeline"
        );
    }

    /// Verify that org structural spans reach the rendering pipeline end-to-end.
    /// Simulates what both TUI and GUI renderers do: check highlight_spans_for_buffer,
    /// if None → use syntax cache (compute_org_spans).
    #[test]
    fn org_text_buffer_gets_structural_spans_via_syntax() {
        let source = "* TODO Fix bug\n- [ ] item\n- [x] done\n#+TITLE: Test\n";
        let spans = crate::syntax::markup::compute_org_spans(source);

        assert!(
            spans.iter().any(|s| s.theme_key == "markup.heading"),
            "missing markup.heading span"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.todo"),
            "missing markup.todo span"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.checkbox"),
            "missing markup.checkbox span"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.theme_key == "markup.checkbox.checked"),
            "missing markup.checkbox.checked span"
        );
        assert!(
            spans.iter().any(|s| s.theme_key == "attribute"),
            "missing attribute span for #+TITLE directive"
        );
    }

    /// Verify heading scale spans exist for org Text buffers via the syntax pipeline.
    #[test]
    fn org_text_buffer_gets_heading_scale_spans() {
        let source = "* Big Heading\n** Sub Heading\nBody text\n";
        let spans = crate::syntax::markup::compute_org_spans(source);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.heading"),
            "markup.heading span required for GUI heading scale"
        );
    }

    /// Verify property drawer spans are produced.
    #[test]
    fn org_drawer_dimming() {
        let source = "* Heading\n:PROPERTIES:\n :ID: abc-123\n:END:\n";
        let spans = crate::syntax::markup::compute_org_spans(source);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.drawer"),
            "missing markup.drawer span for property drawer"
        );
    }

    /// Verify org link spans are computed for display region concealment.
    #[test]
    fn org_text_buffer_link_spans() {
        let source = "Visit [[https://example.com][Example]] for details.\n";
        let spans = crate::syntax::markup::compute_org_spans(source);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.link"),
            "missing markup.link span for org link"
        );
    }
}
