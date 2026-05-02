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
    // Help content mixes markdown and org syntax — compute both.
    let source_text: String = rope.chars().collect();
    spans.extend(crate::syntax::compute_markup_spans(
        &source_text,
        crate::syntax::MarkupFlavor::Markdown,
    ));
    spans.extend(crate::syntax::compute_org_style_spans(&source_text));

    // Syntax highlighting for fenced code blocks (tree-sitter per block).
    spans.extend(code_block_language_spans(&source_text));

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

/// Find fenced code blocks (` ```lang ... ``` `) and run tree-sitter
/// highlighting on each block's content. Returns spans with byte offsets
/// relative to the full source.
fn code_block_language_spans(source: &str) -> Vec<HighlightSpan> {
    use regex::Regex;
    use std::sync::OnceLock;

    static FENCE_OPEN: OnceLock<Regex> = OnceLock::new();
    let fence_open = FENCE_OPEN.get_or_init(|| Regex::new(r"(?m)^```(\w+)\s*$").unwrap());

    let mut spans = Vec::new();
    let mut search_start = 0;

    while let Some(open_match) = fence_open.find_at(source, search_start) {
        let caps = fence_open.captures(&source[open_match.start()..]).unwrap();
        let lang_id = caps.get(1).unwrap().as_str();
        let content_start = open_match.end() + 1; // skip the newline after ```lang
        if content_start >= source.len() {
            break;
        }

        // Find closing fence
        let Some(close_pos) = source[content_start..].find("\n```") else {
            break;
        };
        let content_end = content_start + close_pos;
        let block_content = &source[content_start..content_end];

        if let Some(lang) = crate::language_from_id(lang_id) {
            let block_spans = crate::syntax::compute_spans_standalone(lang, block_content);
            for s in block_spans {
                spans.push(HighlightSpan {
                    byte_start: s.byte_start + content_start,
                    byte_end: s.byte_end + content_start,
                    theme_key: s.theme_key,
                });
            }
        }

        search_start = content_end + 4; // skip past \n```
    }
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
    fn help_spans_code_block_highlighting() {
        let mut buf = Buffer::new_help("test");
        buf.read_only = false;
        buf.insert_text_at(0, "# Example\n\n```rust\nfn hello() {}\n```\n");
        buf.read_only = true;
        let spans = compute_help_spans(&buf);
        assert!(
            spans.iter().any(|s| s.theme_key == "keyword"),
            "help code block should have keyword spans, got: {:?}",
            spans
        );
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
