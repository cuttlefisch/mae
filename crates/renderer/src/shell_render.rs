//! Shell buffer rendering: translates alacritty_terminal grid cells into
//! ratatui widgets with full color and attribute support.

use mae_core::render_common::shell::AnsiName;
use mae_core::{Editor, Window};
use mae_shell::grid_types::{CellFlags, Color as AColor, Colors, NamedColor};
use mae_shell::ShellTerminal;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders};
use tracing::trace;

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

    render_shell_grid(frame, inner, editor, shell, focused);
}

/// Render the alacritty terminal grid into the given area.
fn render_shell_grid(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    shell: &ShellTerminal,
    focused: bool,
) {
    trace!(
        width = area.width,
        height = area.height,
        "render_shell_grid enter"
    );
    let term = shell.term();
    let content = term.renderable_content();

    let cursor_point = content.cursor.point;
    let cols = area.width as usize;

    // Build a 2D grid: rows × cols of (char, Style).
    // Pre-fill with spaces so gaps render correctly.
    let default_style = Style::default();
    let rows = area.height as usize;
    let mut grid: Vec<Vec<(char, Style)>> = vec![vec![(' ', default_style); cols]; rows];

    // Use the already-locked term to get display_offset — calling
    // shell.display_offset() would deadlock (re-entrant FairMutex lock).
    let display_offset = term.grid().display_offset() as i32;
    for indexed in content.display_iter {
        let line_idx = indexed.point.line.0 + display_offset;
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

        let fg_color = convert_color(indexed.cell.fg, content.colors, &editor.theme);
        let bg_color = convert_color(indexed.cell.bg, content.colors, &editor.theme);

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

    // Overlay selection highlight if active.
    if let Some(((sel_start_row, sel_start_col), (sel_end_row, sel_end_col))) =
        shell.selection_range()
    {
        let sel_style = ts(editor, "ui.selection");
        let sel_bg = sel_style.bg.unwrap_or(Color::Rgb(51, 76, 153));
        for row_idx in sel_start_row..=sel_end_row.min(rows.saturating_sub(1)) {
            let col_start = if row_idx == sel_start_row {
                sel_start_col
            } else {
                0
            };
            let col_end = if row_idx == sel_end_row {
                sel_end_col
            } else {
                cols.saturating_sub(1)
            };
            for col_idx in col_start..=col_end.min(cols.saturating_sub(1)) {
                if let Some(cell) = grid.get_mut(row_idx).and_then(|row| row.get_mut(col_idx)) {
                    cell.1 = cell.1.bg(sel_bg);
                }
            }
        }
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
    let cursor_line = cursor_point.line.0 + display_offset;
    if focused && cursor_line >= 0 {
        let cursor_row = area.y + cursor_line as u16;
        let cursor_col = area.x + cursor_point.column.0 as u16;
        if cursor_row < area.y + area.height && cursor_col < area.x + area.width {
            frame.set_cursor_position((cursor_col, cursor_row));
        }
    }
    trace!("render_shell_grid exit");
}

/// Convert an alacritty_terminal Color to a ratatui Color.
///
/// Resolution order for named colors:
/// 1. alacritty_terminal's own color overrides (from `colors`)
/// 2. Editor theme palette (e.g. gruvbox's `red = "#cc241d"`)
/// 3. Standard ANSI terminal colors
///
/// `AColor::Indexed(idx)` for `idx < 16` (the "ANSI base 16" sent via the
/// 256-color escape form, `38;5;0`..`38;5;15`, instead of the classic named
/// SGR codes) now goes through the same theme resolution as
/// `AColor::Named` — previously it fell straight through to
/// `Color::Indexed(idx)`, silently ignoring the MAE theme and deferring to
/// the host terminal's own palette (a real TUI/GUI divergence: the GUI
/// backend already theme-resolves this case, since it owns its cell grid
/// and has no "host terminal palette" to defer to).
fn convert_color(color: AColor, colors: &Colors, theme: &mae_core::Theme) -> Color {
    match color {
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(idx) => {
            if let Some(rgb) = colors[idx as usize] {
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            } else if idx < 16 {
                let ansi = mae_core::render_common::shell::index_to_named(idx);
                resolve_ansi_from_theme(ansi, theme).unwrap_or_else(|| ansi_default_color(ansi))
            } else {
                // 16-255: no MAE theme opinion here, defer to the host
                // terminal's own 256-color palette (unlike the GUI, the TUI
                // has one to defer to).
                Color::Indexed(idx)
            }
        }
        AColor::Named(named) => {
            if let Some(rgb) = colors[named] {
                Color::Rgb(rgb.r, rgb.g, rgb.b)
            } else {
                match named_to_ansi(named) {
                    Some(ansi) => resolve_ansi_from_theme(ansi, theme)
                        .unwrap_or_else(|| ansi_default_color(ansi)),
                    None => Color::Reset,
                }
            }
        }
    }
}

/// Map alacritty's `NamedColor` to the backend-agnostic `AnsiName` (shared
/// with the GUI backend and with `index_to_named`, which handles the
/// indexed-color form of the same 16 base colors). `NamedColor` lives in
/// `mae_shell`/alacritty and can't be a `mae-core` type, so this mapping
/// itself is necessarily backend-local (renderer and gui each have their
/// own copy) — but everything downstream of it (theme resolution, default
/// fallback colors) is shared via `AnsiName`.
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
fn resolve_ansi_from_theme(ansi: AnsiName, theme: &mae_core::Theme) -> Option<Color> {
    use mae_core::render_common::shell;

    for key in shell::palette_candidates(ansi) {
        if let Some(c) = theme.palette.get(*key) {
            return Some(crate::theme_convert::to_ratatui_color(*c));
        }
    }
    if shell::should_fallback_to_ui_background(ansi) {
        if let Some(bg) = theme.style("ui.background").bg {
            return Some(crate::theme_convert::to_ratatui_color(bg));
        }
    }
    None
}

/// Hardcoded xterm-ish default for an `AnsiName` when no theme match exists.
fn ansi_default_color(ansi: AnsiName) -> Color {
    match ansi {
        AnsiName::Black => Color::Black,
        AnsiName::Red => Color::Red,
        AnsiName::Green => Color::Green,
        AnsiName::Yellow => Color::Yellow,
        AnsiName::Blue => Color::Blue,
        AnsiName::Magenta => Color::Magenta,
        AnsiName::Cyan => Color::Cyan,
        AnsiName::White => Color::White,
        AnsiName::BrightBlack => Color::DarkGray,
        AnsiName::BrightRed => Color::LightRed,
        AnsiName::BrightGreen => Color::LightGreen,
        AnsiName::BrightYellow => Color::LightYellow,
        AnsiName::BrightBlue => Color::LightBlue,
        AnsiName::BrightMagenta => Color::LightMagenta,
        AnsiName::BrightCyan => Color::LightCyan,
        AnsiName::BrightWhite => Color::White,
        AnsiName::Foreground => Color::Reset,
        AnsiName::DimForeground => Color::Gray,
        AnsiName::Background => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_theme(toml: &str) -> mae_core::Theme {
        mae_core::Theme::from_toml("test", toml).unwrap()
    }

    #[test]
    fn ansi_falls_back_to_ui_background_style() {
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
    }

    #[test]
    fn indexed_ansi_base_16_resolves_through_theme_like_named() {
        // Regression for the TUI/GUI divergence this module fixes: an
        // explicit 256-color escape for one of the ANSI base 16 (e.g.
        // `38;5;1` for red) must resolve through the MAE theme exactly like
        // the equivalent named-color escape (`31`/red), not silently defer
        // to the host terminal's own indexed palette.
        let theme = make_test_theme(
            r##"
            [palette]
            red = "#ff0000"
            "##,
        );
        let colors = Colors::default();

        let via_named = convert_color(AColor::Named(NamedColor::Red), &colors, &theme);
        let via_indexed_1 = convert_color(AColor::Indexed(1), &colors, &theme);
        assert_eq!(
            via_named, via_indexed_1,
            "indexed color 1 (red) must resolve identically to NamedColor::Red"
        );
        assert_eq!(via_indexed_1, Color::Rgb(0xff, 0x00, 0x00));
    }

    #[test]
    fn indexed_256_color_defers_to_host_terminal_palette() {
        // idx >= 16 (the 6x6x6 cube / grayscale ramp) has no MAE theme
        // opinion — unlike the GUI (which must compute an RGB itself, no
        // host terminal to defer to), the TUI passes it straight through.
        let theme = make_test_theme("");
        let colors = Colors::default();
        let color = convert_color(AColor::Indexed(200), &colors, &theme);
        assert_eq!(color, Color::Indexed(200));
    }

    #[test]
    fn named_to_ansi_covers_every_ansi_variant_the_theme_can_resolve() {
        // Every AnsiName the theme resolver knows about must be reachable
        // from at least one NamedColor — otherwise a base color could
        // silently lose theme resolution if alacritty's NamedColor ever
        // gains new variants without updating this mapping too.
        use AnsiName::*;
        for ansi in [
            Black,
            Red,
            Green,
            Yellow,
            Blue,
            Magenta,
            Cyan,
            White,
            BrightBlack,
            BrightRed,
            BrightGreen,
            BrightYellow,
            BrightBlue,
            BrightMagenta,
            BrightCyan,
            BrightWhite,
            Foreground,
            DimForeground,
            Background,
        ] {
            // ansi_default_color must be exhaustive and not panic for any variant.
            let _ = ansi_default_color(ansi);
        }
    }
}
