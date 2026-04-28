//! Status bar and command line rendering for the GUI backend.
//!
//! Segment building, truncation, and formatting logic lives in
//! `mae_core::render_common::status`.  This module handles Skia drawing.

use mae_core::render_common::status::{
    build_status_segments, command_line_text, layout_status_segments, mode_label, mode_theme_key,
};
use mae_core::Editor;
use skia_safe::Color4f;
use unicode_width::UnicodeWidthStr;

use crate::canvas::SkiaCanvas;
use crate::theme;

/// Render the full status bar at the given screen row.
pub fn render_status_bar(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    row: usize,
    cols: usize,
    frame_ms: Option<u64>,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mode_str = mode_label(editor);

    // Mode label colors.
    let mode_style = editor.theme.style(mode_theme_key(editor));
    let mode_fg = theme::color_or(mode_style.fg, theme::DEFAULT_FG);
    let mode_bg = theme::color_or(mode_style.bg, theme::STATUS_BG);

    // Status bar background.
    let sl_style = editor.theme.style("ui.statusline");
    let (sl_fg, sl_bg) = if editor.bell_active() {
        (
            theme::color_or(sl_style.bg, Color4f::new(0.0, 0.0, 0.0, 1.0)),
            theme::color_or(sl_style.fg, Color4f::new(1.0, 1.0, 1.0, 1.0)),
        )
    } else {
        (
            theme::color_or(sl_style.fg, theme::DEFAULT_FG),
            theme::color_or(sl_style.bg, theme::STATUS_BG),
        )
    };

    canvas.draw_rect_fill(row, 0, cols, 1, sl_bg);

    // Mode label.
    let mode_len = UnicodeWidthStr::width(mode_str.as_str());
    canvas.draw_rect_fill(row, 0, mode_len, 1, mode_bg);
    canvas.draw_text_at(row, 0, &mode_str, mode_fg);

    // Available space after mode label.
    let avail = cols.saturating_sub(mode_len);
    if avail == 0 {
        return;
    }

    // Build and lay out segments using shared logic.
    let mut segments = build_status_segments(editor, frame_ms);
    let layout = layout_status_segments(&mut segments, avail, &buf.name, buf.modified);

    canvas.draw_text_at(row, mode_len, &layout.left_text, sl_fg);

    let right_w = UnicodeWidthStr::width(layout.right_text.as_str());
    let right_col = (mode_len + avail).saturating_sub(right_w);
    canvas.draw_text_at(row, right_col, &layout.right_text, sl_fg);
}

/// Render the command/message line at the given screen row.
pub fn render_command_line(canvas: &mut SkiaCanvas, editor: &Editor, row: usize, cols: usize) {
    let text = command_line_text(editor);
    let fg = theme::ts_fg(editor, "ui.commandline");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);
    canvas.draw_rect_fill(row, 0, cols, 1, bg);
    canvas.draw_text_at(row, 0, &text, fg);
}

// Tests for shared status logic live in mae_core::render_common::status::tests.
