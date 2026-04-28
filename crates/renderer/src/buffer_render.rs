//! Text buffer rendering: gutter, syntax spans, hex color preview,
//! search/selection highlights, cursorline, diagnostics, breakpoints.

use mae_core::render_common::gutter::{
    self as gutter_common, collect_breakpoints, collect_line_severities, gutter_width,
};
use mae_core::wrap::{find_wrap_break, leading_indent_len};
use mae_core::{Editor, HighlightSpan, Mode, Window};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

// ---------------------------------------------------------------------------
// Text buffer window (border + inner)
// ---------------------------------------------------------------------------

pub(crate) fn render_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    win: &Window,
    focused: bool,
    editor: &Editor,
    syntax_spans: Option<&[HighlightSpan]>,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let modified = if buf.modified { " [+]" } else { "" };
    let title = format!(" {}{} ", buf.name, modified);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    render_buffer(frame, inner, buf, win, focused, editor, syntax_spans);
}

// ---------------------------------------------------------------------------
// Buffer content rendering
// ---------------------------------------------------------------------------

pub(crate) fn render_buffer(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    win: &Window,
    focused: bool,
    editor: &Editor,
    syntax_spans: Option<&[HighlightSpan]>,
) {
    let viewport_height = area.height as usize;
    let display_lines = buf.display_line_count();
    let gutter_w = if editor.show_line_numbers {
        gutter_width(display_lines)
    } else {
        2 // marker column + 1 padding
    };
    let gutter_style = ts(editor, "ui.gutter");
    let text_style = ts(editor, "ui.text");
    let search_style = ts(editor, "ui.search.match");
    let selection_style = ts(editor, "ui.selection");
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
    // Cursorline: subtle background on the cursor's line (Emacs hl-line-mode).
    // Only in the focused window, and not in visual mode (selection is enough).
    let cursorline_style = ts(editor, "ui.cursorline");
    let show_cursorline = focused && !highlight_selection && cursorline_style.bg.is_some();
    let needs_spans = highlight_search || highlight_selection || has_syntax || show_cursorline;

    // Per-line diagnostic severities + breakpoints from shared gutter logic.
    let line_severities = collect_line_severities(buf, editor);
    let (breakpoint_lines, stopped_line) = collect_breakpoints(buf, editor);
    let stopped_line_style = ts(editor, "debug.current_line");

    let mut lines = Vec::with_capacity(viewport_height);

    let col_offset = win.col_offset;
    let text_width = (area.width as usize).saturating_sub(gutter_w);
    let wrap = editor.word_wrap && text_width > 0;
    let show_break_width = if wrap {
        editor.show_break.chars().count()
    } else {
        0
    };

    let mut display_row = 0;
    let mut line_idx = win.scroll_offset;

    while display_row < viewport_height && line_idx < display_lines {
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
        let full_display: String = line_text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();

        let line_num = gutter_common::format_line_number(
            line_idx,
            win.cursor_row,
            gutter_w,
            editor.show_line_numbers,
            editor.relative_line_numbers,
        );
        let line_idx_u32 = line_idx as u32;
        let marker = gutter_common::resolve_gutter_marker(
            stopped_line == Some(line_idx_u32),
            breakpoint_lines.contains(&line_idx_u32),
            line_severities.get(&line_idx_u32).copied(),
        );
        let (marker_char, marker_style) = match marker.glyph_and_theme_key() {
            Some((ch, key)) => (ch, ts(editor, key)),
            None => (' ', gutter_style),
        };
        let line_text_style = if stopped_line == Some(line_idx_u32) {
            stopped_line_style
        } else {
            text_style
        };

        if needs_spans {
            let line_char_start = buf.rope().line_to_char(line_idx);
            let full_chars: Vec<char> = full_display.chars().collect();
            let full_count = full_chars.len();
            let line_char_end = line_char_start + full_count;

            let mut styles: Vec<Style> = vec![line_text_style; full_count];

            // Apply tree-sitter syntax highlights (lowest priority).
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

                    if editor.org_hide_emphasis_markers
                        && (span.theme_key == "markup.bold.marker"
                            || span.theme_key == "markup.italic.marker")
                    {
                        // Hide these by setting them to the background color or empty style.
                        // Actually, just skip patching the style.
                        continue;
                    }

                    let style = ts(editor, span.theme_key);
                    for s in styles[sc..ec].iter_mut() {
                        *s = s.patch(style);
                    }
                }
            }

            // Inline hex color preview.
            apply_hex_color_preview(&full_chars, &mut styles);

            // Cursorline: apply bg to every cell on the cursor row.
            if show_cursorline && line_idx == win.cursor_row {
                if let Some(bg) = cursorline_style.bg {
                    for s in styles.iter_mut() {
                        *s = s.patch(Style::default().bg(bg));
                    }
                }
            }

            // Apply selection highlight (overrides syntax).
            if let Some((br_min, br_max, bc_min, bc_max)) = block_rect {
                if line_idx >= br_min && line_idx <= br_max {
                    let s = bc_min.min(full_count);
                    let e = (bc_max + 1).min(full_count);
                    for style in styles[s..e].iter_mut() {
                        *style = selection_style;
                    }
                }
            } else if highlight_selection && sel_start < line_char_end && sel_end > line_char_start
            {
                let s = sel_start.saturating_sub(line_char_start);
                let e = (sel_end - line_char_start).min(full_count);
                for style in styles[s..e].iter_mut() {
                    *style = selection_style;
                }
            }

            // Apply search highlights (highest priority).
            if highlight_search {
                for m in &editor.search_state.matches {
                    if m.end <= line_char_start || m.start >= line_char_end {
                        continue;
                    }
                    let ms = m.start.saturating_sub(line_char_start);
                    let me = (m.end - line_char_start).min(full_count);
                    for style in styles[ms..me].iter_mut() {
                        *style = search_style;
                    }
                }
            }

            // Gutter spans — cursorline bg on gutter cells too.
            let gutter_line_style = if show_cursorline && line_idx == win.cursor_row {
                if let Some(bg) = cursorline_style.bg {
                    gutter_style.patch(Style::default().bg(bg))
                } else {
                    gutter_style
                }
            } else {
                gutter_style
            };

            if wrap {
                // Word wrap with word-boundary breaks + breakindent.
                let indent_len = if editor.break_indent {
                    leading_indent_len(&full_chars)
                } else {
                    0
                };
                // Width available for continuation text (after gutter + indent + showbreak).
                let cont_prefix_w = indent_len + show_break_width;
                let cont_text_w = if text_width > cont_prefix_w {
                    text_width - cont_prefix_w
                } else {
                    text_width
                };

                let mut pos = 0;
                let mut is_first = true;
                loop {
                    if display_row >= viewport_height {
                        break;
                    }

                    let avail = if is_first { text_width } else { cont_text_w };
                    let end = find_wrap_break(&full_chars, pos, avail);
                    let chunk_chars = &full_chars[pos..end];
                    let chunk_styles = &styles[pos..end];

                    let mut spans: Vec<Span> = Vec::new();
                    if is_first {
                        spans.push(Span::styled(line_num.clone(), gutter_line_style));
                        spans.push(Span::styled(marker_char.to_string(), marker_style));
                    } else {
                        // Gutter: blank line number area.
                        let gutter_pad = " ".repeat(gutter_w);
                        spans.push(Span::styled(gutter_pad, gutter_line_style));
                        // Breakindent + showbreak prefix.
                        if indent_len > 0 {
                            spans.push(Span::styled(" ".repeat(indent_len), gutter_line_style));
                        }
                        if !editor.show_break.is_empty() {
                            spans.push(Span::styled(editor.show_break.clone(), gutter_line_style));
                        }
                    }

                    if !chunk_chars.is_empty() {
                        emit_styled_spans(chunk_chars, chunk_styles, &mut spans);
                    }

                    lines.push(Line::from(spans));
                    display_row += 1;
                    is_first = false;
                    pos = end;
                    if pos >= full_count {
                        break;
                    }
                }
                // Empty line still needs one display row.
                if is_first {
                    lines.push(Line::from(vec![
                        Span::styled(line_num, gutter_line_style),
                        Span::styled(marker_char.to_string(), marker_style),
                    ]));
                    display_row += 1;
                }
            } else {
                // No wrap: apply horizontal scroll.
                let visible_start = col_offset.min(full_count);
                let display_chars = &full_chars[visible_start..];
                let visible_styles = &styles[visible_start..];

                let mut spans = vec![
                    Span::styled(line_num, gutter_line_style),
                    Span::styled(marker_char.to_string(), marker_style),
                ];
                if !display_chars.is_empty() {
                    emit_styled_spans(display_chars, visible_styles, &mut spans);
                }

                lines.push(Line::from(spans));
                display_row += 1;
            }
        } else if wrap {
            // Word wrap without syntax spans.
            let full_chars: Vec<char> = full_display.chars().collect();
            let full_count = full_chars.len();
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
                if display_row >= viewport_height {
                    break;
                }
                let avail = if is_first { text_width } else { cont_text_w };
                let end = find_wrap_break(&full_chars, pos, avail);
                let chunk: String = full_chars[pos..end].iter().collect();
                let mut spans: Vec<Span> = Vec::new();
                if is_first {
                    spans.push(Span::styled(line_num.clone(), gutter_style));
                    spans.push(Span::styled(marker_char.to_string(), marker_style));
                } else {
                    let gutter_pad = " ".repeat(gutter_w);
                    spans.push(Span::styled(gutter_pad, gutter_style));
                    if indent_len > 0 {
                        spans.push(Span::styled(" ".repeat(indent_len), gutter_style));
                    }
                    if !editor.show_break.is_empty() {
                        spans.push(Span::styled(editor.show_break.clone(), gutter_style));
                    }
                }
                spans.push(Span::styled(chunk, line_text_style));
                lines.push(Line::from(spans));
                display_row += 1;
                is_first = false;
                pos = end;
                if pos >= full_count {
                    break;
                }
            }
            if is_first {
                lines.push(Line::from(vec![
                    Span::styled(line_num, gutter_style),
                    Span::styled(marker_char.to_string(), marker_style),
                ]));
                display_row += 1;
            }
        } else {
            // No wrap, no spans: apply horizontal scroll to simple lines.
            let display: String = full_display.chars().skip(col_offset).collect();
            lines.push(Line::from(vec![
                Span::styled(line_num, gutter_style),
                Span::styled(marker_char.to_string(), marker_style),
                Span::styled(display, line_text_style),
            ]));
            display_row += 1;
        }

        line_idx += 1;
    }

    // Fill remaining viewport with ~ lines.
    while display_row < viewport_height {
        let padding = " ".repeat(gutter_w.saturating_sub(1));
        lines.push(Line::from(vec![Span::styled(
            format!("{}~", padding),
            gutter_style,
        )]));
        display_row += 1;
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------

/// Coalesce consecutive chars with the same style into `Span`s and append to `out`.
fn emit_styled_spans(chars: &[char], styles: &[Style], out: &mut Vec<Span<'static>>) {
    if chars.is_empty() {
        return;
    }
    let mut run_start = 0;
    let mut run_style = styles[0];
    for j in 1..chars.len() {
        if styles[j] != run_style {
            let s: String = chars[run_start..j].iter().collect();
            out.push(Span::styled(s, run_style));
            run_start = j;
            run_style = styles[j];
        }
    }
    let s: String = chars[run_start..].iter().collect();
    out.push(Span::styled(s, run_style));
}

// ---------------------------------------------------------------------------
// Hex color preview
// ---------------------------------------------------------------------------

/// Detect `#rrggbb` and `#rgb` hex color strings in a line and set
/// their background to the parsed color. Foreground auto-adjusts to
/// black or white for readability (relative luminance threshold).
pub(crate) fn apply_hex_color_preview(chars: &[char], styles: &mut [Style]) {
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if chars[i] == '#' {
            // Try #rrggbb (7 chars total)
            if i + 7 <= len && chars[i + 1..i + 7].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 7].iter().collect();
                if let Some((r, g, b)) = parse_hex6(&hex) {
                    let fg = contrast_fg(r, g, b);
                    let bg = Color::Rgb(r, g, b);
                    for s in styles[i..i + 7].iter_mut() {
                        *s = Style::default().fg(fg).bg(bg);
                    }
                    i += 7;
                    continue;
                }
            }
            // Try #rgb (4 chars total)
            if i + 4 <= len && chars[i + 1..i + 4].iter().all(|c| c.is_ascii_hexdigit()) {
                let hex: String = chars[i + 1..i + 4].iter().collect();
                if let Some((r, g, b)) = parse_hex3(&hex) {
                    let fg = contrast_fg(r, g, b);
                    let bg = Color::Rgb(r, g, b);
                    for s in styles[i..i + 4].iter_mut() {
                        *s = Style::default().fg(fg).bg(bg);
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

/// Pick black or white foreground for readability on the given bg color.
fn contrast_fg(r: u8, g: u8, b: u8) -> Color {
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    if lum > 128.0 {
        Color::Black
    } else {
        Color::White
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Hex color preview ---

    #[test]
    fn hex6_color_sets_bg() {
        let chars: Vec<char> = "color #ff5733 here".chars().collect();
        let mut styles = vec![Style::default(); chars.len()];
        apply_hex_color_preview(&chars, &mut styles);
        assert_eq!(styles[6].bg, Some(Color::Rgb(0xff, 0x57, 0x33)));
        assert_eq!(styles[12].bg, Some(Color::Rgb(0xff, 0x57, 0x33)));
        assert_eq!(styles[0].bg, None);
        assert_eq!(styles[13].bg, None);
    }

    #[test]
    fn hex3_color_sets_bg() {
        let chars: Vec<char> = "#f00".chars().collect();
        let mut styles = vec![Style::default(); chars.len()];
        apply_hex_color_preview(&chars, &mut styles);
        assert_eq!(styles[0].bg, Some(Color::Rgb(0xff, 0x00, 0x00)));
        assert_eq!(styles[3].bg, Some(Color::Rgb(0xff, 0x00, 0x00)));
    }

    #[test]
    fn hex_color_contrast_fg_light_bg_gets_black() {
        assert_eq!(contrast_fg(255, 255, 255), Color::Black);
    }

    #[test]
    fn hex_color_contrast_fg_dark_bg_gets_white() {
        assert_eq!(contrast_fg(0, 0, 0), Color::White);
    }

    #[test]
    fn hex_color_no_false_positive_on_non_hex() {
        let chars: Vec<char> = "#zzzzzz".chars().collect();
        let mut styles = vec![Style::default(); chars.len()];
        apply_hex_color_preview(&chars, &mut styles);
        assert!(styles.iter().all(|s| s.bg.is_none()));
    }

    #[test]
    fn hex_color_multiple_on_same_line() {
        let chars: Vec<char> = "#000000 #ffffff".chars().collect();
        let mut styles = vec![Style::default(); chars.len()];
        apply_hex_color_preview(&chars, &mut styles);
        assert_eq!(styles[0].bg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(styles[8].bg, Some(Color::Rgb(255, 255, 255)));
    }
}
