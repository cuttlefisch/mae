//! Scrollable transcript pane: user turns, assistant text, and
//! collapsed-by-default expandable tool-call blocks — the "reviewability" UX.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::AppState;

/// One entry in the conversation transcript.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    User(String),
    Assistant(String),
    SystemNote(String),
    ToolCall {
        name: String,
        arguments: serde_json::Value,
        /// `None` while the call is still in flight.
        result: Option<(bool, String)>,
        expanded: bool,
    },
}

/// One-line summary for a collapsed tool-call block:
/// `tool_name(args) -> result summary`.
fn tool_call_summary(
    name: &str,
    arguments: &serde_json::Value,
    result: &Option<(bool, String)>,
) -> String {
    let args = serde_json::to_string(arguments).unwrap_or_default();
    match result {
        None => format!("{name}({args}) -> …"),
        Some((true, output)) => format!("{name}({args}) -> {}", first_line_preview(output)),
        Some((false, output)) => {
            format!("{name}({args}) -> FAILED: {}", first_line_preview(output))
        }
    }
}

fn first_line_preview(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(80).collect::<String>())
    } else {
        line.to_string()
    }
}

fn entry_lines(entry: &TranscriptEntry) -> Vec<Line<'static>> {
    match entry {
        TranscriptEntry::User(text) => vec![Line::from(vec![
            Span::styled("you: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(text.clone()),
        ])],
        TranscriptEntry::Assistant(text) => vec![Line::from(vec![
            Span::styled(
                "assistant: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(text.clone()),
        ])],
        TranscriptEntry::SystemNote(text) => vec![Line::from(Span::styled(
            format!("· {text}"),
            Style::default().fg(Color::DarkGray),
        ))],
        TranscriptEntry::ToolCall {
            name,
            arguments,
            result,
            expanded,
        } => {
            let summary = tool_call_summary(name, arguments, result);
            let color = match result {
                None => Color::Yellow,
                Some((true, _)) => Color::Green,
                Some((false, _)) => Color::Red,
            };
            let mut lines = vec![Line::from(Span::styled(
                format!("  ⚙ {summary}"),
                Style::default().fg(color),
            ))];
            if *expanded {
                if let Some((_, output)) = result {
                    for line in output.lines() {
                        lines.push(Line::from(format!("      {line}")));
                    }
                }
            }
            lines
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for entry in &app.transcript {
        lines.extend(entry_lines(entry));
    }

    let block = Block::default().title(" mae-agent ").borders(Borders::ALL);
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll_offset as u16, 0));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_tool_call_summary_omits_full_output() {
        let long_output = "line one\nline two\nline three";
        let summary = tool_call_summary(
            "kb_search",
            &serde_json::json!({"query": "x"}),
            &Some((true, long_output.to_string())),
        );
        assert!(summary.contains("kb_search"));
        assert!(summary.contains("line one"));
        assert!(!summary.contains("line two"));
    }

    #[test]
    fn failed_tool_call_summary_is_marked() {
        let summary = tool_call_summary(
            "kb_get",
            &serde_json::json!({}),
            &Some((false, "boom".into())),
        );
        assert!(summary.contains("FAILED"));
        assert!(summary.contains("boom"));
    }

    #[test]
    fn pending_tool_call_summary_shows_ellipsis() {
        let summary = tool_call_summary("kb_get", &serde_json::json!({}), &None);
        assert!(summary.ends_with("…"));
    }

    #[test]
    fn expanded_tool_call_includes_full_output_lines() {
        let entry = TranscriptEntry::ToolCall {
            name: "kb_search".into(),
            arguments: serde_json::json!({}),
            result: Some((true, "a\nb\nc".into())),
            expanded: true,
        };
        let lines = entry_lines(&entry);
        // 1 summary line + 3 expanded body lines.
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn collapsed_tool_call_is_single_line() {
        let entry = TranscriptEntry::ToolCall {
            name: "kb_search".into(),
            arguments: serde_json::json!({}),
            result: Some((true, "a\nb\nc".into())),
            expanded: false,
        };
        assert_eq!(entry_lines(&entry).len(), 1);
    }
}
