//! Conversation (AI chat) window rendering.

use mae_core::link_detect::render_segments;
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
                mae_core::conversation::LineStyle::ToolRunning => ts(editor, "conversation.tool"),
                mae_core::conversation::LineStyle::ToolPending => Style::default(),
                mae_core::conversation::LineStyle::ToolSuccess => {
                    ts(editor, "conversation.tool.result")
                }
                mae_core::conversation::LineStyle::ToolError => ts(editor, "diagnostic.error"),
                mae_core::conversation::LineStyle::SystemText => ts(editor, "conversation.system"),
                mae_core::conversation::LineStyle::Separator => Style::default(),
                mae_core::conversation::LineStyle::InputPrompt => {
                    // Legacy — InputPrompt no longer produced; input is in *ai-input*.
                    ts(editor, "conversation.input")
                }
            };
            // Render markdown/org links as underlined labels
            let segs = render_segments(&rl.text);
            if segs.len() == 1 && segs[0].link_target.is_none() {
                lines.push(Line::from(Span::styled(rl.text.clone(), style)));
            } else {
                let link_style = ts(editor, "markup.link").add_modifier(Modifier::UNDERLINED);
                let spans: Vec<Span> = segs
                    .iter()
                    .map(|seg| {
                        if seg.link_target.is_some() {
                            Span::styled(seg.text.clone(), link_style)
                        } else {
                            Span::styled(seg.text.clone(), style)
                        }
                    })
                    .collect();
                lines.push(Line::from(spans));
            }
        }

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
    }
}
