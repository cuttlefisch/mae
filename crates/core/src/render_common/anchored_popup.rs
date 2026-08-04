//! Shared geometry for anchor-positioned popups (LSP hover, KB-link preview).
//!
//! Both backends render two popups that are structurally identical: they
//! open below a saved `(anchor_row, anchor_col)` position, flip above when
//! there isn't room below, size themselves to their (already word-wrapped)
//! content, and clamp into the available area. Before this module, that
//! math was copy-pasted four times (TUI hover, TUI KB-preview, GUI hover, GUI
//! KB-preview) — this collapsed a real bug: the TUI hover popup positioned
//! off the *live cursor* (`win.cursor_row`/`cursor_col`) instead of the
//! popup's saved anchor, so a popup could visibly jump if the cursor moved
//! (e.g. via scroll) before the next paint. The TUI KB-preview popup and
//! both GUI popups already used the anchor correctly; this module makes
//! that the only implementation, so the bug cannot reappear in one copy
//! while being fixed in another.
//!
//! ADR-087: `popup_size` measures line width in **display columns**
//! (`display_width`), not bytes/chars — both prior copies used `str::len()`
//! (byte length), which undercounts width for any wrapped line containing
//! CJK or other wide characters, making the popup box too narrow.

use crate::grapheme::display_width;

/// Computed box dimensions for an anchored popup, including its border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupSize {
    pub width: usize,
    pub height: usize,
}

/// Computed absolute position for an anchored popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupPosition {
    pub top: usize,
    pub left: usize,
}

/// Compute `(width, height)` for a popup box from its (already wrapped)
/// content lines, including a 1-cell border on every side.
///
/// `visible_count` is the number of lines actually shown at once (after
/// scrolling); `max_width_cap` bounds the box width (e.g. 76 cols in the
/// TUI, a dynamic screen-relative cap in the GUI); `area_height` bounds the
/// box height so it never exceeds the drawable area.
pub fn popup_size(
    lines: &[String],
    visible_count: usize,
    max_width_cap: usize,
    area_height: usize,
) -> PopupSize {
    let width = lines
        .iter()
        .take(visible_count)
        .map(|l| display_width(l))
        .max()
        .unwrap_or(20)
        .min(max_width_cap)
        + 2; // border
    let height = (visible_count + 2).min(area_height.saturating_sub(2));
    PopupSize { width, height }
}

/// Compute the popup's absolute `(top, left)` position.
///
/// `anchor_row`/`anchor_col` are **window-relative** (relative to the
/// focused window's own content, before any split offset). `win_row_offset`/
/// `win_col_offset` fold in that split offset to get an absolute position
/// within `area`; the GUI (which draws multiple windows into one canvas)
/// passes the focused window's screen offset here, while the TUI (which
/// gets one `Rect` per widget, already positioned by ratatui) passes zero.
///
/// `flip_height` is the height used for the below/above room decision —
/// the *focused window's* height, so a popup in a small split doesn't
/// assume it can use the whole screen below the anchor. `area_height` is
/// the height used only for the final clamp, keeping the popup inside the
/// drawable area even if `flip_height` and `area_height` differ (a split
/// window is shorter than the full screen).
///
/// Placed below the anchor with a 1-line gap so the anchor line stays
/// visible; flips above when there isn't room below; horizontally clamped
/// so the popup never runs past the right edge of `area`.
#[allow(clippy::too_many_arguments)]
pub fn popup_position(
    anchor_row: usize,
    anchor_col: usize,
    size: PopupSize,
    win_row_offset: usize,
    win_col_offset: usize,
    flip_height: usize,
    area_x: usize,
    area_y: usize,
    area_width: usize,
    area_height: usize,
) -> PopupPosition {
    let abs_anchor_row = area_y + win_row_offset + anchor_row;
    let top = if anchor_row + 2 + size.height < flip_height {
        abs_anchor_row + 2
    } else if anchor_row > size.height {
        abs_anchor_row.saturating_sub(size.height + 1)
    } else {
        abs_anchor_row.saturating_sub(size.height)
    };
    // Safety clamp: the branches above are already bounded relative to
    // `flip_height`, but this keeps a stale/out-of-range anchor (e.g. after
    // a big scroll) from ever placing the popup outside the drawable area.
    let top = top.clamp(area_y, area_y + area_height.saturating_sub(size.height));

    let abs_anchor_col = area_x + win_col_offset + anchor_col;
    let left = abs_anchor_col.min(area_x + area_width.saturating_sub(size.width));

    PopupPosition { top, left }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn size_uses_display_width_not_bytes() {
        // "日本語" is 3 chars / 9 bytes / 6 display columns. A byte-length
        // implementation would size the box for 9 cols; this must use 6.
        let ls = lines(&["日本語"]);
        let size = popup_size(&ls, 1, 76, 24);
        assert_eq!(size.width, 6 + 2, "must size by display width, not bytes");
    }

    #[test]
    fn size_caps_and_borders() {
        let ls = lines(&["a very long line that exceeds the cap by a lot indeed"]);
        let size = popup_size(&ls, 1, 20, 24);
        assert_eq!(size.width, 20 + 2);
    }

    #[test]
    fn size_height_capped_by_area() {
        let ls = lines(&["a", "b", "c", "d", "e"]);
        let size = popup_size(&ls, 5, 76, 5); // area_height=5 -> height capped to 3
        assert_eq!(size.height, 3);
    }

    #[test]
    fn position_below_anchor_when_room() {
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let pos = popup_position(2, 3, size, 0, 0, 24, 0, 0, 80, 24);
        assert_eq!(pos.top, 4); // anchor_row + 2
        assert_eq!(pos.left, 3);
    }

    #[test]
    fn position_flips_above_when_no_room_below() {
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        // Anchor near the bottom of a 24-row area — no room for +2+height below.
        let pos = popup_position(22, 3, size, 0, 0, 24, 0, 0, 80, 24);
        assert!(pos.top < 22, "must flip above the anchor");
    }

    #[test]
    fn position_horizontal_clamp_to_area() {
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let pos = popup_position(2, 78, size, 0, 0, 24, 0, 0, 80, 24);
        assert_eq!(pos.left, 70, "must clamp so popup_width fits in area_width");
    }

    #[test]
    fn position_folds_area_origin() {
        // TUI-style non-zero area origin (a Rect not starting at 0,0).
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let pos = popup_position(2, 3, size, 0, 0, 24, 5, 5, 80, 24);
        assert_eq!(pos.top, 5 + 4);
        assert_eq!(pos.left, 5 + 3);
    }

    #[test]
    fn position_folds_window_split_offset() {
        // GUI-style: a focused window drawn at a nonzero screen offset
        // within one shared canvas (a split window). `win_row_offset`/
        // `win_col_offset` must shift the result without disturbing the
        // window-relative flip decision (still based on the un-offset
        // `anchor_row` vs. `flip_height`, the window's own height).
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let pos = popup_position(2, 3, size, 10, 20, 24, 0, 0, 80, 60);
        assert_eq!(pos.top, 10 + 2 + 2); // win_row_offset + anchor_row + 2
        assert_eq!(pos.left, 20 + 3);
    }

    #[test]
    fn position_flip_decision_uses_window_height_not_area_height() {
        // A short split window (flip_height=10) near its own bottom must
        // flip above even though the full canvas (area_height=60) has
        // plenty of room further down the screen.
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let win_row_offset = 30;
        let anchor_row = 8; // window-relative; near the bottom of a 10-row window
        let pos = popup_position(anchor_row, 3, size, win_row_offset, 0, 10, 0, 0, 80, 60);
        assert!(
            pos.top < win_row_offset + anchor_row,
            "must flip above using the window's own height, not the full canvas height"
        );
    }

    #[test]
    fn position_never_escapes_area_vertically() {
        // A pathological anchor far past the area (e.g. stale anchor after
        // a big scroll) must still clamp inside, never panic or overflow.
        let size = PopupSize {
            width: 10,
            height: 5,
        };
        let pos = popup_position(9_000, 3, size, 0, 0, 24, 0, 0, 80, 24);
        assert!(pos.top + size.height <= 24);
    }
}
