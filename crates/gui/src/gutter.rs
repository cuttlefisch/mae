//! Gutter rendering: line numbers, breakpoint markers, diagnostic markers.
//!
//! Pure data types and logic live in `mae_core::render_common::gutter`.
//! This module handles the Skia-specific drawing.

use mae_core::render_common::gutter;
use mae_core::{DiagnosticSeverity, Editor};
use std::collections::{HashMap, HashSet};

use crate::canvas::SkiaCanvas;
use crate::theme;

// Re-export shared functions so call sites don't need to change.
pub use mae_core::render_common::gutter::{
    collect_breakpoints, collect_line_severities, gutter_width,
};

/// Render the gutter for one visible line at a pixel Y position.
/// `line_height` is the pixel height of this line (for cursorline bg).
/// `scale` is the font scale (for scaled line numbers on headings).
/// `display_offset` is the visual distance from the cursor line (for fold-aware relative numbers).
pub fn render_gutter_line_at_y(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    buf: &mae_core::Buffer,
    pixel_y: f32,
    screen_col_offset: usize,
    line_idx: usize,
    gutter_w: usize,
    cursor_row: usize,
    is_cursor_line: bool,
    line_height: f32,
    _scale: f32,
    breakpoint_lines: &HashSet<u32>,
    stopped_line: Option<u32>,
    line_severities: &HashMap<u32, DiagnosticSeverity>,
    display_offset: Option<usize>,
) {
    if gutter_w == 0 {
        return;
    }
    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
    let cursorline_bg = if is_cursor_line {
        theme::ts_bg(editor, "ui.cursorline")
    } else {
        None
    };

    // Background for cursorline gutter (pixel-precise height).
    if let Some(bg) = cursorline_bg {
        canvas.draw_rect_at_y(pixel_y, screen_col_offset, gutter_w, line_height, bg);
    }

    // Line number (always at 1.0 scale — gutter stays fixed width).
    let line_num = gutter::format_line_number_with_offset(
        line_idx,
        cursor_row,
        gutter_w,
        editor.show_line_numbers,
        editor.relative_line_numbers,
        display_offset,
    );
    canvas.draw_text_at_y(pixel_y, screen_col_offset, &line_num, gutter_fg, 1.0);

    // Marker column (last char of gutter).
    let line_idx_u32 = line_idx as u32;
    let marker = gutter::resolve_gutter_marker(
        stopped_line == Some(line_idx_u32),
        breakpoint_lines.contains(&line_idx_u32),
        line_severities.get(&line_idx_u32).copied(),
    );
    if let Some((ch, key)) = marker.glyph_and_theme_key() {
        let marker_fg = theme::ts_fg(editor, key);
        canvas.draw_char_at_y(
            pixel_y,
            screen_col_offset + gutter_w - 1,
            ch,
            marker_fg,
            false,
            false,
            1.0,
        );
    } else if let Some((ch, key)) = gutter::git_line_marker(buf, line_idx) {
        let git_fg = theme::ts_fg(editor, key);
        canvas.draw_char_at_y(
            pixel_y,
            screen_col_offset + gutter_w - 1,
            ch,
            git_fg,
            false,
            false,
            1.0,
        );
    } else if gutter::is_line_changed(buf, line_idx) {
        let change_fg = theme::ts_fg(editor, "diff.modified");
        canvas.draw_char_at_y(
            pixel_y,
            screen_col_offset + gutter_w - 1,
            '│',
            change_fg,
            false,
            false,
            1.0,
        );
    }
}

/// Render the gutter for one visible line (cell-based, for non-buffer contexts).
#[allow(dead_code)]
pub fn render_gutter_line(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    screen_row: usize,
    screen_col_offset: usize,
    line_idx: usize,
    gutter_w: usize,
    cursor_row: usize,
    is_cursor_line: bool,
    breakpoint_lines: &HashSet<u32>,
    stopped_line: Option<u32>,
    line_severities: &HashMap<u32, DiagnosticSeverity>,
) {
    if gutter_w == 0 {
        return;
    }
    let gutter_fg = theme::ts_fg(editor, "ui.gutter");
    let cursorline_bg = if is_cursor_line {
        theme::ts_bg(editor, "ui.cursorline")
    } else {
        None
    };

    // Background for cursorline gutter.
    if let Some(bg) = cursorline_bg {
        canvas.draw_rect_fill(screen_row, screen_col_offset, gutter_w, 1, bg);
    }

    // Line number.
    let line_num = gutter::format_line_number(
        line_idx,
        cursor_row,
        gutter_w,
        editor.show_line_numbers,
        editor.relative_line_numbers,
    );
    canvas.draw_text_at(screen_row, screen_col_offset, &line_num, gutter_fg);

    // Marker column (last char of gutter).
    let line_idx_u32 = line_idx as u32;
    let marker = gutter::resolve_gutter_marker(
        stopped_line == Some(line_idx_u32),
        breakpoint_lines.contains(&line_idx_u32),
        line_severities.get(&line_idx_u32).copied(),
    );
    if let Some((ch, key)) = marker.glyph_and_theme_key() {
        let marker_fg = theme::ts_fg(editor, key);
        canvas.draw_text_at(
            screen_row,
            screen_col_offset + gutter_w - 1,
            &ch.to_string(),
            marker_fg,
        );
    }
}
