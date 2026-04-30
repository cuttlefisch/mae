//! Shared Help buffer span computation — used by both TUI and GUI renderers.

use crate::buffer::Buffer;
use crate::syntax::HighlightSpan;

/// Compute highlight spans for a Help buffer: heading detection,
/// inline markdown/org style spans, and link spans from the HelpView.
pub fn compute_help_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();

    // Heading spans from leading `*` or `#` chars in rope lines.
    let rope = buf.rope();
    for line_idx in 0..buf.line_count() {
        let line = rope.line(line_idx);
        let first_char = line.chars().next().unwrap_or(' ');
        let (prefix_count, is_heading) = if first_char == '*' {
            let c = line.chars().take_while(|&ch| ch == '*').count();
            (c, c > 0 && line.len_chars() > c && line.char(c) == ' ')
        } else if first_char == '#' {
            let c = line.chars().take_while(|&ch| ch == '#').count();
            (c, c > 0 && line.len_chars() > c && line.char(c) == ' ')
        } else {
            (0, false)
        };
        if is_heading && prefix_count > 0 {
            let line_start = rope.line_to_char(line_idx);
            let line_len = line.len_chars();
            let text_len = if line_idx + 1 < buf.line_count() {
                line_len.saturating_sub(1)
            } else {
                line_len
            };
            let byte_start = rope.char_to_byte(line_start);
            let byte_end = rope.char_to_byte(line_start + text_len);
            spans.push(HighlightSpan {
                byte_start,
                byte_end,
                theme_key: "markup.heading",
            });
        }
    }

    // Inline style spans (bold, code, italic) — both markdown and org syntax.
    let source_text: String = rope.chars().collect();
    spans.extend(crate::syntax::compute_markdown_style_spans(&source_text));
    spans.extend(crate::syntax::compute_org_style_spans(&source_text));

    // Link spans from help view.
    if let Some(view) = buf.help_view() {
        for (i, link) in view.rendered_links.iter().enumerate() {
            let is_focused_link = view.focused_link == Some(i);
            spans.push(HighlightSpan {
                byte_start: link.byte_start,
                byte_end: link.byte_end,
                theme_key: if is_focused_link {
                    "ui.selection"
                } else {
                    "markup.link"
                },
            });
        }
    }
    spans.sort_by_key(|s| s.byte_start);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_spans_empty_buffer() {
        let buf = Buffer::new_help("index");
        let spans = compute_help_spans(&buf);
        assert!(spans.is_empty());
    }

    #[test]
    fn help_spans_detect_heading() {
        let mut buf = Buffer::new_help("index");
        buf.read_only = false;
        buf.insert_text_at(0, "* Heading\nBody text\n");
        buf.read_only = true;
        let spans = compute_help_spans(&buf);
        assert!(
            spans.iter().any(|s| s.theme_key == "markup.heading"),
            "should detect heading span"
        );
    }
}
