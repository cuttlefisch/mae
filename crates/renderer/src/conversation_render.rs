//! Conversation (AI chat) window rendering.

use mae_core::{Editor, Window};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme_convert::ts;

pub(crate) fn render_conversation_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!("{}{}", title, streaming_indicator));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(ref conv) = buf.conversation {
        let rendered = conv.rendered_lines();
        let viewport_height = inner.height as usize;

        let auto_start = rendered.len().saturating_sub(viewport_height);
        let start = auto_start.saturating_sub(conv.scroll);

        let mut lines: Vec<Line> = Vec::new();
        for rl in rendered.iter().skip(start).take(viewport_height) {
            // Handle InputPrompt specially for visible cursor rendering.
            if rl.style == mae_core::conversation::LineStyle::InputPrompt {
                let input_style = ts(editor, "conversation.input");
                if editor.mode == mae_core::Mode::ConversationInput && focused {
                    if let Some(ref conv) = buf.conversation {
                        let (prefix, before, cursor_ch, after) = conv.input_cursor_spans();
                        let cursor_style = ts(editor, "ui.cursor");
                        lines.push(Line::from(vec![
                            Span::styled(prefix.to_string(), input_style),
                            Span::styled(before.to_string(), input_style),
                            Span::styled(cursor_ch, cursor_style),
                            Span::styled(after.to_string(), input_style),
                        ]));
                        continue;
                    }
                }
                lines.push(Line::from(Span::styled(rl.text.clone(), input_style)));
                continue;
            }

            let style = match rl.style {
                mae_core::conversation::LineStyle::RoleMarker => {
                    if rl.text.contains("[You]") {
                        ts(editor, "conversation.user")
                    } else if rl.text.contains("[AI]") {
                        ts(editor, "conversation.assistant")
                    } else {
                        ts(editor, "conversation.system")
                    }
                }
                mae_core::conversation::LineStyle::UserText => ts(editor, "conversation.user.text"),
                mae_core::conversation::LineStyle::AssistantText => {
                    ts(editor, "conversation.assistant.text")
                }
                mae_core::conversation::LineStyle::ToolCallHeader => {
                    ts(editor, "conversation.tool")
                }
                mae_core::conversation::LineStyle::ToolResultText => {
                    ts(editor, "conversation.tool.result")
                }
                mae_core::conversation::LineStyle::SystemText => ts(editor, "conversation.system"),
                mae_core::conversation::LineStyle::Separator => Style::default(),
                mae_core::conversation::LineStyle::InputPrompt => {
                    // Handled above — this branch is unreachable in practice.
                    ts(editor, "conversation.input")
                }
            };
            lines.push(Line::from(Span::styled(rl.text.clone(), style)));
        }

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }
}
