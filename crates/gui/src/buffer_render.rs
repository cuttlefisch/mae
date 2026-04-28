//! Text buffer rendering: gutter, syntax highlighting, selection, search,
//! cursorline, hex color preview, tilde lines past EOF.

use mae_core::wrap::{char_width, find_wrap_break, leading_indent_len};
use mae_core::{Editor, HighlightSpan, Mode, Window};
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::gutter;
use crate::theme;
use crate::theme::color4f_eq;

/// Maps logical display rows to pixel Y positions.
///
/// The buffer renderer populates this as it processes lines. Cursor and popup
/// rendering use it to convert row-based positions to exact pixel coordinates.
pub struct PixelYMap {
    /// pixel Y for each display row, indexed by `display_row`.
    entries: Vec<f32>,
    /// The absolute row of the first entry (area_row).
    base_row: usize,
    /// Fallback cell height for rows not in the map.
    cell_height: f32,
}

impl PixelYMap {
    /// Look up the pixel Y for a given absolute screen row.
    pub fn pixel_y_for_row(&self, abs_row: usize) -> f32 {
        let idx = abs_row.saturating_sub(self.base_row);
        self.entries
            .get(idx)
            .copied()
            .unwrap_or(abs_row as f32 * self.cell_height)
    }

    /// Look up the line height (in pixels) for a given absolute screen row.
    #[allow(dead_code)]
    pub fn line_height_for_row(&self, abs_row: usize) -> f32 {
        let idx = abs_row.saturating_sub(self.base_row);
        if let Some(&y) = self.entries.get(idx) {
            // Height = next row's Y - this row's Y
            if let Some(&next_y) = self.entries.get(idx + 1) {
                next_y - y
            } else {
                self.cell_height
            }
        } else {
            self.cell_height
        }
    }
}

/// Compute the font scale for an org heading level.
/// `*` = 1.5x, `**` = 1.3x, `***` = 1.15x, deeper = 1.0x.
pub fn org_heading_scale_for_level(level: u8) -> f32 {
    match level {
        1 => 1.5,
        2 => 1.3,
        3 => 1.15,
        _ => 1.0,
    }
}

/// Compute the number of extra display rows a scaled heading consumes.
fn extra_rows_for_scale(scale: f32) -> usize {
    if scale > 1.0 {
        (scale - 1.0 + 0.5).ceil() as usize
    } else {
        0
    }
}

/// Get the heading scale for a single line. Returns 1.0 if not a heading.
pub fn line_heading_scale(
    buf: &mae_core::Buffer,
    syntax_spans: Option<&[HighlightSpan]>,
    line_idx: usize,
) -> f32 {
    let spans = match syntax_spans {
        Some(s) if !s.is_empty() => s,
        _ => return 1.0,
    };
    let rope = buf.rope();
    if line_idx >= buf.line_count() {
        return 1.0;
    }
    let line_char_start = rope.line_to_char(line_idx);
    let line_len = rope.line(line_idx).len_chars();
    let text_len = if line_idx + 1 < buf.line_count() {
        line_len.saturating_sub(1)
    } else {
        line_len
    };
    let line_byte_start = rope.char_to_byte(line_char_start);
    let line_byte_end = rope.char_to_byte(line_char_start + text_len);

    let start_idx = spans.partition_point(|s| s.byte_end <= line_byte_start);
    let has_heading = spans[start_idx..]
        .iter()
        .take_while(|s| s.byte_start < line_byte_end)
        .any(|s| s.theme_key == "markup.heading" && s.byte_start >= line_byte_start);
    if has_heading {
        let level = rope
            .line(line_idx)
            .chars()
            .take_while(|&c| c == '*')
            .count()
            .min(255) as u8;
        org_heading_scale_for_level(level)
    } else {
        1.0
    }
}

/// Count extra display rows consumed by scaled org headings in a range of lines.
/// Used by cursor positioning and popup rendering to offset screen coordinates.
pub fn heading_extra_rows(
    buf: &mae_core::Buffer,
    syntax_spans: Option<&[HighlightSpan]>,
    from_line: usize,
    to_line: usize,
) -> usize {
    let spans = match syntax_spans {
        Some(s) if !s.is_empty() => s,
        _ => return 0,
    };
    let rope = buf.rope();
    let line_count = buf.line_count();
    let mut extra = 0;
    for ln in from_line..to_line.min(line_count) {
        let line_char_start = rope.line_to_char(ln);
        let line_len = rope.line(ln).len_chars();
        // Exclude trailing newline from char count for byte range.
        let text_len = if ln + 1 < line_count {
            line_len.saturating_sub(1)
        } else {
            line_len
        };
        let line_byte_start = rope.char_to_byte(line_char_start);
        let line_byte_end = rope.char_to_byte(line_char_start + text_len);

        let start_idx = spans.partition_point(|s| s.byte_end <= line_byte_start);
        let has_heading = spans[start_idx..]
            .iter()
            .take_while(|s| s.byte_start < line_byte_end)
            .any(|s| s.theme_key == "markup.heading" && s.byte_start >= line_byte_start);
        if has_heading {
            // Count leading stars.
            let level = rope
                .line(ln)
                .chars()
                .take_while(|&c| c == '*')
                .count()
                .min(255) as u8;
            let scale = org_heading_scale_for_level(level);
            extra += extra_rows_for_scale(scale);
        }
    }
    extra
}

/// Render a text buffer's content into a cell region.
/// `area_row/area_col` are the top-left of the text area (after border).
/// `area_width/area_height` are the available cell dimensions.
///
/// Returns a `PixelYMap` mapping display rows to pixel Y positions,
/// used by cursor and popup rendering for pixel-accurate positioning.
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
) -> PixelYMap {
    let (_, cell_height) = canvas.cell_size();

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

    // Pixel Y accumulator — exact positioning for variable-height lines.
    let mut pixel_y = area_row as f32 * cell_height;
    let pixel_y_limit = (area_row + area_height) as f32 * cell_height;

    // PixelYMap: record pixel_y for each display_row so cursor/popup can look it up.
    let mut y_map_entries: Vec<f32> = Vec::with_capacity(area_height + 1);

    let mut line_idx = win.scroll_offset;
    // Hoisted allocations — reused across lines to avoid ~160 allocs/frame.
    let mut full_chars: Vec<char> = Vec::with_capacity(256);
    let mut char_styles: Vec<CharStyle> = Vec::with_capacity(256);

    while pixel_y < pixel_y_limit && line_idx < display_lines {
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
        let line_byte_end = buf.rope().char_to_byte(line_char_end);

        // Org headings: determine heading level from the star prefix.
        // `*` = level 1, `**` = level 2, `***` = level 3, deeper = no scaling.
        // Emacs pattern: org-level-1 is largest, org-level-3 is slightly enlarged.
        let org_heading_level: u8 = if needs_spans {
            let has_heading = syntax_spans
                .map(|spans| {
                    let start_idx = spans.partition_point(|s| s.byte_end <= line_byte_start);
                    spans[start_idx..]
                        .iter()
                        .take_while(|s| s.byte_start < line_byte_end)
                        .any(|s| s.theme_key == "markup.heading" && s.byte_start >= line_byte_start)
                })
                .unwrap_or(false);
            if has_heading {
                // Count leading stars to determine level.
                full_chars
                    .iter()
                    .take_while(|&&c| c == '*')
                    .count()
                    .min(255) as u8
            } else {
                0
            }
        } else {
            0
        };
        let org_heading_scale = org_heading_scale_for_level(org_heading_level);
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
            // Spans are sorted by byte_start — binary search to skip irrelevant ones.
            if let Some(spans) = syntax_spans {
                let line_byte_end = buf.rope().char_to_byte(line_char_end);
                // Find first span that could overlap this line (byte_end > line_byte_start).
                let start_idx = spans.partition_point(|s| s.byte_end <= line_byte_start);
                for span in &spans[start_idx..] {
                    if span.byte_start >= line_byte_end {
                        break; // all remaining spans are past this line
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
                if pixel_y >= pixel_y_limit {
                    break;
                }
                // Record this display row's pixel Y.
                y_map_entries.push(pixel_y);

                // Line height: first segment scaled, continuations normal.
                let seg_scale = if is_first { org_heading_scale } else { 1.0 };
                let seg_height = seg_scale * cell_height;

                if is_first {
                    // Gutter at pixel Y with scaling.
                    gutter::render_gutter_line_at_y(
                        canvas,
                        editor,
                        buf,
                        pixel_y,
                        area_col,
                        line_idx,
                        gutter_w,
                        win.cursor_row,
                        is_cursor_line,
                        seg_height,
                        seg_scale,
                        &breakpoint_lines,
                        stopped_line,
                        &line_severities,
                    );
                } else {
                    // Continuation line gutter.
                    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
                    let padding = " ".repeat(gutter_w);
                    canvas.draw_text_at_y(pixel_y, area_col, &padding, gutter_fg, 1.0);
                }

                let base_avail = if is_first { text_width } else { cont_text_w };
                // Scale down available width for heading lines so scaled glyphs
                // don't overflow into adjacent split windows.
                let avail = if seg_scale > 1.0 {
                    (base_avail as f32 / seg_scale).floor() as usize
                } else {
                    base_avail
                };
                let end = find_wrap_break(&full_chars, pos, avail);
                let chunk_chars = &full_chars[pos..end];
                let chunk_styles = &char_styles[pos..end];

                // If cursorline is active, fill the background for the whole line.
                if show_cursorline && is_cursor_line {
                    if let Some(bg_tc) = cursorline_style.bg {
                        let bg = theme::theme_color_to_skia(&bg_tc);
                        canvas.draw_rect_at_y(pixel_y, text_col, text_width, seg_height, bg);
                    }
                }

                let mut current_col = text_col;
                if !is_first {
                    // Indent + showbreak prefix
                    let prefix_fg = theme::ts_fg(editor, "ui.gutter");
                    if indent_len > 0 {
                        let indent = " ".repeat(indent_len);
                        canvas.draw_text_at_y(pixel_y, current_col, &indent, prefix_fg, 1.0);
                        current_col += indent_len;
                    }
                    if !editor.show_break.is_empty() {
                        canvas.draw_text_at_y(
                            pixel_y,
                            current_col,
                            &editor.show_break,
                            prefix_fg,
                            1.0,
                        );
                        current_col += show_break_width;
                    }
                }

                draw_styled_at(
                    canvas,
                    pixel_y,
                    current_col,
                    chunk_chars,
                    chunk_styles,
                    seg_scale,
                    seg_height,
                );

                pixel_y += seg_height;
                is_first = false;
                pos = end;
                if pos >= full_count {
                    break;
                }
            }
        } else {
            // No wrap.
            let line_height = org_heading_scale * cell_height;

            // Record this display row's pixel Y.
            y_map_entries.push(pixel_y);

            gutter::render_gutter_line_at_y(
                canvas,
                editor,
                buf,
                pixel_y,
                area_col,
                line_idx,
                gutter_w,
                win.cursor_row,
                is_cursor_line,
                line_height,
                org_heading_scale,
                &breakpoint_lines,
                stopped_line,
                &line_severities,
            );

            // Cursorline full-width background.
            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_at_y(pixel_y, text_col, text_width, line_height, bg);
                }
            }

            let visible_start = col_offset.min(full_count);
            // Scale down available width for heading lines so scaled glyphs
            // don't overflow into adjacent split windows.
            let effective_width = if org_heading_scale > 1.0 {
                (text_width as f32 / org_heading_scale).floor() as usize
            } else {
                text_width
            };
            // Walk from visible_start accumulating display width to find visible_end.
            let mut vis_width = 0;
            let mut visible_end = visible_start;
            for &ch in &full_chars[visible_start..] {
                let w = char_width(ch);
                if vis_width + w > effective_width {
                    break;
                }
                vis_width += w;
                visible_end += 1;
            }
            let visible_chars = &full_chars[visible_start..visible_end];
            let visible_styles = &char_styles[visible_start..visible_end];

            draw_styled_at(
                canvas,
                pixel_y,
                text_col,
                visible_chars,
                visible_styles,
                org_heading_scale,
                line_height,
            );

            pixel_y += line_height;
        }

        line_idx += 1;
    }
    // Tilde lines past EOF.
    while pixel_y < pixel_y_limit {
        y_map_entries.push(pixel_y);
        canvas.draw_text_at_y(
            pixel_y,
            area_col,
            &format!("{}{}", " ".repeat(gutter_w.saturating_sub(1)), "~"),
            gutter_fg,
            1.0,
        );
        pixel_y += cell_height;
    }

    PixelYMap {
        entries: y_map_entries,
        base_row: area_row,
        cell_height,
    }
}

/// Draw styled text at a pixel Y position using 3-pass run batching.
///
/// Pass 1: coalesce adjacent cells with the same bg into single rects.
/// Pass 2: accumulate text into runs with uniform style, flush via `draw_text_run_at_y`.
/// Pass 3: coalesce adjacent underlined cells into single line spans.
///
/// `pixel_y` is the exact pixel Y coordinate. `line_height` is the pixel height
/// of this line (scale * cell_height). Column positioning remains cell-based.
///
/// Reduces ~80 Skia calls per line to ~3-5 (one per style run).
fn draw_styled_at(
    canvas: &mut SkiaCanvas,
    pixel_y: f32,
    col: usize,
    chars: &[char],
    styles: &[CharStyle],
    scale: f32,
    line_height: f32,
) {
    if chars.is_empty() {
        return;
    }
    let ascii_ok = *canvas.ascii_in_font();

    // Pre-compute cumulative display column for each char (CJK = 2 cols).
    // When scale > 1.0, each character occupies more horizontal space.
    // We use fractional column offsets rounded to the nearest cell to keep
    // runs positioned correctly under Skia's natural glyph advance.
    let mut col_offsets = Vec::with_capacity(chars.len() + 1);
    if scale != 1.0 {
        let mut acc_f = 0.0f32;
        for &ch in chars {
            col_offsets.push(acc_f.round() as usize);
            acc_f += char_width(ch) as f32 * scale;
        }
        col_offsets.push(acc_f.round() as usize);
    } else {
        let mut acc = 0usize;
        for &ch in chars {
            col_offsets.push(acc);
            acc += char_width(ch);
        }
        col_offsets.push(acc); // sentinel for total width
    }

    // Pass 1: Coalesce background rects (pixel-precise height).
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
                    canvas.draw_rect_at_y(pixel_y, col + start_col, width, line_height, bg);
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
                    canvas.draw_text_run_at_y(
                        pixel_y,
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
                    canvas.draw_char_at_y(
                        pixel_y,
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
            canvas.draw_text_run_at_y(
                pixel_y,
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
                canvas.draw_underline_at_y(pixel_y, col + start_col, width, fg);
            }
        }
        if let Some((_, start_col, fg)) = ul_start {
            let width = col_offsets[styles.len()] - start_col;
            canvas.draw_underline_at_y(pixel_y, col + start_col, width, fg);
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

use mae_core::render_common::color::{parse_hex3, parse_hex6};

#[cfg(test)]
mod tests {
    use super::*;

    // parse_hex6/parse_hex3 tests are in mae_core::render_common::color

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

    // --- Pixel-Y / variable-height regression tests ---

    #[test]
    fn pixel_y_map_lookup_basic() {
        let map = PixelYMap {
            entries: vec![0.0, 20.0, 40.0, 70.0], // row 3 is taller (heading)
            base_row: 0,
            cell_height: 20.0,
        };
        assert_eq!(map.pixel_y_for_row(0), 0.0);
        assert_eq!(map.pixel_y_for_row(1), 20.0);
        assert_eq!(map.pixel_y_for_row(2), 40.0);
        assert_eq!(map.pixel_y_for_row(3), 70.0);
    }

    #[test]
    fn pixel_y_map_fallback_for_missing_row() {
        let map = PixelYMap {
            entries: vec![0.0, 20.0],
            base_row: 0,
            cell_height: 20.0,
        };
        // Row 5 is not in the map — falls back to row * cell_height.
        assert_eq!(map.pixel_y_for_row(5), 100.0);
    }

    #[test]
    fn pixel_y_map_with_base_row_offset() {
        let map = PixelYMap {
            entries: vec![100.0, 120.0, 140.0],
            base_row: 5,
            cell_height: 20.0,
        };
        assert_eq!(map.pixel_y_for_row(5), 100.0);
        assert_eq!(map.pixel_y_for_row(6), 120.0);
        assert_eq!(map.pixel_y_for_row(7), 140.0);
    }

    #[test]
    fn pixel_y_map_line_height() {
        let map = PixelYMap {
            entries: vec![0.0, 30.0, 50.0], // first line is 30px (heading), second is 20px
            base_row: 0,
            cell_height: 20.0,
        };
        assert_eq!(map.line_height_for_row(0), 30.0);
        assert_eq!(map.line_height_for_row(1), 20.0);
        // Last row falls back to cell_height.
        assert_eq!(map.line_height_for_row(2), 20.0);
    }

    #[test]
    fn org_heading_scale_levels() {
        assert_eq!(org_heading_scale_for_level(1), 1.5);
        assert_eq!(org_heading_scale_for_level(2), 1.3);
        assert_eq!(org_heading_scale_for_level(3), 1.15);
        assert_eq!(org_heading_scale_for_level(4), 1.0);
        assert_eq!(org_heading_scale_for_level(0), 1.0);
        assert_eq!(org_heading_scale_for_level(255), 1.0);
    }

    #[test]
    fn extra_rows_for_scale_values() {
        assert_eq!(extra_rows_for_scale(1.0), 0);
        assert_eq!(extra_rows_for_scale(1.15), 1);
        assert_eq!(extra_rows_for_scale(1.3), 1);
        assert_eq!(extra_rows_for_scale(1.5), 1);
    }

    #[test]
    fn help_buffer_heading_scale_with_markup_spans() {
        // Simulate help buffer with markup.heading spans generated from `*` prefix lines.
        let mut buf = mae_core::Buffer::new();
        buf.insert_text_at(0, "* Welcome\nSome text\n** Details\n");

        // Build heading spans the same way lib.rs does for help buffers.
        let rope = buf.rope();
        let mut spans: Vec<HighlightSpan> = Vec::new();
        for line_idx in 0..buf.line_count() {
            let line = rope.line(line_idx);
            let star_count = line.chars().take_while(|&c| c == '*').count();
            if star_count > 0 && line.len_chars() > star_count && line.char(star_count) == ' ' {
                let line_start = rope.line_to_char(line_idx);
                let line_len = line.len_chars();
                let text_len = if line_idx + 1 < buf.line_count() {
                    line_len.saturating_sub(1)
                } else {
                    line_len
                };
                let byte_start = rope.char_to_byte(line_start);
                let byte_end = rope.char_to_byte(line_start + text_len);
                spans.push(HighlightSpan {
                    byte_start,
                    byte_end,
                    theme_key: "markup.heading",
                });
            }
        }

        // `* Welcome` (level 1) should scale to 1.5
        let scale0 = line_heading_scale(&buf, Some(&spans), 0);
        assert_eq!(scale0, 1.5);

        // `Some text` should not scale
        let scale1 = line_heading_scale(&buf, Some(&spans), 1);
        assert_eq!(scale1, 1.0);

        // `** Details` (level 2) should scale to 1.3
        let scale2 = line_heading_scale(&buf, Some(&spans), 2);
        assert_eq!(scale2, 1.3);
    }

    #[test]
    fn help_buffer_heading_extra_rows() {
        let mut buf = mae_core::Buffer::new();
        buf.insert_text_at(0, "* H1\ntext\n** H2\n");
        let rope = buf.rope();
        let mut spans: Vec<HighlightSpan> = Vec::new();
        for line_idx in 0..buf.line_count() {
            let line = rope.line(line_idx);
            let star_count = line.chars().take_while(|&c| c == '*').count();
            if star_count > 0 && line.len_chars() > star_count && line.char(star_count) == ' ' {
                let line_start = rope.line_to_char(line_idx);
                let line_len = line.len_chars();
                let text_len = if line_idx + 1 < buf.line_count() {
                    line_len.saturating_sub(1)
                } else {
                    line_len
                };
                let byte_start = rope.char_to_byte(line_start);
                let byte_end = rope.char_to_byte(line_start + text_len);
                spans.push(HighlightSpan {
                    byte_start,
                    byte_end,
                    theme_key: "markup.heading",
                });
            }
        }
        // Lines 0 and 2 are headings. Extra rows should be > 0.
        let extra = heading_extra_rows(&buf, Some(&spans), 0, 3);
        assert!(extra > 0, "expected extra rows for 2 headings, got 0");
    }
}
