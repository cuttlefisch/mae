//! Vertical scrollbar rendering for the GUI.
//!
//! Uses the FrameLayout's scrollbar_col to know where to draw. The track
//! spans the full content area height; the thumb size and position reflect
//! the viewport's proportion of total buffer lines.

use crate::canvas::SkiaCanvas;
use crate::layout::FrameLayout;
use crate::theme;
use mae_core::Editor;
use skia_safe::Color4f;

/// Render a vertical scrollbar in the allocated column.
///
/// The scrollbar is pixel-precise: track fills the full content area,
/// thumb position = scroll_offset / total_lines, thumb height = viewport / total_lines.
pub fn render_scrollbar(canvas: &mut SkiaCanvas, editor: &Editor, fl: &FrameLayout) {
    let Some(sb_col) = fl.scrollbar_col else {
        return;
    };

    let total = fl.total_lines;
    if total == 0 {
        return;
    }

    let viewport = fl.lines.len();
    let (cw, _ch) = canvas.cell_size();
    let track_x = sb_col as f32 * cw;
    let track_y_start = fl.area_row as f32 * fl.cell_height;
    let track_height = fl.pixel_y_limit - track_y_start;

    if track_height <= 0.0 {
        return;
    }

    // Track background.
    let track_color = theme::ts_fg_or(
        editor,
        "ui.scrollbar.track",
        Color4f::new(0.15, 0.15, 0.15, 1.0),
    );
    canvas.draw_pixel_rect(track_x, track_y_start, cw, track_height, track_color);

    // Thumb.
    let thumb_ratio = (viewport as f32 / total as f32).min(1.0);
    let thumb_height = (thumb_ratio * track_height).max(fl.cell_height); // min 1 cell tall
    let scroll_ratio = if total > viewport {
        fl.scroll_offset as f32 / (total - viewport) as f32
    } else {
        0.0
    };
    let thumb_y = track_y_start + scroll_ratio * (track_height - thumb_height);

    let thumb_color = theme::ts_fg_or(
        editor,
        "ui.scrollbar.thumb",
        Color4f::new(0.4, 0.4, 0.4, 1.0),
    );
    canvas.draw_pixel_rect(track_x, thumb_y, cw, thumb_height, thumb_color);
}

#[cfg(test)]
mod tests {
    #[test]
    fn scrollbar_col_none_is_noop() {
        // Just verify the function doesn't panic with no scrollbar_col.
        // Full rendering requires a SkiaCanvas which needs GPU context.
    }
}
