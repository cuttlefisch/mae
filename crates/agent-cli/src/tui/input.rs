//! Fixed bottom multi-line input box.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::AppState;

pub fn render(frame: &mut Frame, area: Rect, app: &AppState) {
    let title = if app.busy {
        " thinking… "
    } else {
        " message (Enter to send) "
    };
    let border_color = if app.busy {
        Color::DarkGray
    } else {
        Color::Blue
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let paragraph = Paragraph::new(app.input.as_str()).block(block);
    frame.render_widget(paragraph, area);

    if !app.busy {
        let cursor_x = area.x + 1 + app.cursor as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}
