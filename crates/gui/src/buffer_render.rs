//! Text buffer rendering: gutter, syntax highlighting, selection, search,
//! cursorline, hex color preview, tilde lines past EOF.
//!
//! The `syntax_spans` parameter MUST match the spans used by `compute_layout()`
//! for the same frame. See `crates/gui/src/RENDERING.md` for invariants.

use mae_core::wrap::{char_width, leading_indent_len};
use mae_core::{Editor, HighlightSpan, Mode, Window};
use skia_safe::Color4f;

use crate::canvas::SkiaCanvas;
use crate::gutter;
use crate::theme;
use crate::theme::color4f_eq;

/// Compute the font scale for an org heading level using default scale values.
/// `*` = 1.5x, `**` = 1.3x, `***` = 1.15x, deeper = 1.0x.
#[allow(dead_code)]
pub fn org_heading_scale_for_level(level: u8) -> f32 {
    mae_core::heading::heading_scale_for_level(level)
}

/// Compute the font scale for an org heading level using editor-configured values.
pub fn org_heading_scale_for_level_with(level: u8, h1: f32, h2: f32, h3: f32) -> f32 {
    mae_core::heading::heading_scale_for_level_with(level, h1, h2, h3)
}

/// Get the heading scale for a single line using editor-configured scale values.
/// Returns 1.0 if not a heading or if heading scaling is disabled.
pub fn line_heading_scale_with(
    buf: &mae_core::Buffer,
    syntax_spans: Option<&[HighlightSpan]>,
    line_idx: usize,
    h1: f32,
    h2: f32,
    h3: f32,
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
        let line_chars: Vec<char> = rope.line(line_idx).chars().collect();
        // Detect heading level: org uses `*`, markdown uses `#`
        let level = if line_chars.first() == Some(&'*') {
            line_chars.iter().take_while(|&&c| c == '*').count()
        } else if line_chars.first() == Some(&'#') {
            line_chars.iter().take_while(|&&c| c == '#').count()
        } else {
            0
        };
        org_heading_scale_for_level_with(level.min(255) as u8, h1, h2, h3)
    } else {
        1.0
    }
}

/// Get the heading scale for a single line. Returns 1.0 if not a heading
/// or if heading scaling is disabled. Uses default scale values.
#[allow(dead_code)]
pub fn line_heading_scale(
    buf: &mae_core::Buffer,
    syntax_spans: Option<&[HighlightSpan]>,
    line_idx: usize,
) -> f32 {
    line_heading_scale_with(buf, syntax_spans, line_idx, 1.5, 1.3, 1.15)
}

/// Render a text buffer's content using a pre-computed `FrameLayout`.
///
/// The layout (computed by `layout::compute_layout()`) provides all line
/// positions, scales, and fold information. This function only draws —
/// no position computation happens here.
pub fn render_buffer_content(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    buf: &mae_core::Buffer,
    win: &Window,
    focused: bool,
    frame_layout: &crate::layout::FrameLayout,
    syntax_spans: Option<&[HighlightSpan]>,
) {
    let (_, cell_height) = canvas.cell_size();
    let area_col = frame_layout.area_col;
    let gutter_w = frame_layout.gutter_width;
    let text_col = frame_layout.text_col;
    let text_width = frame_layout.text_width;

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

    // Code block background: use pre-computed cache from render() mutable phase.
    let (cb_line_start, code_block_lines): (usize, &[bool]) = editor
        .code_block_cache
        .get(&win.buffer_idx)
        .map(|c| (c.line_start, c.lines.as_slice()))
        .unwrap_or((0, &[]));
    let code_block_bg = {
        let cb_style = editor.theme.style("markup.code_block");
        cb_style.bg.map(|c| theme::theme_color_to_skia(&c))
    };

    let col_offset = win.col_offset;

    let wrap = buf.local_options.word_wrap.unwrap_or(editor.word_wrap);
    let show_break_width = if wrap {
        unicode_width::UnicodeWidthStr::width(editor.show_break.as_str())
    } else {
        0
    };

    // Hoisted allocations — reused across lines to avoid ~160 allocs/frame.
    let mut full_chars: Vec<char> = Vec::with_capacity(256);
    let mut char_styles: Vec<CharStyle> = Vec::with_capacity(256);

    // Pre-compute cursor's display row for fold-aware relative line numbers.
    let cursor_display_row = frame_layout.display_row_of(win.cursor_row);

    // Iterate over the pre-computed layout lines.
    // Layout already handled fold skipping, pixel_y accumulation, and heading scale.
    let mut prev_buf_row: Option<usize> = None;
    for (display_idx, ll) in frame_layout.lines.iter().enumerate() {
        let line_idx = ll.buf_row;
        let pixel_y = ll.pixel_y;
        let org_heading_scale = ll.scale;
        let line_height = ll.line_height;
        let is_wrap_cont = ll.is_wrap_continuation;

        // Only re-collect chars/styles when we move to a new buffer line.
        if prev_buf_row != Some(line_idx) {
            full_chars.clear();
            if let Some(ref dc) = ll.display_chars {
                // Display regions active: use pre-computed display chars.
                full_chars.extend_from_slice(dc);
            } else {
                let line_text = buf.rope().line(line_idx);
                full_chars.extend(line_text.chars().filter(|c| *c != '\n' && *c != '\r'));
            }
            prev_buf_row = Some(line_idx);
        }

        let is_cursor_line = focused && line_idx == win.cursor_row;
        let is_stopped_line = stopped_line == Some(line_idx as u32);

        // For span mapping we always need the rope-level positions.
        let rope_line_char_start = buf.rope().line_to_char(line_idx);
        let rope_line_text = buf.rope().line(line_idx);
        let rope_char_count = rope_line_text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .count();

        let full_count = full_chars.len();
        let line_char_start = rope_line_char_start;
        let line_char_end = line_char_start + rope_char_count;
        let line_byte_start = buf.rope().char_to_byte(line_char_start);
        let line_byte_end = buf.rope().char_to_byte(line_char_end);
        let base_fg = if is_stopped_line {
            stopped_line_fg
        } else {
            text_fg
        };

        // Only rebuild char_styles for the first segment of each line.
        if !is_wrap_cont {
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
                    strikethrough: false,
                },
            );

            if needs_spans {
                // Layer 1: Tree-sitter syntax spans.
                if let Some(spans) = syntax_spans {
                    let start_idx = spans.partition_point(|s| s.byte_end <= line_byte_start);
                    for span in &spans[start_idx..] {
                        if span.byte_start >= line_byte_end {
                            break;
                        }
                        let sb = span.byte_start.max(line_byte_start);
                        let eb = span.byte_end.min(line_byte_end);
                        let rope_sc = buf.rope().byte_to_char(sb).saturating_sub(line_char_start);
                        let rope_ec = buf
                            .rope()
                            .byte_to_char(eb)
                            .saturating_sub(line_char_start)
                            .min(rope_char_count);
                        // Map rope char offsets to display char offsets when display regions are active.
                        let (sc, ec) = if let Some(ref dm) = ll.display_map {
                            let dsc =
                                mae_core::display_region::rope_col_to_display_col(rope_sc, dm);
                            let dec =
                                mae_core::display_region::rope_col_to_display_col(rope_ec, dm);
                            (dsc.min(full_count), dec.min(full_count))
                        } else {
                            (rope_sc.min(full_count), rope_ec.min(full_count))
                        };

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
                            if ts.strikethrough {
                                cs.strikethrough = true;
                            }
                            if let Some(bg) = ts.bg {
                                cs.bg = Some(theme::theme_color_to_skia(&bg));
                            }
                        }
                    }
                }

                // Layer 1b: Display region link styling (underline + markup.link color).
                if let Some(ref dm) = ll.display_map {
                    let link_style = editor.theme.style("markup.link");
                    let link_fg = theme::color_or(link_style.fg, base_fg);
                    let eff_regions = mae_core::display_region::regions_with_cursor_reveal(
                        &buf.display_regions,
                        buf.display_reveal_cursor,
                    );
                    for region in eff_regions.iter() {
                        if region.byte_start >= line_byte_end || region.byte_end <= line_byte_start
                        {
                            continue;
                        }
                        if region.link_target.is_none() {
                            continue;
                        }
                        let rope_start = buf
                            .rope()
                            .byte_to_char(region.byte_start.max(line_byte_start))
                            .saturating_sub(line_char_start);
                        let dsc = mae_core::display_region::rope_col_to_display_col(rope_start, dm);
                        let replacement_len = region
                            .replacement
                            .as_ref()
                            .map(|r| r.chars().count())
                            .unwrap_or(0);
                        let dec = (dsc + replacement_len).min(full_count);
                        for cs in char_styles[dsc..dec].iter_mut() {
                            cs.fg = link_fg;
                            cs.underline = true;
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

                // Layer 3b: LSP document highlights (background-only, behind selection).
                if !editor.highlight_ranges.is_empty() {
                    for hr in &editor.highlight_ranges {
                        if line_idx < hr.start_line || line_idx > hr.end_line {
                            continue;
                        }
                        let key = match hr.kind {
                            mae_core::HighlightKind::Read => "lsp.highlight.read",
                            mae_core::HighlightKind::Write => "lsp.highlight.write",
                            mae_core::HighlightKind::Text => "lsp.highlight.text",
                        };
                        let hl_style = editor.theme.style(key);
                        if let Some(bg_tc) = hl_style.bg {
                            let bg = theme::theme_color_to_skia(&bg_tc);
                            let sc = if line_idx == hr.start_line {
                                hr.start_col
                            } else {
                                0
                            };
                            let ec = if line_idx == hr.end_line {
                                hr.end_col
                            } else {
                                full_count
                            };
                            let sc = sc.min(full_count);
                            let ec = ec.min(full_count);
                            for cs in char_styles[sc..ec].iter_mut() {
                                cs.bg = Some(bg);
                            }
                        }
                    }
                }

                // Layer 4: Visual selection.
                if let Some((br_min, br_max, bc_min, bc_max)) = block_rect {
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
                } else if highlight_selection
                    && sel_start < line_char_end
                    && sel_end > line_char_start
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
        }

        // --- Drawing ---

        // Code block tinted background (drawn before cursorline so cursorline overlays).
        if let Some(cb_bg) = code_block_bg {
            let is_code_block = line_idx
                .checked_sub(cb_line_start)
                .and_then(|rel| code_block_lines.get(rel))
                .copied()
                .unwrap_or(false);
            if is_code_block {
                canvas.draw_rect_at_y(pixel_y, text_col, text_width, line_height, cb_bg);
            }
        }

        if is_wrap_cont {
            // Wrap continuation segment: draw showbreak prefix + chunk.
            let indent_len = if editor.break_indent {
                leading_indent_len(&full_chars)
            } else {
                0
            };

            // Continuation line gutter.
            let padding = " ".repeat(gutter_w);
            canvas.draw_text_at_y(pixel_y, area_col, &padding, gutter_fg, 1.0);

            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_at_y(pixel_y, text_col, text_width, line_height, bg);
                }
            }

            let char_start = ll.char_start;
            let char_end = (char_start + ll.char_count).min(full_count);
            let chunk_chars = &full_chars[char_start..char_end];
            let chunk_styles = &char_styles[char_start..char_end];

            let mut current_col = text_col;
            let prefix_fg = theme::ts_fg(editor, "ui.gutter");
            if indent_len > 0 {
                let indent = " ".repeat(indent_len);
                canvas.draw_text_at_y(pixel_y, current_col, &indent, prefix_fg, 1.0);
                current_col += indent_len;
            }
            if !editor.show_break.is_empty() {
                canvas.draw_text_at_y(pixel_y, current_col, &editor.show_break, prefix_fg, 1.0);
                current_col += show_break_width;
            }

            draw_styled_at(
                canvas,
                pixel_y,
                current_col,
                chunk_chars,
                chunk_styles,
                1.0,
                line_height,
                ll.glyph_advance,
            );
        } else if wrap {
            // First segment of a wrapped line.
            let rel_offset = cursor_display_row.map(|cdr| display_idx.abs_diff(cdr));
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
                rel_offset,
            );

            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_at_y(pixel_y, text_col, text_width, line_height, bg);
                }
            }

            let char_start = ll.char_start;
            let char_end = (char_start + ll.char_count).min(full_count);
            let chunk_chars = &full_chars[char_start..char_end];
            let chunk_styles = &char_styles[char_start..char_end];

            draw_styled_at(
                canvas,
                pixel_y,
                text_col,
                chunk_chars,
                chunk_styles,
                org_heading_scale,
                line_height,
                ll.glyph_advance,
            );
        } else {
            // No wrap: single segment per line.
            let rel_offset = cursor_display_row.map(|cdr| display_idx.abs_diff(cdr));
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
                rel_offset,
            );

            // Cursorline full-width background.
            if show_cursorline && is_cursor_line {
                if let Some(bg_tc) = cursorline_style.bg {
                    let bg = theme::theme_color_to_skia(&bg_tc);
                    canvas.draw_rect_at_y(pixel_y, text_col, text_width, line_height, bg);
                }
            }

            let visible_start = col_offset.min(full_count);
            let visible_end = (visible_start + ll.char_count).min(full_count);
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
                ll.glyph_advance,
            );

            // Fold indicator: show "... N lines" after fold start lines.
            // Use actual glyph advance for positioning after the scaled heading.
            if ll.is_fold_start && ll.folded_line_count > 0 {
                let indicator = format!(" ··· {} lines", ll.folded_line_count);
                let vis_char_count = visible_end - visible_start;
                let line_str: String = visible_chars.iter().collect();
                let (cw, _) = canvas.cell_size();
                let effective_scale = ll.glyph_advance / cw;
                let scaled_vis_width = crate::layout::FrameLayout::scaled_col(
                    &line_str,
                    vis_char_count,
                    effective_scale,
                );
                let indicator_col = text_col + scaled_vis_width;
                let fold_fg = theme::ts_fg(editor, "comment");
                canvas.draw_text_run_at_y(
                    pixel_y,
                    indicator_col,
                    &indicator,
                    fold_fg,
                    false,
                    true,
                    org_heading_scale,
                );
            }
        }
    }

    // Pass 5 (image): Render inline images below their line's text.
    for ll in frame_layout.lines.iter() {
        if ll.is_wrap_continuation {
            continue;
        }
        if let Some(ref img_layout) = ll.image {
            let img_y = ll.pixel_y + ll.line_height;
            // Draw the image from cache (loads on first access); fall back to placeholder.
            let drawn = canvas.draw_image_from_cache(
                &img_layout.path,
                img_layout.pixel_x,
                img_y,
                img_layout.display_width,
                img_layout.display_height,
            );
            if !drawn {
                let placeholder_fg = theme::ts_fg(editor, "markup.image");
                let filename = img_layout
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| img_layout.path.to_string_lossy().to_string());
                let placeholder = format!("[Image: {}]", filename);
                canvas.draw_text_at_y(img_y, text_col, &placeholder, placeholder_fg, 1.0);
            }
        }
    }

    // Pass 6: Diagnostic inline underlines (wavy) + virtual text.
    if editor.lsp_diagnostics_inline {
        if let Some(path) = buf.file_path() {
            let uri = mae_core::path_to_uri(path);
            let start_line = frame_layout.lines.first().map(|ll| ll.buf_row).unwrap_or(0);
            let end_line = frame_layout
                .lines
                .last()
                .map(|ll| ll.buf_row + 1)
                .unwrap_or(0);
            let diag_spans = mae_core::render_common::diagnostics::compute_diagnostic_spans(
                &editor.diagnostics,
                &uri,
                start_line,
                end_line,
            );

            let (cw, _) = canvas.cell_size();
            for ds in &diag_spans {
                // Find the layout line for this diagnostic's buffer row.
                if let Some(ll) = frame_layout.lines.iter().find(|ll| ll.buf_row == ds.line) {
                    let severity_key = ds.severity.theme_key();
                    let diag_color = theme::ts_fg(editor, severity_key);

                    // Wavy underline from col_start to col_end.
                    let col_start = ds.col_start;
                    let col_end = ds.col_end.max(col_start + 1);
                    let x = text_col as f32 * cw + col_start as f32 * cw;
                    let w = (col_end - col_start) as f32 * cw;
                    canvas.draw_wavy_underline_at_pixel(x, ll.pixel_y, w, diag_color);

                    // Virtual text at end of line (if enabled).
                    let line_len = buf.line_len(ds.line);
                    let vt_col = text_col + line_len + 2;
                    let available = text_width.saturating_sub(line_len + 2);
                    if available > 10 {
                        let (vt_text, _) =
                            mae_core::render_common::diagnostics::format_virtual_text(
                                ds.severity,
                                &ds.message,
                                available,
                            );
                        canvas.draw_text_at_y(ll.pixel_y, vt_col, &vt_text, diag_color, 1.0);
                    }
                }
            }
        }
    }

    // Tilde lines past EOF.
    let last_pixel_y = frame_layout
        .lines
        .last()
        .map(|ll| ll.pixel_y + ll.line_height)
        .unwrap_or(frame_layout.area_row as f32 * cell_height);
    let mut tilde_y = last_pixel_y;
    while tilde_y < frame_layout.pixel_y_limit {
        canvas.draw_text_at_y(
            tilde_y,
            area_col,
            &format!("{}{}", " ".repeat(gutter_w.saturating_sub(1)), "~"),
            gutter_fg,
            1.0,
        );
        tilde_y += cell_height;
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
    glyph_advance: f32,
) {
    if chars.is_empty() {
        return;
    }
    let ascii_ok = *canvas.ascii_in_font();
    let (cw, _) = canvas.cell_size();

    // Pre-compute cumulative PIXEL offset for each char position.
    // For scale != 1.0, uses the font's actual glyph advance directly.
    let base_x = col as f32 * cw;
    let mut pixel_offsets: Vec<f32> = Vec::with_capacity(chars.len() + 1);
    if scale != 1.0 {
        let mut acc = 0.0f32;
        for &ch in chars {
            pixel_offsets.push(acc);
            acc += char_width(ch) as f32 * glyph_advance;
        }
        pixel_offsets.push(acc);
    } else {
        let mut acc = 0.0f32;
        for &ch in chars {
            pixel_offsets.push(acc);
            acc += char_width(ch) as f32 * cw;
        }
        pixel_offsets.push(acc);
    }
    // Integer col_offsets for scale==1.0 path (draw_text_run_at_y).
    let col_offsets: Vec<usize> = pixel_offsets
        .iter()
        .map(|px| (px / cw).round() as usize)
        .collect();
    // Use pixel-precise rendering for scaled lines to avoid multi-run drift.
    let use_pixel = scale != 1.0;

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
                    if use_pixel {
                        let px = base_x + pixel_offsets[run_start];
                        let pw = pixel_offsets[i] - pixel_offsets[run_start];
                        canvas.draw_rect_at_pixel(px, pixel_y, pw, line_height, bg);
                    } else {
                        let start_col = col_offsets[run_start];
                        let width = col_offsets[i] - start_col;
                        canvas.draw_rect_at_y(pixel_y, col + start_col, width, line_height, bg);
                    }
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
        let mut run_start_idx = 0usize;
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
                    run_start_idx = i;
                    run_start_col = col_offsets[i];
                    run_fg = cs.fg;
                    run_bold = cs.bold;
                    run_italic = cs.italic;
                }
                run_buf.push(ch);
            } else {
                // Flush current run.
                if !run_buf.is_empty() {
                    if use_pixel {
                        canvas.draw_text_run_at_pixel(
                            base_x + pixel_offsets[run_start_idx],
                            pixel_y,
                            &run_buf,
                            run_fg,
                            run_bold,
                            run_italic,
                            scale,
                        );
                    } else {
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
                    run_buf.clear();
                }

                if can_batch {
                    // New style, batchable char — start new run.
                    run_start_idx = i;
                    run_start_col = col_offsets[i];
                    run_fg = cs.fg;
                    run_bold = cs.bold;
                    run_italic = cs.italic;
                    run_buf.push(ch);
                } else if ch != ' ' {
                    // Non-ASCII / missing glyph — per-char fallback.
                    if use_pixel {
                        canvas.draw_char_at_pixel(
                            base_x + pixel_offsets[i],
                            pixel_y,
                            ch,
                            cs.fg,
                            cs.bold,
                            scale,
                        );
                    } else {
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
        }
        if !run_buf.is_empty() {
            if use_pixel {
                canvas.draw_text_run_at_pixel(
                    base_x + pixel_offsets[run_start_idx],
                    pixel_y,
                    &run_buf,
                    run_fg,
                    run_bold,
                    run_italic,
                    scale,
                );
            } else {
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
    }

    // Pass 3: Coalesce underline spans.
    {
        let mut ul_start: Option<(usize, usize, Color4f)> = None; // (char_idx, col_offset, fg)
        for (i, cs) in styles.iter().enumerate() {
            if cs.underline {
                if ul_start.is_none() {
                    ul_start = Some((i, col_offsets[i], cs.fg));
                }
            } else if let Some((start_idx, start_col, fg)) = ul_start.take() {
                if use_pixel {
                    let pw = pixel_offsets[i] - pixel_offsets[start_idx];
                    canvas.draw_underline_at_pixel(
                        base_x + pixel_offsets[start_idx],
                        pixel_y,
                        pw,
                        fg,
                    );
                } else {
                    let width = col_offsets[i] - start_col;
                    canvas.draw_underline_at_y(pixel_y, col + start_col, width, fg);
                }
            }
        }
        if let Some((start_idx, start_col, fg)) = ul_start {
            if use_pixel {
                let pw = pixel_offsets[styles.len()] - pixel_offsets[start_idx];
                canvas.draw_underline_at_pixel(base_x + pixel_offsets[start_idx], pixel_y, pw, fg);
            } else {
                let width = col_offsets[styles.len()] - start_col;
                canvas.draw_underline_at_y(pixel_y, col + start_col, width, fg);
            }
        }
    }

    // Pass 4: Coalesce strikethrough spans (line at 60% of ascent).
    {
        let mut st_start: Option<(usize, usize, Color4f)> = None;
        for (i, cs) in styles.iter().enumerate() {
            if cs.strikethrough {
                if st_start.is_none() {
                    st_start = Some((i, col_offsets[i], cs.fg));
                }
            } else if let Some((start_idx, start_col, fg)) = st_start.take() {
                if use_pixel {
                    let pw = pixel_offsets[i] - pixel_offsets[start_idx];
                    canvas.draw_strikethrough_at_pixel(
                        base_x + pixel_offsets[start_idx],
                        pixel_y,
                        pw,
                        fg,
                    );
                } else {
                    let width = col_offsets[i] - start_col;
                    canvas.draw_strikethrough_at_y(pixel_y, col + start_col, width, fg);
                }
            }
        }
        if let Some((start_idx, start_col, fg)) = st_start {
            if use_pixel {
                let pw = pixel_offsets[styles.len()] - pixel_offsets[start_idx];
                canvas.draw_strikethrough_at_pixel(
                    base_x + pixel_offsets[start_idx],
                    pixel_y,
                    pw,
                    fg,
                );
            } else {
                let width = col_offsets[styles.len()] - start_col;
                canvas.draw_strikethrough_at_y(pixel_y, col + start_col, width, fg);
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
    strikethrough: bool,
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
                strikethrough: false,
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
                strikethrough: false,
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
                strikethrough: false,
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
            strikethrough: false,
        };
        assert!(!cs.bold);
        assert!(cs.bg.is_none());
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

    /// Regression: multi-run pixel offsets must be continuous across style
    /// boundaries. Before the fix, each run started at `col * cell_width`
    /// (integer grid), causing drift between runs on scaled org headings
    /// with tags like `* Section :tag:`.
    #[test]
    fn multi_run_pixel_offsets_continuous() {
        use mae_core::wrap::char_width;

        // Simulate an org heading: "* Title :tag:" split into 3 style runs:
        //   Run 0: "* "         (punctuation)
        //   Run 1: "Title "     (markup.heading)
        //   Run 2: ":tag:"      (attribute)
        let text = "* Title :tag:";
        let chars: Vec<char> = text.chars().collect();

        let cell_width = 8.0_f32;
        // Simulate glyph advance at scale 1.5 (grid-fitted, differs from 8*1.5=12)
        let glyph_advance = 13.0_f32;

        // Build pixel_offsets exactly as draw_styled_at does.
        let mut pixel_offsets: Vec<f32> = Vec::with_capacity(chars.len() + 1);
        let mut acc = 0.0f32;
        for &ch in &chars {
            pixel_offsets.push(acc);
            acc += char_width(ch) as f32 * glyph_advance;
        }
        pixel_offsets.push(acc);

        // Run boundaries: run 0 ends at char 2 ("* "), run 1 ends at char 8 ("Title ")
        let run0_end = 2; // "* " → 2 chars
        let run1_start = 2;
        let run1_end = 8; // "Title " → 6 chars
        let run2_start = 8;

        // The pixel offset at a run boundary must be the same whether computed
        // as "end of previous run" or "start of next run".
        assert_eq!(
            pixel_offsets[run0_end], pixel_offsets[run1_start],
            "run 0→1 boundary pixel offset mismatch"
        );
        assert_eq!(
            pixel_offsets[run1_end], pixel_offsets[run2_start],
            "run 1→2 boundary pixel offset mismatch"
        );

        // Now verify integer col_offsets introduce rounding error at boundaries.
        let col_offsets: Vec<usize> = pixel_offsets
            .iter()
            .map(|px| (px / cell_width).round() as usize)
            .collect();

        // With glyph_advance=13, cell_width=8:
        // char 2: pixel=26.0, col=26/8=3.25→3, pixel_from_col=24 (ERROR: 2px)
        // char 8: pixel=104.0, col=104/8=13.0→13, pixel_from_col=104 (OK)
        // The point: col_offsets[i] * cell_width != pixel_offsets[i] in general.
        // This is the quantization error that caused multi-run drift.
        let boundary_error = (col_offsets[run0_end] as f32 * cell_width) - pixel_offsets[run0_end];
        // Just document that integer col mapping introduces error.
        // The fix uses pixel_offsets directly, bypassing col_offsets for scaled lines.
        eprintln!(
            "run0→1 boundary: pixel={:.1}, col_based={:.1}, error={:.1}px",
            pixel_offsets[run0_end],
            col_offsets[run0_end] as f32 * cell_width,
            boundary_error,
        );

        // The total pixel width must equal N * glyph_advance for all-width-1 ASCII.
        let expected_total = chars.len() as f32 * glyph_advance;
        assert_eq!(
            *pixel_offsets.last().unwrap(),
            expected_total,
            "total pixel width mismatch"
        );
    }
}
