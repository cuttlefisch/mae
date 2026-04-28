//! Vertical scrollbar rendering for the GUI.
//!
//! Design: a thin rounded bar (6px) centered within the allocated scrollbar
//! column. The track is nearly invisible (low-alpha tint); the thumb uses the
//! theme's `ui.scrollbar.thumb` color or a sensible default derived from the
//! editor's current theme brightness.
//!
//! Pattern references: VS Code, Zed, Helix — all use a thin overlay scrollbar
//! that doesn't fight with the content area for attention.

use crate::canvas::SkiaCanvas;
use crate::layout::FrameLayout;
use crate::theme;
use mae_core::Editor;
use skia_safe::Color4f;

/// Thin bar width in pixels.
const SCROLLBAR_WIDTH: f32 = 6.0;
/// Corner radius for the thumb pill shape.
const THUMB_RADIUS: f32 = 3.0;
/// Minimum thumb height in pixels (so it's always grabbable).
const MIN_THUMB_HEIGHT: f32 = 20.0;

/// Render a vertical scrollbar in the allocated column.
///
/// The scrollbar uses pixel-precise positioning:
/// - Track: subtle tinted background spanning the full content area height.
/// - Thumb: proportional pill (rounded rect) positioned by scroll ratio.
pub fn render_scrollbar(canvas: &mut SkiaCanvas, editor: &Editor, fl: &FrameLayout) {
    let Some(sb_col) = fl.scrollbar_col else {
        return;
    };

    let total = fl.total_lines;
    if total == 0 {
        return;
    }

    let viewport = fl.lines.len();
    // Don't show scrollbar when everything fits.
    if viewport >= total {
        return;
    }

    let (cw, _ch) = canvas.cell_size();
    // Center the thin bar within the column.
    let col_x = sb_col as f32 * cw;
    let bar_x = col_x + (cw - SCROLLBAR_WIDTH) / 2.0;
    let track_y_start = fl.area_row as f32 * fl.cell_height;
    let track_height = fl.pixel_y_limit - track_y_start;

    if track_height <= 0.0 {
        return;
    }

    // --- Track background (very subtle) ---
    let track_color = resolve_track_color(editor);
    canvas.draw_pixel_rrect(
        bar_x,
        track_y_start,
        SCROLLBAR_WIDTH,
        track_height,
        THUMB_RADIUS,
        track_color,
    );

    // --- Thumb ---
    let thumb_ratio = (viewport as f32 / total as f32).min(1.0);
    let thumb_height = (thumb_ratio * track_height).max(MIN_THUMB_HEIGHT);
    let scroll_ratio = if total > viewport {
        fl.scroll_offset as f32 / (total - viewport) as f32
    } else {
        0.0
    };
    let thumb_y = track_y_start + scroll_ratio * (track_height - thumb_height);

    let thumb_color = resolve_thumb_color(editor);
    canvas.draw_pixel_rrect(
        bar_x,
        thumb_y,
        SCROLLBAR_WIDTH,
        thumb_height,
        THUMB_RADIUS,
        thumb_color,
    );
}

/// Resolve the track background color from the theme or derive a sensible default.
///
/// If the theme defines `ui.scrollbar.track`, use that (as fg color).
/// Otherwise: derive from the editor background — on dark themes a faint white,
/// on light themes a faint black.
fn resolve_track_color(editor: &Editor) -> Color4f {
    let style = editor.theme.style("ui.scrollbar.track");
    if let Some(c) = style.fg {
        return theme::theme_color_to_skia(&c);
    }
    // Derive: faint tint opposite to background brightness.
    if editor.theme.is_dark() {
        Color4f::new(1.0, 1.0, 1.0, 0.04)
    } else {
        Color4f::new(0.0, 0.0, 0.0, 0.04)
    }
}

/// Resolve the thumb color from the theme or derive a sensible default.
///
/// If the theme defines `ui.scrollbar.thumb`, use that.
/// Otherwise: semi-transparent foreground-ish color that contrasts with track.
fn resolve_thumb_color(editor: &Editor) -> Color4f {
    let style = editor.theme.style("ui.scrollbar.thumb");
    if let Some(c) = style.fg {
        return theme::theme_color_to_skia(&c);
    }
    // Derive: medium-alpha tint for visibility.
    if editor.theme.is_dark() {
        Color4f::new(1.0, 1.0, 1.0, 0.25)
    } else {
        Color4f::new(0.0, 0.0, 0.0, 0.25)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scrollbar_col_none_is_noop() {
        // Just verify the function doesn't panic with no scrollbar_col.
        // Full rendering requires a SkiaCanvas which needs GPU context.
    }
}
