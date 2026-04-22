//! Shell buffer rendering: translates alacritty_terminal grid cells into
//! Skia drawing calls with full color and attribute support.

use mae_core::{Editor, Window};
use mae_shell::grid_types::{CellFlags, Color as AColor, Colors, NamedColor};
use mae_shell::ShellTerminal;
use skia_safe::Color4f;
use tracing::trace;

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
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
    trace!(
        width = area_width,
        height = area_height,
        "render_shell_grid enter"
    );
    let term = shell.term();
    let content = term.renderable_content();
    let cursor_point = content.cursor.point;

    let default_fg = theme::ts_fg(editor, "ui.text");
    let default_bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    // Collect cells into a grid for bg-coalescing and text rendering.
    // This reduces ~1920 individual Skia draw_rect_fill calls per frame
    // to ~24-100 coalesced rectangles (one per bg-color run per row).
    #[derive(Clone)]
    struct CellInfo {
        fg: Color4f,
        bg: Color4f,
        ch: char,
        bold: bool,
    }

    // Build a sparse grid of visible cells.
    let mut grid: Vec<Vec<Option<CellInfo>>> = vec![vec![None; area_width]; area_height];

    // Use the already-locked term to get display_offset — calling
    // shell.display_offset() would deadlock (re-entrant FairMutex lock).
    let display_offset = term.grid().display_offset() as i32;
    for indexed in content.display_iter {
        let line_idx = indexed.point.line.0 + display_offset;
        let col_idx = indexed.point.column.0;

        if line_idx < 0 || line_idx as usize >= area_height || col_idx >= area_width {
            continue;
        }

        let flags = indexed.cell.flags;

        let mut fg_color =
            convert_color(indexed.cell.fg, content.colors, default_fg, &editor.theme);
        let mut bg_color =
            convert_color(indexed.cell.bg, content.colors, default_bg, &editor.theme);

        if flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg_color, &mut bg_color);
        }

        // Wide-char spacers: record bg for coalescing but render as space.
        if flags.contains(CellFlags::WIDE_CHAR_SPACER)
            || flags.contains(CellFlags::LEADING_WIDE_CHAR_SPACER)
        {
            grid[line_idx as usize][col_idx] = Some(CellInfo {
                fg: fg_color,
                bg: bg_color,
                ch: ' ',
                bold: false,
            });
            continue;
        }

        let hidden = flags.contains(CellFlags::HIDDEN);

        grid[line_idx as usize][col_idx] = Some(CellInfo {
            fg: fg_color,
            bg: bg_color,
            ch: if hidden { ' ' } else { indexed.cell.c },
            bold: flags.contains(CellFlags::BOLD),
        });
    }

    // Overlay selection highlight if active.
    if let Some(((sel_start_row, sel_start_col), (sel_end_row, sel_end_col))) =
        shell.selection_range()
    {
        let sel_bg =
            theme::ts_bg(editor, "ui.selection").unwrap_or(Color4f::new(0.2, 0.3, 0.6, 1.0));
        for row_idx in sel_start_row..=sel_end_row.min(area_height.saturating_sub(1)) {
            let col_start = if row_idx == sel_start_row {
                sel_start_col
            } else {
                0
            };
            let col_end = if row_idx == sel_end_row {
                sel_end_col
            } else {
                area_width.saturating_sub(1)
            };
            for col_idx in col_start..=col_end.min(area_width.saturating_sub(1)) {
                if let Some(ref mut cell_info) = grid
                    .get_mut(row_idx)
                    .and_then(|row| row.get_mut(col_idx))
                    .and_then(|c| c.as_mut())
                {
                    cell_info.bg = sel_bg;
                }
            }
        }
    }

    // Render: coalesce adjacent cells with same bg into wide rectangles.
    for (line_idx, row_cells) in grid.iter().enumerate() {
        let row = area_row + line_idx;
        let mut run_start = 0usize;
        let mut run_bg: Option<Color4f> = None;
        let mut run_len = 0usize;

        for (col_idx, cell_opt) in row_cells.iter().enumerate() {
            let bg = cell_opt.as_ref().map(|c| c.bg).unwrap_or(default_bg);

            if run_bg.is_some_and(|rb| color4f_eq(rb, bg)) {
                run_len += 1;
            } else {
                // Flush previous run.
                if run_len > 0 {
                    if let Some(rb) = run_bg {
                        canvas.draw_rect_fill(row, area_col + run_start, run_len, 1, rb);
                    }
                }
                run_start = col_idx;
                run_bg = Some(bg);
                run_len = 1;
            }
        }
        // Flush final run.
        if run_len > 0 {
            if let Some(rb) = run_bg {
                canvas.draw_rect_fill(row, area_col + run_start, run_len, 1, rb);
            }
        }

        // Draw text (non-space characters).
        for (col_idx, cell_opt) in row_cells.iter().enumerate() {
            if let Some(cell) = cell_opt {
                if cell.ch != ' ' {
                    let col = area_col + col_idx;
                    if cell.bold {
                        canvas.draw_text_bold(row, col, &cell.ch.to_string(), cell.fg);
                    } else {
                        canvas.draw_text_at(row, col, &cell.ch.to_string(), cell.fg);
                    }
                }
            }
        }
    }

    // Cursor.
    let cursor_line = cursor_point.line.0 + display_offset;
    if focused && cursor_line >= 0 {
        let crow = area_row + cursor_line as usize;
        let ccol = area_col + cursor_point.column.0;
        if crow < area_row + area_height && ccol < area_col + area_width {
            let cursor_style = editor.theme.style("ui.cursor");
            let cursor_color = theme::color_or(cursor_style.bg, theme::DEFAULT_FG);
            canvas.draw_rect_fill(crow, ccol, 1, 1, cursor_color);
        }
    }
    trace!("render_shell_grid exit");
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
    _default: Color4f,
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
            } else if idx < 16 {
                // ANSI base colors (0-15) → resolve through theme, same as Named.
                let named = index_to_named(idx);
                if let Some(color) = resolve_named_from_theme(named, theme) {
                    color
                } else {
                    named_color_to_skia(named)
                }
            } else if idx < 232 {
                // xterm 6×6×6 color cube (indices 16-231).
                let ci = idx - 16;
                let r = if ci / 36 > 0 { (ci / 36) * 40 + 55 } else { 0 };
                let g = if (ci % 36) / 6 > 0 {
                    ((ci % 36) / 6) * 40 + 55
                } else {
                    0
                };
                let b = if ci % 6 > 0 { (ci % 6) * 40 + 55 } else { 0 };
                Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
            } else {
                // Grayscale ramp (indices 232-255).
                let v = (idx - 232) * 10 + 8;
                Color4f::new(v as f32 / 255.0, v as f32 / 255.0, v as f32 / 255.0, 1.0)
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
        NamedColor::Background => &["bg", "bg0", "base", "base03", "background"],
        _ => return None,
    };
    for key in candidates {
        if let Some(c) = theme.palette.get(*key) {
            return Some(theme::theme_color_to_skia(c));
        }
    }
    // For Background and Black, fall back to the theme's ui.background style.
    // Terminal programs use ANSI "black" as the background color, so it should
    // match the editor background rather than xterm's hardcoded #000000.
    if matches!(
        named,
        NamedColor::Background | NamedColor::Black | NamedColor::DimBlack
    ) {
        if let Some(bg) = theme.style("ui.background").bg {
            return Some(theme::theme_color_to_skia(&bg));
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

/// Map xterm indexed color 0-15 to a NamedColor for theme resolution.
fn index_to_named(idx: u8) -> NamedColor {
    match idx {
        0 => NamedColor::Black,
        1 => NamedColor::Red,
        2 => NamedColor::Green,
        3 => NamedColor::Yellow,
        4 => NamedColor::Blue,
        5 => NamedColor::Magenta,
        6 => NamedColor::Cyan,
        7 => NamedColor::White,
        8 => NamedColor::BrightBlack,
        9 => NamedColor::BrightRed,
        10 => NamedColor::BrightGreen,
        11 => NamedColor::BrightYellow,
        12 => NamedColor::BrightBlue,
        13 => NamedColor::BrightMagenta,
        14 => NamedColor::BrightCyan,
        15 => NamedColor::BrightWhite,
        _ => NamedColor::Foreground,
    }
}

/// Fast equality check for Color4f (skia_safe doesn't derive PartialEq).
fn color4f_eq(a: Color4f, b: Color4f) -> bool {
    a.r == b.r && a.g == b.g && a.b == b.b && a.a == b.a
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

    fn make_test_theme(toml: &str) -> mae_core::Theme {
        mae_core::Theme::from_toml("test", toml).unwrap()
    }

    #[test]
    fn background_resolves_base03_for_solarized() {
        // Solarized-dark uses "base03" as background — verify it's in our candidates.
        let theme = make_test_theme(
            r##"
            [palette]
            base03 = "#002b36"
            [styles]
            "ui.background" = { bg = "base03" }
            "##,
        );
        let color = resolve_named_from_theme(NamedColor::Background, &theme);
        assert!(color.is_some());
        let c = color.unwrap();
        assert!(c.r < 0.01, "expected near-zero red for solarized base03");
        assert!(c.g > 0.1 && c.g < 0.2, "expected ~0.17 green for base03");
    }

    #[test]
    fn black_falls_back_to_ui_background_style() {
        // Theme with no "black"/"bg0"/"base"/"crust" palette key, but has
        // ui.background style — Black should resolve to that bg color.
        let theme = make_test_theme(
            r##"
            [palette]
            mybg = "#282c34"
            [styles]
            "ui.background" = { bg = "mybg" }
            "##,
        );
        let color = resolve_named_from_theme(NamedColor::Black, &theme);
        assert!(color.is_some(), "Black should fall back to ui.background");
        let c = color.unwrap();
        // #282c34 → r=0.157, g=0.173, b=0.204
        assert!(c.r > 0.1 && c.r < 0.2);
    }
}
