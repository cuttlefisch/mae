//! Text buffer rendering: gutter, syntax highlighting, selection, search,
//! cursorline, hex color preview, tilde lines past EOF.

use mae_core::{Editor, HighlightSpan, Mode, Window};
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::gutter;
use crate::text::{StyledCell, StyledLine};
use crate::theme;

/// Render a text buffer's content into a cell region.
/// `area_row/area_col` are the top-left of the text area (after border).
/// `area_width/area_height` are the available cell dimensions.
pub fn render_buffer_content(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    buf: &mae_core::Buffer,
    win: &Window,
    focused: bool,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
    syntax_spans: Option<&[HighlightSpan]>,
) {
    let display_lines = buf.display_line_count();
    let gutter_w = if editor.show_line_numbers {
        gutter::gutter_width(display_lines)
    } else {
        2
    };

    let text_fg = theme::ts_fg(editor, "ui.text");
    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
    let search_style = editor.theme.style("ui.search.match");
    let selection_style = editor.theme.style("ui.selection");
    let cursorline_style = editor.theme.style("ui.cursorline");

    let highlight_search =
        editor.search_state.highlight_active && !editor.search_state.matches.is_empty();
    let highlight_selection = matches!(editor.mode, Mode::Visual(_));
    let (sel_start, sel_end) = if highlight_selection {
        editor.visual_selection_range()
    } else {
        (0, 0)
    };
    let has_syntax = syntax_spans.map(|s| !s.is_empty()).unwrap_or(false);
    let show_cursorline = focused && !highlight_selection && cursorline_style.bg.is_some();
    let needs_spans = highlight_search || highlight_selection || has_syntax || show_cursorline;

    // Collect diagnostic and debug markers.
    let line_severities = gutter::collect_line_severities(buf, editor);
    let (breakpoint_lines, stopped_line) = gutter::collect_breakpoints(buf, editor);
    let stopped_line_fg = theme::ts_fg(editor, "debug.current_line");

    let col_offset = win.col_offset;
    let text_width = area_width.saturating_sub(gutter_w);

    let mut display_row = 0;
    let mut line_idx = win.scroll_offset;

    while display_row < area_height && line_idx < display_lines {
        let line_text = buf.rope().line(line_idx);
        let full_display: String = line_text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();

        let is_cursor_line = focused && line_idx == win.cursor_row;
        let screen_row = area_row + display_row;

        // Gutter.
        gutter::render_gutter_line(
            canvas,
            editor,
            screen_row,
            area_col,
            line_idx,
            gutter_w,
            win.cursor_row,
            is_cursor_line,
            &breakpoint_lines,
            stopped_line,
            &line_severities,
        );

        let text_col = area_col + gutter_w;
        let is_stopped_line = stopped_line == Some(line_idx as u32);

        if needs_spans {
            let line_char_start = buf.rope().line_to_char(line_idx);
            let full_chars: Vec<char> = full_display.chars().collect();
            let full_count = full_chars.len();
            let line_char_end = line_char_start + full_count;

            // Base style: per-char fg/bg.
            let base_fg = if is_stopped_line {
                stopped_line_fg
            } else {
                text_fg
            };
            let mut char_styles: Vec<CharStyle> = vec![
                CharStyle {
                    fg: base_fg,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                };
                full_count
            ];

            // Layer 1: Tree-sitter syntax spans.
            if let Some(spans) = syntax_spans {
                let line_byte_start = buf.rope().char_to_byte(line_char_start);
                let line_byte_end = buf.rope().char_to_byte(line_char_end);
                for span in spans {
                    if span.byte_end <= line_byte_start || span.byte_start >= line_byte_end {
                        continue;
                    }
                    let sb = span.byte_start.max(line_byte_start);
                    let eb = span.byte_end.min(line_byte_end);
                    let sc = buf.rope().byte_to_char(sb).saturating_sub(line_char_start);
                    let ec = buf
                        .rope()
                        .byte_to_char(eb)
                        .saturating_sub(line_char_start)
                        .min(full_count);
                    let ts = editor.theme.style(span.theme_key);
                    let fg = theme::color_or(ts.fg, base_fg);
                    for cs in char_styles[sc..ec].iter_mut() {
                        cs.fg = fg;
                        if ts.bold {
                            cs.bold = true;
                        }
                        if ts.italic {
                            cs.italic = true;
                        }
                        if ts.underline {
                            cs.underline = true;
                        }
                        if let Some(bg) = ts.bg {
                            cs.bg = Some(theme::theme_color_to_skia(&bg));
                        }
                    }
                }
            }

            // Layer 2: Hex color preview.
            apply_hex_color_preview(&full_chars, &mut char_styles);

            // Layer 3: Cursorline bg.
            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    for cs in char_styles.iter_mut() {
                        cs.bg = Some(bg);
                    }
                    // Also fill the text area bg.
                    canvas.draw_rect_fill(screen_row, text_col, text_width, 1, bg);
                }
            }

            // Layer 4: Visual selection.
            if highlight_selection && sel_start < line_char_end && sel_end > line_char_start {
                let s = sel_start.saturating_sub(line_char_start);
                let e = (sel_end - line_char_start).min(full_count);
                let sel_fg = theme::color_or(selection_style.fg, text_fg);
                let sel_bg = selection_style.bg.map(|c| theme::theme_color_to_skia(&c));
                for cs in char_styles[s..e].iter_mut() {
                    cs.fg = sel_fg;
                    if let Some(bg) = sel_bg {
                        cs.bg = Some(bg);
                    }
                }
            }

            // Layer 5: Search highlights (highest priority).
            if highlight_search {
                let search_fg = theme::color_or(search_style.fg, text_fg);
                let search_bg = search_style.bg.map(|c| theme::theme_color_to_skia(&c));
                for m in &editor.search_state.matches {
                    if m.end <= line_char_start || m.start >= line_char_end {
                        continue;
                    }
                    let ms = m.start.saturating_sub(line_char_start);
                    let me = (m.end - line_char_start).min(full_count);
                    for cs in char_styles[ms..me].iter_mut() {
                        cs.fg = search_fg;
                        if let Some(bg) = search_bg {
                            cs.bg = Some(bg);
                        }
                    }
                }
            }

            // Apply horizontal scroll and build styled line.
            let visible_start = col_offset.min(full_count);
            let visible_chars = &full_chars[visible_start..];
            let visible_styles = &char_styles[visible_start..];

            let styled: StyledLine = visible_chars
                .iter()
                .zip(visible_styles)
                .take(text_width)
                .map(|(&ch, cs)| StyledCell {
                    ch,
                    fg: cs.fg,
                    bg: cs.bg,
                    bold: cs.bold,
                    italic: cs.italic,
                    underline: cs.underline,
                })
                .collect();

            draw_styled_at(canvas, screen_row, text_col, &styled);
        } else {
            // Simple rendering: no spans needed.
            let base_fg = if is_stopped_line {
                stopped_line_fg
            } else {
                text_fg
            };
            let display: String = full_display
                .chars()
                .skip(col_offset)
                .take(text_width)
                .collect();
            canvas.draw_text_at(screen_row, text_col, &display, base_fg);

            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_fill(screen_row, text_col, text_width, 1, bg);
                    canvas.draw_text_at(screen_row, text_col, &display, base_fg);
                }
            }
        }

        display_row += 1;
        line_idx += 1;
    }

    // Tilde lines past EOF.
    while display_row < area_height {
        let screen_row = area_row + display_row;
        let padding = " ".repeat(gutter_w.saturating_sub(1));
        canvas.draw_text_at(screen_row, area_col, &format!("{}~", padding), gutter_fg);
        display_row += 1;
    }
}

/// Draw a styled line at an absolute position.
fn draw_styled_at(canvas: &mut SkiaCanvas, row: usize, col: usize, cells: &[StyledCell]) {
    let offset_cells: StyledLine = cells.to_vec();

    // We need to draw at the right position — use draw_styled_line with row offset.
    // But draw_styled_line draws at row with col=0. We need col offset.
    // Use cell-by-cell drawing instead.
    for (i, cell) in offset_cells.iter().enumerate() {
        if let Some(bg) = cell.bg {
            canvas.draw_rect_fill(row, col + i, 1, 1, bg);
        }
        if cell.ch != ' ' || cell.bold || cell.italic || cell.underline {
            if cell.bold {
                canvas.draw_text_bold(row, col + i, &cell.ch.to_string(), cell.fg);
            } else {
                canvas.draw_text_at(row, col + i, &cell.ch.to_string(), cell.fg);
            }
        }
    }
}

// -----------------------------------------------------------------------
// Internal char style type
// -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct CharStyle {
    fg: Color4f,
    bg: Option<Color4f>,
    bold: bool,
    italic: bool,
    underline: bool,
}

// -----------------------------------------------------------------------
// Hex color preview
// -----------------------------------------------------------------------

fn apply_hex_color_preview(chars: &[char], styles: &mut [CharStyle]) {
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i] == '#' {
            // Try #rrggbb (7 chars total)
            if i + 7 <= len && chars[i + 1..i + 7].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 7].iter().collect();
                if let Some((r, g, b)) = parse_hex6(&hex) {
                    let fg = theme::contrast_fg(r, g, b);
                    let bg =
                        Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
                    for s in styles[i..i + 7].iter_mut() {
                        s.fg = fg;
                        s.bg = Some(bg);
                    }
                    i += 7;
                    continue;
                }
            }
            // Try #rgb (4 chars total)
            if i + 4 <= len && chars[i + 1..i + 4].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 4].iter().collect();
                if let Some((r, g, b)) = parse_hex3(&hex) {
                    let fg = theme::contrast_fg(r, g, b);
                    let bg =
                        Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
                    for s in styles[i..i + 4].iter_mut() {
                        s.fg = fg;
                        s.bg = Some(bg);
                    }
                    i += 4;
                    continue;
                }
            }
        }
        i += 1;
    }
}

fn parse_hex6(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn parse_hex3(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 3 {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let r = u8::from_str_radix(&format!("{0}{0}", chars[0]), 16).ok()?;
    let g = u8::from_str_radix(&format!("{0}{0}", chars[1]), 16).ok()?;
    let b = u8::from_str_radix(&format!("{0}{0}", chars[2]), 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex6_parses_correctly() {
        assert_eq!(parse_hex6("ff5733"), Some((255, 87, 51)));
    }

    #[test]
    fn hex3_parses_correctly() {
        assert_eq!(parse_hex3("f00"), Some((255, 0, 0)));
    }

    #[test]
    fn hex6_invalid_returns_none() {
        assert_eq!(parse_hex6("zzzzzz"), None);
    }

    #[test]
    fn hex_color_preview_applies() {
        let chars: Vec<char> = "color #ff5733 here".chars().collect();
        let fg = Color4f::new(0.9, 0.9, 0.9, 1.0);
        let mut styles: Vec<CharStyle> = chars
            .iter()
            .map(|_| CharStyle {
                fg,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            })
            .collect();
        apply_hex_color_preview(&chars, &mut styles);
        assert!(styles[6].bg.is_some()); // '#' position
        assert!(styles[12].bg.is_some()); // last hex digit
        assert!(styles[0].bg.is_none());
        assert!(styles[13].bg.is_none());
    }

    #[test]
    fn hex3_color_preview_applies() {
        let chars: Vec<char> = "#f00".chars().collect();
        let fg = Color4f::new(0.9, 0.9, 0.9, 1.0);
        let mut styles: Vec<CharStyle> = chars
            .iter()
            .map(|_| CharStyle {
                fg,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            })
            .collect();
        apply_hex_color_preview(&chars, &mut styles);
        assert!(styles[0].bg.is_some());
        assert!(styles[3].bg.is_some());
    }

    #[test]
    fn hex_no_false_positive() {
        let chars: Vec<char> = "#zzzzzz".chars().collect();
        let fg = Color4f::new(0.9, 0.9, 0.9, 1.0);
        let mut styles: Vec<CharStyle> = chars
            .iter()
            .map(|_| CharStyle {
                fg,
                bg: None,
                bold: false,
                italic: false,
                underline: false,
            })
            .collect();
        apply_hex_color_preview(&chars, &mut styles);
        assert!(styles.iter().all(|s| s.bg.is_none()));
    }

    #[test]
    fn char_style_default() {
        let cs = CharStyle {
            fg: Color4f::new(1.0, 1.0, 1.0, 1.0),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        };
        assert!(!cs.bold);
        assert!(cs.bg.is_none());
    }
}
