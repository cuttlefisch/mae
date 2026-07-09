//! Bottom status line: model/provider, round count, permission mode.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::AppState;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let mode_label = format!("{:?}", app.permission_mode);
    let line = Line::from(vec![
        Span::styled(
            format!(" {}/{} ", app.provider, app.model),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(" round {} ", app.round)),
        Span::styled(
            format!(" perms:{mode_label} "),
            Style::default().fg(Color::Black).bg(Color::Magenta),
        ),
        Span::raw("  /help for commands"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
