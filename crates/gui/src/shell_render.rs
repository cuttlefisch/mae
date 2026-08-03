//! Shell buffer rendering: translates alacritty_terminal grid cells into
//! Skia drawing calls with full color and attribute support.

use mae_core::render_common::shell::AnsiName;
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
        italic: bool,
        underline: bool,
        strikeout: bool,
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
        // Dim: fade the (post-inverse) foreground rather than adding a new
        // per-attribute draw path — matches the dimming convention already
        // used for the which-key doc color (`popup_render.rs`).
        if flags.contains(CellFlags::DIM) {
            fg_color.a *= 0.6;
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
                italic: false,
                underline: false,
                strikeout: false,
            });
            continue;
        }

        let hidden = flags.contains(CellFlags::HIDDEN);

        grid[line_idx as usize][col_idx] = Some(CellInfo {
            fg: fg_color,
            bg: bg_color,
            ch: if hidden { ' ' } else { indexed.cell.c },
            bold: flags.contains(CellFlags::BOLD),
            // Attributes below were previously dropped entirely by this
            // backend (WS5 finding: TUI's shell renderer already applies
            // all of these via ratatui `Modifier`s; this backend tracked
            // only `bold`). `hidden` already blanks the glyph above, same
            // as the TUI's `Modifier::HIDDEN`.
            italic: flags.contains(CellFlags::ITALIC),
            underline: flags.intersects(CellFlags::ALL_UNDERLINES),
            strikeout: flags.contains(CellFlags::STRIKEOUT),
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

    let (_, cell_h) = canvas.cell_size();

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

        // Draw text: coalesce adjacent cells with same style into text runs.
        {
            let mut run_buf = String::with_capacity(area_width);
            let mut run_start = 0usize;
            let mut run_fg = default_fg;
            let mut run_bold = false;
            let mut run_italic = false;

            for (col_idx, cell_opt) in row_cells.iter().enumerate() {
                let (ch, fg, bold, italic) = if let Some(cell) = cell_opt {
                    (cell.ch, cell.fg, cell.bold, cell.italic)
                } else {
                    (' ', default_fg, false, false)
                };

                let style_match = if run_buf.is_empty() {
                    true
                } else {
                    theme::color4f_eq(fg, run_fg) && bold == run_bold && italic == run_italic
                };

                if ch.is_ascii() && style_match {
                    if run_buf.is_empty() {
                        run_start = col_idx;
                        run_fg = fg;
                        run_bold = bold;
                        run_italic = italic;
                    }
                    run_buf.push(ch);
                } else {
                    // Flush current run.
                    if !run_buf.is_empty() {
                        canvas.draw_text_run(
                            row,
                            area_col + run_start,
                            &run_buf,
                            run_fg,
                            run_bold,
                            run_italic,
                            1.0,
                        );
                        run_buf.clear();
                    }
                    if ch.is_ascii() {
                        run_start = col_idx;
                        run_fg = fg;
                        run_bold = bold;
                        run_italic = italic;
                        run_buf.push(ch);
                    } else if ch != ' ' {
                        // Non-ASCII — per-char fallback.
                        canvas.draw_char(row, area_col + col_idx, ch, fg, bold, italic, 1.0);
                    }
                }
            }
            // Flush final run.
            if !run_buf.is_empty() {
                canvas.draw_text_run(
                    row,
                    area_col + run_start,
                    &run_buf,
                    run_fg,
                    run_bold,
                    run_italic,
                    1.0,
                );
            }
        }

        // Underline / strikethrough: coalesce into runs and draw one line
        // per contiguous, same-color run — previously dropped entirely by
        // this backend (see `CellInfo` comment above).
        let pixel_y = row as f32 * cell_h;
        let underline_cells: Vec<(bool, Color4f)> = row_cells
            .iter()
            .map(|c| {
                c.as_ref()
                    .map(|c| (c.underline, c.fg))
                    .unwrap_or((false, default_fg))
            })
            .collect();
        draw_flag_runs(
            canvas,
            &underline_cells,
            pixel_y,
            area_col,
            |c, y, x, w, fg| c.draw_underline_at_y(y, x, w, fg),
        );
        let strikeout_cells: Vec<(bool, Color4f)> = row_cells
            .iter()
            .map(|c| {
                c.as_ref()
                    .map(|c| (c.strikeout, c.fg))
                    .unwrap_or((false, default_fg))
            })
            .collect();
        draw_flag_runs(
            canvas,
            &strikeout_cells,
            pixel_y,
            area_col,
            |c, y, x, w, fg| c.draw_strikethrough_at_y(y, x, w, fg),
        );
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

/// Coalesce a per-column boolean flag (underline / strikethrough) into
/// contiguous, same-color runs and draw one line per run — the same
/// coalescing shape already used for background fill and text runs above,
/// applied to the two line-decoration attributes.
fn draw_flag_runs(
    canvas: &mut SkiaCanvas,
    cells: &[(bool, Color4f)],
    pixel_y: f32,
    area_col: usize,
    draw_line: impl Fn(&mut SkiaCanvas, f32, usize, usize, Color4f),
) {
    let mut run_start = 0usize;
    let mut run_fg: Option<Color4f> = None;
    let mut run_len = 0usize;

    for (col_idx, &(active, fg)) in cells.iter().enumerate() {
        if active && run_fg.is_some_and(|rf| theme::color4f_eq(rf, fg)) {
            run_len += 1;
        } else {
            if run_len > 0 {
                if let Some(rf) = run_fg {
                    draw_line(canvas, pixel_y, area_col + run_start, run_len, rf);
                }
            }
            if active {
                run_start = col_idx;
                run_fg = Some(fg);
                run_len = 1;
            } else {
                run_fg = None;
                run_len = 0;
            }
        }
    }
    if run_len > 0 {
        if let Some(rf) = run_fg {
            draw_line(canvas, pixel_y, area_col + run_start, run_len, rf);
        }
    }
}

fn rgb_to_color4f(rgb: mae_shell::grid_types::Rgb) -> Color4f {
    Color4f::new(
        rgb.r as f32 / 255.0,
        rgb.g as f32 / 255.0,
        rgb.b as f32 / 255.0,
        1.0,
    )
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
    default_fg: Color4f,
    theme: &mae_core::Theme,
) -> Color4f {
    match color {
        AColor::Spec(rgb) => rgb_to_color4f(rgb),
        AColor::Indexed(idx) => {
            if let Some(rgb) = colors[idx as usize] {
                rgb_to_color4f(rgb)
            } else if idx < 16 {
                // ANSI base colors (0-15) → resolve through theme, same as Named
                // (shared `index_to_named`/`AnsiName` — see TUI's
                // `crates/renderer/src/shell_render.rs::convert_color` for the
                // symmetric implementation).
                let ansi = mae_core::render_common::shell::index_to_named(idx);
                resolve_ansi_from_theme(ansi, theme).unwrap_or_else(|| ansi_default_color(ansi))
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
                rgb_to_color4f(rgb)
            } else {
                match named_to_ansi(named) {
                    Some(ansi) => resolve_ansi_from_theme(ansi, theme)
                        .unwrap_or_else(|| ansi_default_color(ansi)),
                    // No AnsiName equivalent (e.g. a cursor-only NamedColor
                    // variant) — fall back to the caller's default text color
                    // rather than a hardcoded near-white RGB.
                    None => default_fg,
                }
            }
        }
    }
}

/// Map alacritty's `NamedColor` to the backend-agnostic `AnsiName` (shared
/// with the TUI backend and with `index_to_named`, which handles the
/// indexed-color form of the same 16 base colors). `NamedColor` lives in
/// `mae_shell`/alacritty and can't be a `mae-core` type, so this mapping
/// itself is necessarily backend-local — but everything downstream of it
/// (theme resolution, default fallback colors) is shared via `AnsiName`.
fn named_to_ansi(named: NamedColor) -> Option<AnsiName> {
    use AnsiName::*;
    Some(match named {
        NamedColor::Black | NamedColor::DimBlack => Black,
        NamedColor::Red | NamedColor::DimRed => Red,
        NamedColor::Green | NamedColor::DimGreen => Green,
        NamedColor::Yellow | NamedColor::DimYellow => Yellow,
        NamedColor::Blue | NamedColor::DimBlue => Blue,
        NamedColor::Magenta | NamedColor::DimMagenta => Magenta,
        NamedColor::Cyan | NamedColor::DimCyan => Cyan,
        NamedColor::White | NamedColor::DimWhite => White,
        NamedColor::BrightBlack => BrightBlack,
        NamedColor::BrightRed => BrightRed,
        NamedColor::BrightGreen => BrightGreen,
        NamedColor::BrightYellow => BrightYellow,
        NamedColor::BrightBlue => BrightBlue,
        NamedColor::BrightMagenta => BrightMagenta,
        NamedColor::BrightCyan => BrightCyan,
        NamedColor::BrightWhite => BrightWhite,
        NamedColor::Foreground | NamedColor::BrightForeground => Foreground,
        NamedColor::DimForeground => DimForeground,
        NamedColor::Background => Background,
        _ => return None,
    })
}

/// Try to resolve an `AnsiName` via the editor theme palette.
///
/// Themes use different naming conventions (gruvbox: "purple"/"aqua",
/// dracula: "pink"/"cyan", catppuccin: "mauve"/"teal"). We try the
/// canonical ANSI name first, then common aliases.
fn resolve_ansi_from_theme(ansi: AnsiName, theme: &mae_core::Theme) -> Option<Color4f> {
    use mae_core::render_common::shell;

    for key in shell::palette_candidates(ansi) {
        if let Some(c) = theme.palette.get(*key) {
            return Some(theme::theme_color_to_skia(c));
        }
    }
    if shell::should_fallback_to_ui_background(ansi) {
        if let Some(bg) = theme.style("ui.background").bg {
            return Some(theme::theme_color_to_skia(&bg));
        }
    }
    None
}

/// Hardcoded xterm-ish default for an `AnsiName` when no theme match exists.
fn ansi_default_color(ansi: AnsiName) -> Color4f {
    let (r, g, b) = match ansi {
        AnsiName::Black => (0, 0, 0),
        AnsiName::Red => (205, 0, 0),
        AnsiName::Green => (0, 205, 0),
        AnsiName::Yellow => (205, 205, 0),
        AnsiName::Blue => (0, 0, 238),
        AnsiName::Magenta => (205, 0, 205),
        AnsiName::Cyan => (0, 205, 205),
        AnsiName::White => (229, 229, 229),
        AnsiName::BrightBlack => (127, 127, 127),
        AnsiName::BrightRed => (255, 0, 0),
        AnsiName::BrightGreen => (0, 255, 0),
        AnsiName::BrightYellow => (255, 255, 0),
        AnsiName::BrightBlue => (92, 92, 255),
        AnsiName::BrightMagenta => (255, 0, 255),
        AnsiName::BrightCyan => (0, 255, 255),
        AnsiName::BrightWhite => (255, 255, 255),
        AnsiName::Foreground => (229, 229, 229),
        AnsiName::DimForeground => (192, 192, 192),
        AnsiName::Background => (0, 0, 0),
    };
    Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
}

// color4f_eq moved to crate::theme — re-import via `theme::color4f_eq`.
use crate::theme::color4f_eq;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_default_black() {
        let c = ansi_default_color(AnsiName::Black);
        assert!(c.r < 0.01);
        assert!(c.g < 0.01);
        assert!(c.b < 0.01);
    }

    #[test]
    fn ansi_default_bright_white() {
        let c = ansi_default_color(AnsiName::BrightWhite);
        assert!(c.r > 0.99);
    }

    #[test]
    fn ansi_default_red() {
        let c = ansi_default_color(AnsiName::Red);
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
        let color = resolve_ansi_from_theme(AnsiName::Background, &theme);
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
        let color = resolve_ansi_from_theme(AnsiName::Black, &theme);
        assert!(color.is_some(), "Black should fall back to ui.background");
        let c = color.unwrap();
        // #282c34 → r=0.157, g=0.173, b=0.204
        assert!(c.r > 0.1 && c.r < 0.2);
    }

    #[test]
    fn indexed_ansi_base_16_resolves_through_theme_like_named() {
        // Both entry points into the ANSI base 16 (classic named SGR vs.
        // the 256-color indexed form) must agree.
        let theme = make_test_theme(
            r##"
            [palette]
            red = "#ff0000"
            "##,
        );
        let colors = Colors::default();
        let default_fg = Color4f::new(1.0, 1.0, 1.0, 1.0);

        let via_named = convert_color(AColor::Named(NamedColor::Red), &colors, default_fg, &theme);
        let via_indexed_1 = convert_color(AColor::Indexed(1), &colors, default_fg, &theme);
        assert!(theme::color4f_eq(via_named, via_indexed_1));
    }

    #[test]
    fn unmapped_named_color_falls_back_to_caller_default() {
        // A NamedColor with no AnsiName equivalent (Cursor et al.) must use
        // the caller-supplied default text color, not a hardcoded RGB.
        let theme = make_test_theme("");
        let colors = Colors::default();
        let default_fg = Color4f::new(0.25, 0.5, 0.75, 1.0);
        let color = convert_color(
            AColor::Named(NamedColor::Cursor),
            &colors,
            default_fg,
            &theme,
        );
        assert!(theme::color4f_eq(color, default_fg));
    }
}
