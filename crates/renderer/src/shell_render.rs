//! Shell buffer rendering: translates alacritty_terminal grid cells into
//! ratatui widgets with full color and attribute support.

use mae_core::{Editor, Window};
use mae_shell::grid_types::{CellFlags, Color as AColor, Colors, NamedColor};
use mae_shell::ShellTerminal;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders};

use crate::theme_convert::ts;

/// Render a shell terminal buffer inside a window with a border.
pub(crate) fn render_shell_window(
    frame: &mut Frame,
    area: Rect,
    _buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
    shell: &ShellTerminal,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let title_text = shell.title();
    let offset = shell.display_offset();
    let base_title = if title_text.is_empty() {
        "*Terminal*".to_string()
    } else {
        title_text.to_string()
    };
    let title = if offset > 0 {
        format!(" {} [\u{2191}{}] ", base_title, offset)
    } else {
        format!(" {} ", base_title)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    render_shell_grid(frame, inner, shell, focused);
}

/// Render the alacritty terminal grid into the given area.
fn render_shell_grid(frame: &mut Frame, area: Rect, shell: &ShellTerminal, focused: bool) {
    let term = shell.term();
    let content = term.renderable_content();

    let cursor_point = content.cursor.point;
    let cols = area.width as usize;

    // Build a 2D grid: rows × cols of (char, Style).
    // Pre-fill with spaces so gaps render correctly.
    let default_style = Style::default();
    let rows = area.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> = vec![vec![(' ', default_style); cols]; rows];

    for indexed in content.display_iter {
        let line_idx = indexed.point.line.0;
        let col_idx = indexed.point.column.0;

        if line_idx < 0 || line_idx as usize >= rows || col_idx >= cols {
            continue;
        }

        let flags = indexed.cell.flags;

        // Skip wide char spacers (the filler cell after a double-width char).
        if flags.contains(CellFlags::WIDE_CHAR_SPACER)
            || flags.contains(CellFlags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }

        let fg_color = convert_color(indexed.cell.fg, content.colors);
        let bg_color = convert_color(indexed.cell.bg, content.colors);

        let mut style = Style::default().fg(fg_color).bg(bg_color);

        // Handle inverse (swap fg/bg).
        if flags.contains(CellFlags::INVERSE) {
            style = Style::default().fg(bg_color).bg(fg_color);
        }

        if flags.contains(CellFlags::BOLD) {
            style = style.add_modifier(Modifier::BOLD);
        }
        if flags.contains(CellFlags::ITALIC) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if flags.intersects(CellFlags::ALL_UNDERLINES) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if flags.contains(CellFlags::DIM) {
            style = style.add_modifier(Modifier::DIM);
        }
        if flags.contains(CellFlags::STRIKEOUT) {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if flags.contains(CellFlags::HIDDEN) {
            style = style.add_modifier(Modifier::HIDDEN);
        }

        grid[line_idx as usize][col_idx] = (indexed.cell.c, style);
    }

    // Render each line from the grid.
    for (row_idx, row) in grid.iter().enumerate() {
        let spans: Vec<Span> = row
            .iter()
            .map(|(c, style)| Span::styled(c.to_string(), *style))
            .collect();

        let line = Line::from(spans);
        let line_area = Rect::new(area.x, area.y + row_idx as u16, area.width, 1);
        frame.render_widget(line, line_area);
    }

    // Set cursor position for the terminal.
    if focused && cursor_point.line.0 >= 0 {
        let cursor_row = area.y + cursor_point.line.0 as u16;
        let cursor_col = area.x + cursor_point.column.0 as u16;
        if cursor_row < area.y + area.height && cursor_col < area.x + area.width {
            frame.set_cursor_position((cursor_col, cursor_row));
        }
    }
}

/// Convert an alacritty_terminal Color to a ratatui Color.
fn convert_color(color: AColor, colors: &Colors) -> Color {
    match color {
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(idx) => {
            if let Some(rgb) = colors[idx as usize] {
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            } else {
                Color::Indexed(idx)
            }
        }
        AColor::Named(named) => {
            if let Some(rgb) = colors[named] {
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            } else {
                match named {
                    NamedColor::Black | NamedColor::DimBlack => Color::Black,
                    NamedColor::Red | NamedColor::DimRed => Color::Red,
                    NamedColor::Green | NamedColor::DimGreen => Color::Green,
                    NamedColor::Yellow | NamedColor::DimYellow => Color::Yellow,
                    NamedColor::Blue | NamedColor::DimBlue => Color::Blue,
                    NamedColor::Magenta | NamedColor::DimMagenta => Color::Magenta,
                    NamedColor::Cyan | NamedColor::DimCyan => Color::Cyan,
                    NamedColor::White | NamedColor::DimWhite => Color::White,
                    NamedColor::BrightBlack => Color::DarkGray,
                    NamedColor::BrightRed => Color::LightRed,
                    NamedColor::BrightGreen => Color::LightGreen,
                    NamedColor::BrightYellow => Color::LightYellow,
                    NamedColor::BrightBlue => Color::LightBlue,
                    NamedColor::BrightMagenta => Color::LightMagenta,
                    NamedColor::BrightCyan => Color::LightCyan,
                    NamedColor::BrightWhite => Color::White,
                    NamedColor::Foreground | NamedColor::BrightForeground => Color::Reset,
                    NamedColor::DimForeground => Color::Gray,
                    NamedColor::Background => Color::Reset,
                    _ => Color::Reset,
                }
            }
        }
    }
}
