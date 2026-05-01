//! Shared highlight span selection for buffer kinds using the standard text pipeline.

use crate::buffer::Buffer;
use crate::syntax::HighlightSpan;

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
}
