//! Text buffer rendering: gutter, syntax highlighting, selection, search,
//! cursorline, hex color preview, tilde lines past EOF.

use mae_core::wrap::{char_width, find_wrap_break, leading_indent_len};
use mae_core::{Editor, HighlightSpan, Mode, Window};
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::gutter;
use crate::theme;
use crate::theme::color4f_eq;

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
    let is_block_visual = matches!(editor.mode, Mode::Visual(mae_core::VisualType::Block));
    let (sel_start, sel_end) = if highlight_selection && !is_block_visual {
        editor.visual_selection_range()
    } else {
        (0, 0)
    };
    let block_rect = if is_block_visual {
        Some(editor.block_selection_rect())
    } else {
        None
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

    let wrap = editor.word_wrap && text_width > 0;
    let show_break_width = if wrap {
        unicode_width::UnicodeWidthStr::width(editor.show_break.as_str())
    } else {
        0
    };

    let mut display_row = 0;
    let mut line_idx = win.scroll_offset;
    // Hoisted allocations — reused across lines to avoid ~160 allocs/frame.
    let mut full_chars: Vec<char> = Vec::with_capacity(256);
    let mut char_styles: Vec<CharStyle> = Vec::with_capacity(256);

    while display_row < area_height && line_idx < display_lines {
        // Skip folded lines
        let mut is_folded = false;
        for (start, end) in &buf.folded_ranges {
            if line_idx > *start && line_idx < *end {
                is_folded = true;
                break;
            }
        }
        if is_folded {
            line_idx += 1;
            continue;
        }

        let line_text = buf.rope().line(line_idx);
        // Reuse hoisted vec — collect chars directly, skip intermediate String.
        full_chars.clear();
        full_chars.extend(line_text.chars().filter(|c| *c != '\n' && *c != '\r'));

        let is_cursor_line = focused && line_idx == win.cursor_row;
        let is_stopped_line = stopped_line == Some(line_idx as u32);
        let text_col = area_col + gutter_w;

        let full_count = full_chars.len();
        // Cache rope boundaries once per line (used by org heading check + syntax spans).
        let line_char_start = buf.rope().line_to_char(line_idx);
        let line_char_end = line_char_start + full_count;
        let line_byte_start = buf.rope().char_to_byte(line_char_start);

        let is_org_heading = needs_spans
            && syntax_spans
                .and_then(|spans| {
                    spans.iter().find(|s| {
                        s.byte_start == line_byte_start && s.theme_key == "markup.heading"
                    })
                })
                .is_some();

        let base_fg = if is_stopped_line {
            stopped_line_fg
        } else {
            text_fg
        };

        // Reuse hoisted style vec.
        char_styles.clear();
        let init_bg = if !needs_spans && show_cursorline && is_cursor_line {
            cursorline_style.bg.map(|c| theme::theme_color_to_skia(&c))
        } else {
            None
        };
        char_styles.resize(
            full_count,
            CharStyle {
                fg: base_fg,
                bg: init_bg,
                bold: false,
                italic: false,
                underline: false,
            },
        );

        if needs_spans {
            // Layer 1: Tree-sitter syntax spans.
            if let Some(spans) = syntax_spans {
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

                    if editor.org_hide_emphasis_markers
                        && (span.theme_key == "markup.bold.marker"
                            || span.theme_key == "markup.italic.marker")
                    {
                        continue;
                    }

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
                }
            }

            // Layer 4: Visual selection.
            if let Some((br_min, br_max, bc_min, bc_max)) = block_rect {
                // Block visual: highlight column range on rows within the rectangle.
                if line_idx >= br_min && line_idx <= br_max {
                    let s = bc_min.min(full_count);
                    let e = (bc_max + 1).min(full_count);
                    let sel_fg = theme::color_or(selection_style.fg, text_fg);
                    let sel_bg = selection_style.bg.map(|c| theme::theme_color_to_skia(&c));
                    for cs in char_styles[s..e].iter_mut() {
                        cs.fg = sel_fg;
                        if let Some(bg) = sel_bg {
                            cs.bg = Some(bg);
                        }
                    }
                }
            } else if highlight_selection && sel_start < line_char_end && sel_end > line_char_start
            {
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
        }

        if wrap {
            let indent_len = if editor.break_indent {
                leading_indent_len(&full_chars)
            } else {
                0
            };
            let cont_prefix_w = indent_len + show_break_width;
            let cont_text_w = if text_width > cont_prefix_w {
                text_width - cont_prefix_w
            } else {
                text_width
            };

            let mut pos = 0;
            let mut is_first = true;
            loop {
                if display_row >= area_height {
                    break;
                }
                let screen_row = area_row + display_row;

                if is_first {
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
                } else {
                    // Continuation line gutter.
                    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
                    let padding = " ".repeat(gutter_w);
                    canvas.draw_text_at(screen_row, area_col, &padding, gutter_fg);
                }

                let avail = if is_first { text_width } else { cont_text_w };
                let end = find_wrap_break(&full_chars, pos, avail);
                let chunk_chars = &full_chars[pos..end];
                let chunk_styles = &char_styles[pos..end];

                // If cursorline is active, fill the background for the whole line.
                if show_cursorline && is_cursor_line {
                    if let Some(bg_tc) = cursorline_style.bg {
                        let bg = theme::theme_color_to_skia(&bg_tc);
                        canvas.draw_rect_fill(screen_row, text_col, text_width, 1, bg);
                    }
                }

                let mut current_col = text_col;
                if !is_first {
                    // Indent + showbreak prefix
                    let prefix_fg = theme::ts_fg(editor, "ui.gutter");
                    if indent_len > 0 {
                        let indent = " ".repeat(indent_len);
                        canvas.draw_text_at(screen_row, current_col, &indent, prefix_fg);
                        current_col += indent_len;
                    }
                    if !editor.show_break.is_empty() {
                        canvas.draw_text_at(screen_row, current_col, &editor.show_break, prefix_fg);
                        current_col += show_break_width;
                    }
                }

                let scale = if is_org_heading { 1.5 } else { 1.0 };
                draw_styled_at(
                    canvas,
                    screen_row,
                    current_col,
                    chunk_chars,
                    chunk_styles,
                    scale,
                );

                display_row += 1;
                if is_org_heading {
                    display_row += 1; // Org headings take 2 rows for height.
                }
                is_first = false;
                pos = end;
                if pos >= full_count {
                    if full_count == 0 {
                        // Empty line still takes one row
                    }
                    break;
                }
            }
        } else {
            // No wrap.
            let screen_row = area_row + display_row;
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

            // Cursorline full-width background.
            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_fill(screen_row, text_col, text_width, 1, bg);
                }
            }

            let visible_start = col_offset.min(full_count);
            // Walk from visible_start accumulating display width to find visible_end.
            let mut vis_width = 0;
            let mut visible_end = visible_start;
            for &ch in &full_chars[visible_start..] {
                let w = char_width(ch);
                if vis_width + w > text_width {
                    break;
                }
                vis_width += w;
                visible_end += 1;
            }
            let visible_chars = &full_chars[visible_start..visible_end];
            let visible_styles = &char_styles[visible_start..visible_end];

            let scale = if is_org_heading { 1.5 } else { 1.0 };
            draw_styled_at(
                canvas,
                screen_row,
                text_col,
                visible_chars,
                visible_styles,
                scale,
            );
            display_row += 1;
            if is_org_heading {
                display_row += 1;
            }
        }

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

/// Draw styled text at an absolute position using 3-pass run batching.
///
/// Pass 1: coalesce adjacent cells with the same bg into single rects.
/// Pass 2: accumulate text into runs with uniform style, flush via `draw_text_run`.
/// Pass 3: coalesce adjacent underlined cells into single line spans.
///
/// Reduces ~80 Skia calls per line to ~3-5 (one per style run).
fn draw_styled_at(
    canvas: &mut SkiaCanvas,
    row: usize,
    col: usize,
    chars: &[char],
    styles: &[CharStyle],
    scale: f32,
) {
    if chars.is_empty() {
        return;
    }
    let ascii_ok = *canvas.ascii_in_font();

    // Pre-compute cumulative display column for each char (CJK = 2 cols).
    let mut col_offsets = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0usize;
    for &ch in chars {
        col_offsets.push(acc);
        acc += char_width(ch);
    }
    col_offsets.push(acc); // sentinel for total width

    // Pass 1: Coalesce background rects.
    {
        let mut run_start = 0;
        let mut run_bg: Option<Color4f> = styles[0].bg;
        let mut i = 1;
        while i <= styles.len() {
            let this_bg = if i < styles.len() { styles[i].bg } else { None };
            let same = match (run_bg, this_bg) {
                (Some(a), Some(b)) => color4f_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same || i == styles.len() {
                if let Some(bg) = run_bg {
                    let start_col = col_offsets[run_start];
                    let width = col_offsets[i] - start_col;
                    canvas.draw_rect_fill(row, col + start_col, width, 1, bg);
                }
                run_start = i;
                run_bg = this_bg;
            }
            i += 1;
        }
    }

    // Pass 2: Text runs — batch chars with uniform style that are in the primary font.
    {
        let mut run_buf = String::with_capacity(128);
        let mut run_start_col = 0usize;
        let mut run_fg = styles[0].fg;
        let mut run_bold = styles[0].bold;
        let mut run_italic = styles[0].italic;

        for (i, (&ch, cs)) in chars.iter().zip(styles).enumerate() {
            let in_font = ch.is_ascii() && ascii_ok[ch as usize];
            let can_batch = in_font || ch == ' ';

            let style_match = if run_buf.is_empty() {
                true
            } else {
                color4f_eq(cs.fg, run_fg) && cs.bold == run_bold && cs.italic == run_italic
            };

            if can_batch && style_match {
                if run_buf.is_empty() {
                    run_start_col = col_offsets[i];
                    run_fg = cs.fg;
                    run_bold = cs.bold;
                    run_italic = cs.italic;
                }
                run_buf.push(ch);
            } else {
                // Flush current run.
                if !run_buf.is_empty() {
                    canvas.draw_text_run(
                        row,
                        col + run_start_col,
                        &run_buf,
                        run_fg,
                        run_bold,
                        run_italic,
                        scale,
                    );
                    run_buf.clear();
                }

                if can_batch {
                    // New style, batchable char — start new run.
                    run_start_col = col_offsets[i];
                    run_fg = cs.fg;
                    run_bold = cs.bold;
                    run_italic = cs.italic;
                    run_buf.push(ch);
                } else if ch != ' ' {
                    // Non-ASCII / missing glyph — per-char fallback.
                    canvas.draw_char(
                        row,
                        col + col_offsets[i],
                        ch,
                        cs.fg,
                        cs.bold,
                        cs.italic,
                        scale,
                    );
                }
            }
        }
        if !run_buf.is_empty() {
            canvas.draw_text_run(
                row,
                col + run_start_col,
                &run_buf,
                run_fg,
                run_bold,
                run_italic,
                scale,
            );
        }
    }

    // Pass 3: Coalesce underline spans.
    {
        let mut ul_start: Option<(usize, usize, Color4f)> = None; // (char_idx, col_offset, fg)
        for (i, cs) in styles.iter().enumerate() {
            if cs.underline {
                if ul_start.is_none() {
                    ul_start = Some((i, col_offsets[i], cs.fg));
                }
            } else if let Some((_, start_col, fg)) = ul_start.take() {
                let width = col_offsets[i] - start_col;
                canvas.draw_underline_span(row, col + start_col, width, fg);
            }
        }
        if let Some((_, start_col, fg)) = ul_start {
            let width = col_offsets[styles.len()] - start_col;
            canvas.draw_underline_span(row, col + start_col, width, fg);
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
