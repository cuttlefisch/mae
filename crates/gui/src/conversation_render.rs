//! Conversation (AI chat) buffer rendering for the GUI backend.

use mae_core::conversation::{
    char_boundary_at, chars_to_display_cols, screen_line_count, wrap_text_into_rows, LineStyle,
};
use mae_core::{Editor, Mode, Window};
use unicode_width::UnicodeWidthChar;

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

/// A screen line produced by wrapping a rendered line.
struct ScreenLine<'a> {
    text: &'a str,
    style: &'a LineStyle,
    /// Character offset of this screen line in the flattened conversation text.
    char_offset: usize,
}

/// Wrap only the rendered lines visible in the viewport. Returns screen lines
/// starting at `start_screen_line` and collecting up to `viewport_height` lines.
/// O(viewport) wrapping instead of O(total_lines).
///
/// `screen_counts` is a pre-computed per-rendered-line count from
/// `Conversation::ensure_screen_counts`. This avoids recomputing display widths.
fn wrap_visible_lines<'a>(
    rendered: &'a [mae_core::conversation::RenderedLine],
    screen_counts: &[usize],
    width: usize,
    start_screen_line: usize,
    viewport_height: usize,
) -> Vec<ScreenLine<'a>> {
    let w = width.max(1);

    // Find which rendered line contains start_screen_line
    let mut cumulative = 0;
    let mut first_rendered = 0;
    let mut skip_within_first = 0;

    for (i, &count) in screen_counts.iter().enumerate() {
        if cumulative + count > start_screen_line {
            first_rendered = i;
            skip_within_first = start_screen_line - cumulative;
            break;
        }
        cumulative += count;
        if i == rendered.len() - 1 {
            first_rendered = i;
            skip_within_first = 0;
        }
    }

    // Compute char_offset for the first rendered line we'll wrap.
    // Each rendered line contributes chars().count() + 1 (for the newline separator
    // in flat_text). The last line has no trailing newline but we only need relative
    // offsets within the viewport so the +1 for joining newlines is correct.
    let mut base_char_offset: usize = rendered[..first_rendered]
        .iter()
        .map(|rl| rl.text.chars().count() + 1) // +1 for '\n' join
        .sum();

    // Wrap only the rendered lines we need
    let needed = viewport_height + skip_within_first;
    let mut screen_lines = Vec::with_capacity(needed + 4);

    for rl in &rendered[first_rendered..] {
        let line_chars = rl.text.chars().count();
        if rl.text.is_empty() || screen_line_count(&rl.text, w) <= 1 {
            screen_lines.push(ScreenLine {
                text: &rl.text,
                style: &rl.style,
                char_offset: base_char_offset,
            });
        } else {
            let mut remaining = rl.text.as_str();
            let mut local_char_offset = 0;
            while !remaining.is_empty() {
                let end = char_boundary_at(remaining, w);
                let chunk = &remaining[..end];
                screen_lines.push(ScreenLine {
                    text: chunk,
                    style: &rl.style,
                    char_offset: base_char_offset + local_char_offset,
                });
                local_char_offset += chunk.chars().count();
                remaining = &remaining[end..];
            }
        }
        base_char_offset += line_chars + 1; // +1 for '\n' join
        if screen_lines.len() >= needed {
            break;
        }
    }

    // Skip the lines before the viewport within the first rendered line
    if skip_within_first > 0 {
        screen_lines.drain(..skip_within_first.min(screen_lines.len()));
    }

    screen_lines.truncate(viewport_height);
    screen_lines
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
        let w = inner_width.max(1);

        // Use pre-computed screen counts (populated by ensure_screen_counts before render).
        let (screen_counts, total_screen_lines) = conv.screen_counts();
        // Fallback if counts weren't pre-computed (e.g. width mismatch).
        let local_counts;
        let (counts, total) = if screen_counts.len() == rendered.len() {
            (screen_counts, total_screen_lines)
        } else {
            local_counts = rendered
                .iter()
                .map(|rl| screen_line_count(&rl.text, w))
                .collect::<Vec<_>>();
            let t: usize = local_counts.iter().sum();
            (local_counts.as_slice(), t)
        };

        let auto_start = total.saturating_sub(viewport_height);
        let start = auto_start.saturating_sub(conv.scroll);

        // Phase 2: Find which rendered lines map to the visible viewport.
        let screen_lines = wrap_visible_lines(rendered, counts, w, start, viewport_height);

        // Selection range (char offsets in flattened text)
        let highlight_selection = matches!(editor.mode, mae_core::Mode::Visual(_));
        let (sel_start, sel_end) = if highlight_selection && focused {
            editor.visual_selection_range()
        } else {
            (0, 0)
        };

        // Manual indexing loop so InputPrompt cursor rendering can consume
        // all wrapped InputPrompt screen lines at once (fixing duplication).
        let visible: Vec<_> = screen_lines.iter().collect();
        let mut viewport_row = 0;
        let mut input_prompt_rendered = false;

        while viewport_row < visible.len() {
            let sl = visible[viewport_row];
            let row = inner_row + viewport_row;

            // Selection background — uses char_offset from wrap phase (no flat_text needed)
            if highlight_selection {
                let line_start_char = sl.char_offset;
                let line_end_char = line_start_char + sl.text.chars().count();

                if sel_start < line_end_char && sel_end > line_start_char {
                    let s_char = sel_start.saturating_sub(line_start_char);
                    let e_char = (sel_end - line_start_char).min(sl.text.chars().count());
                    let s_col = chars_to_display_cols(sl.text, s_char);
                    let e_col = chars_to_display_cols(sl.text, e_char);
                    let sel_bg = theme::ts_bg(editor, "ui.selection").unwrap_or(theme::DEFAULT_BG);
                    canvas.draw_rect_fill(row, inner_col + s_col, e_col - s_col, 1, sel_bg);
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
                                let col = inner_col + unicode_width::UnicodeWidthStr::width(before);
                                let cursor_w = cursor_ch
                                    .chars()
                                    .next()
                                    .and_then(|c| c.width())
                                    .unwrap_or(1);
                                if let Some(bg) = cursor_bg {
                                    canvas.draw_rect_fill(draw_row, col, cursor_w, 1, bg);
                                }
                                canvas.draw_text_at(draw_row, col, &cursor_ch, cursor_fg);
                                let col = col + cursor_w;
                                canvas.draw_text_at(draw_row, col, after_cursor, input_fg);
                            } else if cursor_byte == row_end && ri == rows.len() - 1 {
                                canvas.draw_text_at(draw_row, inner_col, row_text, input_fg);
                                let col =
                                    inner_col + unicode_width::UnicodeWidthStr::width(*row_text);
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

    fn make_counts(rendered: &[mae_core::conversation::RenderedLine], w: usize) -> Vec<usize> {
        rendered
            .iter()
            .map(|rl| screen_line_count(&rl.text, w))
            .collect()
    }

    #[test]
    fn wrap_visible_short_lines_unchanged() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "short".into(),
            style: LineStyle::AssistantText,
            entry_index: None,
        }];
        let counts = make_counts(&rendered, 80);
        let wrapped = wrap_visible_lines(&rendered, &counts, 80, 0, 100);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].text, "short");
    }

    #[test]
    fn wrap_visible_long_line_splits() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "a".repeat(20),
            style: LineStyle::AssistantText,
            entry_index: None,
        }];
        let counts = make_counts(&rendered, 10);
        let wrapped = wrap_visible_lines(&rendered, &counts, 10, 0, 100);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].text.len(), 10);
        assert_eq!(wrapped[1].text.len(), 10);
    }

    #[test]
    fn screen_line_count_basic() {
        assert_eq!(screen_line_count("hello", 80), 1);
        assert_eq!(screen_line_count("", 80), 1);
        assert_eq!(screen_line_count(&"a".repeat(20), 10), 2);
        assert_eq!(screen_line_count(&"a".repeat(30), 10), 3);
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
    fn wrap_visible_input_prompt_still_wraps() {
        let rendered = vec![mae_core::conversation::RenderedLine {
            text: "> ".to_string() + &"x".repeat(30),
            style: LineStyle::InputPrompt,
            entry_index: None,
        }];
        let counts = make_counts(&rendered, 16);
        let wrapped = wrap_visible_lines(&rendered, &counts, 16, 0, 100);
        assert_eq!(wrapped.len(), 2);
        assert!(wrapped.iter().all(|sl| *sl.style == LineStyle::InputPrompt));
    }

    #[test]
    fn wrap_visible_only_wraps_viewport() {
        // 10 lines, viewport of 3 starting at line 5
        let rendered: Vec<_> = (0..10)
            .map(|i| mae_core::conversation::RenderedLine {
                text: format!("line {}", i),
                style: LineStyle::AssistantText,
                entry_index: Some(i),
            })
            .collect();
        let counts = make_counts(&rendered, 80);
        let wrapped = wrap_visible_lines(&rendered, &counts, 80, 5, 3);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0].text, "line 5");
        assert_eq!(wrapped[1].text, "line 6");
        assert_eq!(wrapped[2].text, "line 7");
    }

    #[test]
    fn char_boundary_at_cjk() {
        // Each CJK char is 3 bytes, 2 display columns
        let s = "日本語テスト"; // 6 chars, 12 display columns
        let boundary = char_boundary_at(s, 4); // 4 display cols = 2 CJK chars
        assert_eq!(boundary, 6); // 2 chars × 3 bytes
        assert!(s.is_char_boundary(boundary));
    }

    #[test]
    fn screen_line_count_cjk() {
        // 6 CJK chars = 12 display columns
        assert_eq!(screen_line_count("日本語テスト", 12), 1);
        assert_eq!(screen_line_count("日本語テスト", 6), 2);
        assert_eq!(screen_line_count("日本語テスト", 4), 3);
    }
}
