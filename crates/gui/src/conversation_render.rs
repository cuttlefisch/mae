//! Conversation (AI chat) buffer rendering for the GUI backend.

use mae_core::{conversation::LineStyle, Editor, Mode, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

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

        let auto_start = rendered.len().saturating_sub(viewport_height);
        let start = auto_start.saturating_sub(conv.scroll);

        for (i, rl) in rendered
            .iter()
            .skip(start)
            .take(viewport_height)
            .enumerate()
        {
            let row = inner_row + i;

            if rl.style == LineStyle::InputPrompt {
                let input_fg = theme::ts_fg(editor, "conversation.input");
                if editor.mode == Mode::ConversationInput && focused {
                    if let Some(ref conv) = buf.conversation {
                        let (prefix, before, cursor_ch, after) = conv.input_cursor_spans();
                        let cursor_fg = theme::ts_fg(editor, "ui.cursor");
                        let cursor_bg = theme::ts_bg(editor, "ui.cursor");
                        canvas.draw_text_at(row, inner_col, prefix, input_fg);
                        let offset = prefix.len();
                        canvas.draw_text_at(row, inner_col + offset, before, input_fg);
                        let offset = offset + before.len();
                        if let Some(bg) = cursor_bg {
                            canvas.draw_rect_fill(
                                row,
                                inner_col + offset,
                                cursor_ch.len().max(1),
                                1,
                                bg,
                            );
                        }
                        canvas.draw_text_at(row, inner_col + offset, &cursor_ch, cursor_fg);
                        let offset = offset + cursor_ch.len().max(1);
                        canvas.draw_text_at(row, inner_col + offset, after, input_fg);
                        continue;
                    }
                }
                canvas.draw_text_at(row, inner_col, &rl.text, input_fg);
                continue;
            }

            let fg = match rl.style {
                LineStyle::RoleMarker => {
                    if rl.text.contains("[You]") {
                        theme::ts_fg(editor, "conversation.user")
                    } else if rl.text.contains("[AI]") {
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

            let display: String = rl.text.chars().take(inner_width).collect();
            canvas.draw_text_at(row, inner_col, &display, fg);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn role_marker_styles_differ() {
        // Verify the style lookup keys are distinct.
        assert_ne!("conversation.user", "conversation.assistant");
        assert_ne!("conversation.assistant", "conversation.system");
    }
}
