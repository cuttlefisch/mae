//! Shared math for rendering remote collaborators' cursors/selections
//! (principle #8 — shared computation, backend-specific drawing). Both
//! `mae-renderer` (ratatui cells) and `mae-gui` (Skia pixel rects) draw
//! remote selections and off-screen indicators from the same row/col
//! ranges and above/below classification computed here; only the actual
//! draw call differs per backend.

/// Normalize a (start, end) cursor pair into (start, end) with start <=
/// end lexicographically, so callers don't need to special-case a
/// selection made by dragging backwards.
pub fn normalize_selection_range(
    start: (usize, usize),
    end: (usize, usize),
) -> ((usize, usize), (usize, usize)) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

/// For one row spanned by a (normalized) selection, compute the visible
/// column range after subtracting the window's horizontal scroll offset.
///
/// `sr`/`er` are the normalized selection's start/end rows; `sc`/`ec` its
/// start/end columns. `line_len` is the target row's length (used as the
/// column end for rows strictly inside the selection). Returns
/// `(vis_start, vis_end)`; callers should skip drawing when
/// `vis_end <= vis_start`.
pub fn selection_col_range(
    row: usize,
    sr: usize,
    sc: usize,
    er: usize,
    ec: usize,
    line_len: usize,
    col_offset: usize,
) -> (usize, usize) {
    let col_start = if row == sr { sc } else { 0 };
    let col_end = if row == er { ec } else { line_len };
    let vis_start = col_start.saturating_sub(col_offset);
    let vis_end = col_end.saturating_sub(col_offset);
    (vis_start, vis_end)
}

/// Which edge of the viewport a remote cursor has scrolled past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffscreenSide {
    Above,
    Below,
}

/// Classify a remote cursor's row against the current (non-fold-aware)
/// viewport. Used directly by the TUI backend, and by the GUI backend's
/// fallback path when no `FrameLayout` is available (GUI additionally
/// consults fold-aware display-row lookup before falling back to this).
pub fn offscreen_side(
    cursor_row: usize,
    scroll_offset: usize,
    viewport_height: usize,
) -> Option<OffscreenSide> {
    if cursor_row < scroll_offset {
        Some(OffscreenSide::Above)
    } else if cursor_row >= scroll_offset + viewport_height {
        Some(OffscreenSide::Below)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_selection_range_keeps_forward_order() {
        let (s, e) = normalize_selection_range((2, 3), (5, 1));
        assert_eq!(s, (2, 3));
        assert_eq!(e, (5, 1));
    }

    #[test]
    fn normalize_selection_range_swaps_backward_drag() {
        let (s, e) = normalize_selection_range((5, 1), (2, 3));
        assert_eq!(s, (2, 3));
        assert_eq!(e, (5, 1));
    }

    #[test]
    fn normalize_selection_range_equal_points_unchanged() {
        let (s, e) = normalize_selection_range((3, 3), (3, 3));
        assert_eq!(s, (3, 3));
        assert_eq!(e, (3, 3));
    }

    #[test]
    fn selection_col_range_first_row_starts_at_selection_col() {
        // Single-row selection: sr == er == row.
        let (vs, ve) = selection_col_range(2, 2, 5, 2, 10, 20, 0);
        assert_eq!((vs, ve), (5, 10));
    }

    #[test]
    fn selection_col_range_middle_row_spans_full_line() {
        // Row strictly between sr and er: starts at 0, ends at line_len.
        let (vs, ve) = selection_col_range(3, 2, 5, 6, 2, 42, 0);
        assert_eq!((vs, ve), (0, 42));
    }

    #[test]
    fn selection_col_range_last_row_ends_at_selection_col() {
        let (vs, ve) = selection_col_range(6, 2, 5, 6, 2, 42, 0);
        assert_eq!((vs, ve), (0, 2));
    }

    #[test]
    fn selection_col_range_applies_col_offset() {
        let (vs, ve) = selection_col_range(2, 2, 15, 2, 20, 30, 10);
        assert_eq!((vs, ve), (5, 10));
    }

    #[test]
    fn selection_col_range_col_offset_past_start_saturates_to_zero() {
        // Adversarial: horizontal scroll past the selection start must
        // clamp to 0, not underflow.
        let (vs, ve) = selection_col_range(2, 2, 3, 2, 20, 30, 50);
        assert_eq!((vs, ve), (0, 0));
    }

    #[test]
    fn offscreen_side_above_when_before_scroll_offset() {
        assert_eq!(offscreen_side(2, 10, 20), Some(OffscreenSide::Above));
    }

    #[test]
    fn offscreen_side_below_when_past_viewport() {
        assert_eq!(offscreen_side(35, 10, 20), Some(OffscreenSide::Below));
    }

    #[test]
    fn offscreen_side_none_when_within_viewport() {
        assert_eq!(offscreen_side(15, 10, 20), None);
    }

    #[test]
    fn offscreen_side_boundaries_are_visible_not_offscreen() {
        // Adversarial: exact boundary rows (first/last visible row) must
        // NOT be classified as off-screen.
        assert_eq!(offscreen_side(10, 10, 20), None, "first visible row");
        assert_eq!(offscreen_side(29, 10, 20), None, "last visible row");
        assert_eq!(offscreen_side(30, 10, 20), Some(OffscreenSide::Below));
    }
}
