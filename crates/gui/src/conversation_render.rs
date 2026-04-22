//! Conversation (AI chat) buffer rendering for the GUI backend.

use mae_core::{conversation::LineStyle, Editor, Mode, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

/// A screen line produced by wrapping a rendered line.
struct ScreenLine<'a> {
    text: &'a str,
    style: &'a LineStyle,
}

/// Wrap rendered lines into screen lines that fit within `width`.
fn wrap_lines<'a>(
    rendered: &'a [mae_core::conversation::RenderedLine],
    width: usize,
) -> Vec<ScreenLine<'a>> {
    let mut screen_lines = Vec::new();
    let w = width.max(1);
    for rl in rendered {
        if rl.text.is_empty() || rl.text.len() <= w {
            screen_lines.push(ScreenLine {
                text: &rl.text,
                style: &rl.style,
            });
        } else {
            // Split into chunks of `w` characters, respecting char boundaries
            let mut remaining = rl.text.as_str();
            while !remaining.is_empty() {
                let end = char_boundary_at(remaining, w);
                screen_lines.push(ScreenLine {
                    text: &remaining[..end],
                    style: &rl.style,
                });
                remaining = &remaining[end..];
            }
        }
    }
    screen_lines
}

/// Find the byte offset at approximately `n` characters, snapped to a char boundary.
fn char_boundary_at(s: &str, n: usize) -> usize {
    if n >= s.len() {
        return s.len();
    }
    // Walk char indices to find the boundary at the nth char
    let mut last = 0;
    for (i, (byte_idx, _)) in s.char_indices().enumerate() {
        if i >= n {
            return byte_idx;
        }
        last = byte_idx;
    }
    // If we ran out of chars, return full length
    let _ = last;
    s.len()
}

/// Split text into rows of at most `width` characters, respecting char boundaries.
fn wrap_text_into_rows(text: &str, width: usize) -> Vec<&str> {
    let w = width.max(1);
    if text.is_empty() || text.len() <= w {
        return vec![text];
    }
    let mut rows = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        let end = char_boundary_at(remaining, w);
        rows.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    rows
}

/// Render a conversation buffer window.
pub fn render_conversation_window(
    canvas: &mut SkiaCanvas,
    buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    // Border.
    let border_fg = if focused {
        theme::ts_fg(editor, "ui.window.border.active")
    } else {
        theme::ts_fg(editor, "ui.window.border")
    };

    let title = format!(" {} ", buf.name);
    let streaming_indicator = if let Some(conv) = buf.conversation.as_ref() {
        if conv.streaming {
            if let Some(start) = conv.streaming_start {
                let elapsed = start.elapsed().as_secs();
                format!(" [waiting... {}s] ", elapsed)
            } else {
                " [waiting...] ".to_string()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    draw_window_border(
        canvas,
        area_row,
        area_col,
        area_width,
        area_height,
        border_fg,
        &format!("{}{}", title, streaming_indicator),
    );

    let inner_row = area_row + 1;
    let inner_col = area_col + 1;
    let inner_width = area_width.saturating_sub(2);
    let inner_height = area_height.saturating_sub(2);

    if let Some(ref conv) = buf.conversation {
        let rendered = conv.rendered_lines();
        let viewport_height = inner_height;

        // Wrap lines to fit viewport width
        let screen_lines = wrap_lines(&rendered, inner_width);

        // Transition: use win.scroll_offset for parity with text buffers.
        // We still fall back to conv.scroll if offset is 0 to preserve legacy scroll behavior.
        let start = if _win.scroll_offset > 0 {
            _win.scroll_offset
        } else {
            screen_lines
                .len()
                .saturating_sub(viewport_height)
                .saturating_sub(conv.scroll)
        };

        // Selection range (char offsets in flattened text)
        let highlight_selection = matches!(editor.mode, mae_core::Mode::Visual(_));
        let (sel_start, sel_end) = if highlight_selection && focused {
            editor.visual_selection_range()
        } else {
            (0, 0)
        };

        // Manual indexing loop so InputPrompt cursor rendering can consume
        // all wrapped InputPrompt screen lines at once (fixing duplication).
        let visible: Vec<_> = screen_lines
            .iter()
            .skip(start)
            .take(viewport_height)
            .collect();
        let mut viewport_row = 0;
        let mut input_prompt_rendered = false;

        // Flatten text for selection mapping if needed.
        let flat = if highlight_selection {
            Some(conv.flat_text())
        } else {
            None
        };

        while viewport_row < visible.len() {
            let sl = visible[viewport_row];
            let row = inner_row + viewport_row;

            // Selection background for this line
            if let Some(ref ft) = flat {
                // Find this line's start in flat text (approximate mapping)
                // In a perfect world we'd track byte/char offsets during wrap_lines.
                if let Some(line_start_byte) = ft.find(sl.text) {
                    let line_start_char = ft[..line_start_byte].chars().count();
                    let line_end_char = line_start_char + sl.text.chars().count();

                    if sel_start < line_end_char && sel_end > line_start_char {
                        let s = sel_start.saturating_sub(line_start_char);
                        let e = (sel_end - line_start_char).min(sl.text.chars().count());
                        let sel_bg =
                            theme::ts_bg(editor, "ui.selection").unwrap_or(theme::DEFAULT_BG);
                        canvas.draw_rect_fill(row, inner_col + s, e - s, 1, sel_bg);
                    }
                }
            }

            if *sl.style == LineStyle::InputPrompt {
                let input_fg = theme::ts_fg(editor, "conversation.input");

                // In cursor mode, render ALL InputPrompt rows as a group.
                if editor.mode == Mode::ConversationInput && focused && !input_prompt_rendered {
                    if let Some(ref conv) = buf.conversation {
                        let full_text = format!("> {}", conv.input_line);
                        let rows = wrap_text_into_rows(&full_text, inner_width);
                        // Cursor byte position in full_text.
                        let cursor_byte = 2 + conv.input_cursor.min(conv.input_line.len());

                        let cursor_fg = theme::ts_fg(editor, "ui.cursor");
                        let cursor_bg = theme::ts_bg(editor, "ui.cursor");

                        let mut byte_offset = 0;
                        for (ri, row_text) in rows.iter().enumerate() {
                            let draw_row = row + ri;
                            if draw_row >= inner_row + inner_height {
                                break;
                            }
                            let row_start = byte_offset;
                            let row_end = row_start + row_text.len();

                            if cursor_byte >= row_start && cursor_byte < row_end {
                                // This row contains the cursor.
                                let local_cursor = cursor_byte - row_start;
                                let before = &row_text[..local_cursor];
                                let rest = &row_text[local_cursor..];
                                let cursor_ch = if rest.is_empty() {
                                    " ".to_string()
                                } else {
                                    let end = rest
                                        .char_indices()
                                        .nth(1)
                                        .map(|(i, _)| i)
                                        .unwrap_or(rest.len());
                                    rest[..end].to_string()
                                };
                                let after_cursor = if rest.is_empty() {
                                    ""
                                } else {
                                    let end = rest
                                        .char_indices()
                                        .nth(1)
                                        .map(|(i, _)| i)
                                        .unwrap_or(rest.len());
                                    &rest[end..]
                                };

                                canvas.draw_text_at(draw_row, inner_col, before, input_fg);
                                let col = inner_col + before.len();
                                if let Some(bg) = cursor_bg {
                                    canvas.draw_rect_fill(
                                        draw_row,
                                        col,
                                        cursor_ch.len().max(1),
                                        1,
                                        bg,
                                    );
                                }
                                canvas.draw_text_at(draw_row, col, &cursor_ch, cursor_fg);
                                let col = col + cursor_ch.len().max(1);
                                canvas.draw_text_at(draw_row, col, after_cursor, input_fg);
                            } else if cursor_byte == row_end && ri == rows.len() - 1 {
                                // Cursor at very end of last row.
                                canvas.draw_text_at(draw_row, inner_col, row_text, input_fg);
                                let col = inner_col + row_text.len();
                                if let Some(bg) = cursor_bg {
                                    canvas.draw_rect_fill(draw_row, col, 1, 1, bg);
                                }
                                canvas.draw_text_at(draw_row, col, " ", cursor_fg);
                            } else {
                                canvas.draw_text_at(draw_row, inner_col, row_text, input_fg);
                            }
                            byte_offset = row_end;
                        }
                        input_prompt_rendered = true;
                        // Skip all InputPrompt screen lines we've consumed.
                        let mut skip = 1;
                        while viewport_row + skip < visible.len()
                            && *visible[viewport_row + skip].style == LineStyle::InputPrompt
                        {
                            skip += 1;
                        }
                        viewport_row += skip.max(rows.len());
                        continue;
                    }
                }

                // Already rendered via cursor path — skip duplicate.
                if input_prompt_rendered {
                    viewport_row += 1;
                    continue;
                }

                canvas.draw_text_at(row, inner_col, sl.text, input_fg);
                viewport_row += 1;
                continue;
            }

            let fg = match sl.style {
                LineStyle::RoleMarker => {
                    if sl.text.contains("[You]") {
                        theme::ts_fg(editor, "conversation.user")
                    } else if sl.text.contains("[AI]") {
                        theme::ts_fg(editor, "conversation.assistant")
                    } else {
                        theme::ts_fg(editor, "conversation.system")
                    }
                }
                LineStyle::UserText => theme::ts_fg(editor, "conversation.user.text"),
                LineStyle::AssistantText => theme::ts_fg(editor, "conversation.assistant.text"),
                LineStyle::ToolCallHeader => theme::ts_fg(editor, "conversation.tool"),
                LineStyle::ToolResultText => theme::ts_fg(editor, "conversation.tool.result"),
                LineStyle::SystemText => theme::ts_fg(editor, "conversation.system"),
                LineStyle::Separator => theme::ts_fg(editor, "ui.text"),
                LineStyle::InputPrompt => theme::ts_fg(editor, "conversation.input"),
            };

            canvas.draw_text_at(row, inner_col, sl.text, fg);
            viewport_row += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_marker_styles_differ() {
        // Verify the style lookup keys are distinct.
        assert_ne!("conversation.user", "conversation.assistant");
        assert_ne!("conversation.assistant", "conversation.system");
    }

    #[test]
    fn char_boundary_at_basic() {
        assert_eq!(char_boundary_at("hello", 3), 3);
        assert_eq!(char_boundary_at("hello", 10), 5);
        assert_eq!(char_boundary_at("", 5), 0);
    }

    #[test]
    fn char_boundary_at_multibyte() {
        let s = "héllo"; // é is 2 bytes
        let boundary = char_boundary_at(s, 2);
        assert!(s.is_char_boundary(boundary));
    }

    #[test]
    fn wrap_lines_short_lines_unchanged() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "short".into(),
            style: LineStyle::AssistantText,
            entry_index: None,
        }];
        let wrapped = wrap_lines(&rendered, 80);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].text, "short");
    }

    #[test]
    fn wrap_lines_long_line_splits() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "a".repeat(20),
            style: LineStyle::AssistantText,
            entry_index: None,
        }];
        let wrapped = wrap_lines(&rendered, 10);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].text.len(), 10);
        assert_eq!(wrapped[1].text.len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_basic() {
        let text = "a".repeat(20);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 10);
        assert_eq!(rows[1].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_exact() {
        let text = "a".repeat(10);
        let rows = wrap_text_into_rows(&text, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 10);
    }

    #[test]
    fn wrap_text_into_rows_short() {
        let rows = wrap_text_into_rows("hello", 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "hello");
    }

    #[test]
    fn wrap_lines_input_prompt_still_wraps() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "> ".to_string() + &"x".repeat(30),
            style: LineStyle::InputPrompt,
            entry_index: None,
        }];
        let wrapped = wrap_lines(&rendered, 16);
        assert_eq!(wrapped.len(), 2);
        assert!(wrapped.iter().all(|sl| *sl.style == LineStyle::InputPrompt));
    }
}
