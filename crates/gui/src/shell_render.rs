//! Shell buffer rendering: translates alacritty_terminal grid cells into
//! Skia drawing calls with full color and attribute support.

use mae_core::{Editor, Window};
use mae_shell::grid_types::{CellFlags, Color as AColor, Colors, NamedColor};
use mae_shell::ShellTerminal;
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::text::StyledCell;
use crate::theme;

/// Render a shell terminal buffer window.
pub fn render_shell_window(
    canvas: &mut SkiaCanvas,
    _buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
    shell: &ShellTerminal,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let border_fg = if focused {
        theme::ts_fg(editor, "ui.window.border.active")
    } else {
        theme::ts_fg(editor, "ui.window.border")
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

    draw_window_border(
        canvas,
        area_row,
        area_col,
        area_width,
        area_height,
        border_fg,
        &title,
    );

    let inner_row = area_row + 1;
    let inner_col = area_col + 1;
    let inner_width = area_width.saturating_sub(2);
    let inner_height = area_height.saturating_sub(2);

    render_shell_grid(
        canvas,
        editor,
        shell,
        focused,
        inner_row,
        inner_col,
        inner_width,
        inner_height,
    );
}

/// Render the alacritty terminal grid using Skia.
fn render_shell_grid(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    shell: &ShellTerminal,
    focused: bool,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let term = shell.term();
    let content = term.renderable_content();
    let cursor_point = content.cursor.point;

    let default_fg = theme::ts_fg(editor, "ui.text");
    let default_bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    for indexed in content.display_iter {
        let line_idx = indexed.point.line.0;
        let col_idx = indexed.point.column.0;

        if line_idx < 0 || line_idx as usize >= area_height || col_idx >= area_width {
            continue;
        }

        let flags = indexed.cell.flags;

        if flags.contains(CellFlags::WIDE_CHAR_SPACER)
            || flags.contains(CellFlags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }

        let mut fg_color =
            convert_color(indexed.cell.fg, content.colors, default_fg, &editor.theme);
        let mut bg_color =
            convert_color(indexed.cell.bg, content.colors, default_bg, &editor.theme);

        if flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg_color, &mut bg_color);
        }

        let bold = flags.contains(CellFlags::BOLD);
        let italic = flags.contains(CellFlags::ITALIC);
        let underline = flags.intersects(CellFlags::ALL_UNDERLINES);
        let hidden = flags.contains(CellFlags::HIDDEN);

        let cell = StyledCell {
            ch: if hidden { ' ' } else { indexed.cell.c },
            fg: fg_color,
            bg: Some(bg_color),
            bold,
            italic,
            underline,
        };

        let row = area_row + line_idx as usize;
        let col = area_col + col_idx;

        // Draw bg.
        if let Some(bg) = cell.bg {
            canvas.draw_rect_fill(row, col, 1, 1, bg);
        }
        // Draw char.
        if cell.ch != ' ' {
            if cell.bold {
                canvas.draw_text_bold(row, col, &cell.ch.to_string(), cell.fg);
            } else {
                canvas.draw_text_at(row, col, &cell.ch.to_string(), cell.fg);
            }
        }
    }

    // Cursor.
    if focused && cursor_point.line.0 >= 0 {
        let crow = area_row + cursor_point.line.0 as usize;
        let ccol = area_col + cursor_point.column.0;
        if crow < area_row + area_height && ccol < area_col + area_width {
            let cursor_style = editor.theme.style("ui.cursor");
            let cursor_color = theme::color_or(cursor_style.bg, theme::DEFAULT_FG);
            canvas.draw_rect_fill(crow, ccol, 1, 1, cursor_color);
        }
    }
}

/// Convert an alacritty_terminal Color to a Skia Color4f.
///
/// Resolution order for named colors:
/// 1. alacritty_terminal's own color overrides (from `colors`)
/// 2. Editor theme palette (e.g. gruvbox's `red = "#cc241d"`)
/// 3. Hardcoded xterm defaults
fn convert_color(
    color: AColor,
    colors: &Colors,
    default: Color4f,
    theme: &mae_core::Theme,
) -> Color4f {
    match color {
        AColor::Spec(rgb) => Color4f::new(
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
            1.0,
        ),
        AColor::Indexed(idx) => {
            if let Some(rgb) = colors[idx as usize] {
                Color4f::new(
                    rgb.r as f32 / 255.0,
                    rgb.g as f32 / 255.0,
                    rgb.b as f32 / 255.0,
                    1.0,
                )
            } else {
                default
            }
        }
        AColor::Named(named) => {
            if let Some(rgb) = colors[named] {
                Color4f::new(
                    rgb.r as f32 / 255.0,
                    rgb.g as f32 / 255.0,
                    rgb.b as f32 / 255.0,
                    1.0,
                )
            } else if let Some(color) = resolve_named_from_theme(named, theme) {
                color
            } else {
                named_color_to_skia(named)
            }
        }
    }
}

/// Try to resolve a NamedColor via the editor theme palette.
///
/// Themes use different naming conventions (gruvbox: "purple"/"aqua",
/// dracula: "pink"/"cyan", catppuccin: "mauve"/"teal"). We try the
/// canonical ANSI name first, then common aliases.
fn resolve_named_from_theme(named: NamedColor, theme: &mae_core::Theme) -> Option<Color4f> {
    let candidates: &[&str] = match named {
        NamedColor::Black | NamedColor::DimBlack => &["black", "bg0", "base", "crust"],
        NamedColor::Red | NamedColor::DimRed => &["red", "maroon"],
        NamedColor::Green | NamedColor::DimGreen => &["green"],
        NamedColor::Yellow | NamedColor::DimYellow => &["yellow", "peach", "orange"],
        NamedColor::Blue | NamedColor::DimBlue => &["blue", "sapphire"],
        NamedColor::Magenta | NamedColor::DimMagenta => &["magenta", "purple", "pink", "mauve"],
        NamedColor::Cyan | NamedColor::DimCyan => &["cyan", "aqua", "teal", "sky"],
        NamedColor::White | NamedColor::DimWhite => &["white", "fg0", "fg1", "text", "fg"],
        NamedColor::BrightBlack => &["bright_black", "bg3", "overlay0", "comment"],
        NamedColor::BrightRed => &["bright_red", "red"],
        NamedColor::BrightGreen => &["bright_green", "green"],
        NamedColor::BrightYellow => &["bright_yellow", "yellow"],
        NamedColor::BrightBlue => &["bright_blue", "blue", "lavender"],
        NamedColor::BrightMagenta => {
            &["bright_magenta", "bright_purple", "purple", "pink", "mauve"]
        }
        NamedColor::BrightCyan => &["bright_cyan", "bright_aqua", "aqua", "teal", "sky"],
        NamedColor::BrightWhite => &["bright_white", "fg0", "text", "fg"],
        NamedColor::Foreground | NamedColor::BrightForeground => {
            &["fg", "fg1", "fg0", "text", "foreground"]
        }
        NamedColor::DimForeground => &["fg", "fg2", "fg3", "subtext0"],
        NamedColor::Background => &["bg", "bg0", "base", "background"],
        _ => return None,
    };
    for key in candidates {
        if let Some(c) = theme.palette.get(*key) {
            return Some(theme::theme_color_to_skia(c));
        }
    }
    None
}

fn named_color_to_skia(named: NamedColor) -> Color4f {
    let (r, g, b) = match named {
        NamedColor::Black | NamedColor::DimBlack => (0, 0, 0),
        NamedColor::Red | NamedColor::DimRed => (205, 0, 0),
        NamedColor::Green | NamedColor::DimGreen => (0, 205, 0),
        NamedColor::Yellow | NamedColor::DimYellow => (205, 205, 0),
        NamedColor::Blue | NamedColor::DimBlue => (0, 0, 238),
        NamedColor::Magenta | NamedColor::DimMagenta => (205, 0, 205),
        NamedColor::Cyan | NamedColor::DimCyan => (0, 205, 205),
        NamedColor::White | NamedColor::DimWhite => (229, 229, 229),
        NamedColor::BrightBlack => (127, 127, 127),
        NamedColor::BrightRed => (255, 0, 0),
        NamedColor::BrightGreen => (0, 255, 0),
        NamedColor::BrightYellow => (255, 255, 0),
        NamedColor::BrightBlue => (92, 92, 255),
        NamedColor::BrightMagenta => (255, 0, 255),
        NamedColor::BrightCyan => (0, 255, 255),
        NamedColor::BrightWhite => (255, 255, 255),
        NamedColor::Foreground | NamedColor::BrightForeground => (229, 229, 229),
        NamedColor::DimForeground => (192, 192, 192),
        NamedColor::Background => (0, 0, 0),
        _ => (229, 229, 229),
    };
    Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

fn draw_window_border(
    canvas: &mut SkiaCanvas,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    color: Color4f,
    title: &str,
) {
    if width < 2 || height < 2 {
        return;
    }
    let top = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row, col, &top, color);
    if title.len() + 2 < width {
        canvas.draw_text_at(row, col + 1, title, color);
    }
    for r in 1..height.saturating_sub(1) {
        canvas.draw_text_at(row + r, col, "│", color);
        canvas.draw_text_at(row + r, col + width - 1, "│", color);
    }
    let bottom = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row + height - 1, col, &bottom, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_color_black() {
        let c = named_color_to_skia(NamedColor::Black);
        assert!(c.r < 0.01);
        assert!(c.g < 0.01);
        assert!(c.b < 0.01);
    }

    #[test]
    fn named_color_bright_white() {
        let c = named_color_to_skia(NamedColor::BrightWhite);
        assert!(c.r > 0.99);
    }

    #[test]
    fn named_color_red() {
        let c = named_color_to_skia(NamedColor::Red);
        assert!(c.r > 0.7);
        assert!(c.g < 0.01);
    }
}
