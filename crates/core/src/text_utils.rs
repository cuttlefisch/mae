//! Text display utilities: safe truncation, display width, which-key layout constants.
//!
//! @ai-caution: [which-key] All string truncation MUST use truncate_end() / truncate_start() —
//! never raw &s[..n] which panics on multi-byte chars. All position calculations MUST use
//! display_width() not .len() which counts bytes.
//!
//! ADR-087 Rule 2: `display_width` here is a re-export of `grapheme::display_width` (the
//! grapheme-cluster-aware implementation), not a second parallel one. A prior version of this
//! file defined its own `s.chars().map(|c| c.width().unwrap_or(0)).sum()`, which is wrong for
//! ZWJ sequences, emoji modifier sequences, presentation sequences, and several scripts'
//! ligatures — `unicode-width`'s own docs list these as cases where a string's width differs
//! from the sum of its characters' widths. `truncate_end`/`truncate_start` are rewritten over
//! `grapheme_indices(true)` for the same reason: cutting on `char_indices()` can land between a
//! ZWJ and its base character, or (for `truncate_start`, walking in reverse) accumulate a
//! combining mark's width before its base.

// ---------------------------------------------------------------------------
// Which-key layout constants (shared between TUI and GUI renderers)
// ---------------------------------------------------------------------------

/// Minimum column width for which-key popup layout (display columns).
pub const WK_COL_WIDTH_MIN: usize = 25;

/// Maximum column width for which-key popup layout (display columns).
pub const WK_COL_WIDTH_MAX: usize = 60;

/// Padding added to max entry width when computing column width.
pub const WK_COL_PADDING: usize = 2;

/// Fallback column width when there are no entries.
pub const WK_COL_WIDTH_FALLBACK: usize = 20;

/// Minimum remaining column space to display a doc string.
pub const WK_DOC_MIN_WIDTH: usize = 8;

/// Minimum popup height in rows (including borders).
pub const WK_MIN_HEIGHT: usize = 3;

/// Default maximum popup height as percentage of screen height.
pub const WK_MAX_HEIGHT_PCT_DEFAULT: usize = 40;
/// Minimum allowed value for the height percentage option.
pub const WK_MAX_HEIGHT_PCT_MIN: usize = 10;
/// Maximum allowed value for the height percentage option.
pub const WK_MAX_HEIGHT_PCT_MAX: usize = 90;

/// Breadcrumb separator between prefix keys in the popup title.
pub const WK_BREADCRUMB_SEP: &str = " > ";

/// Truncation suffix for label/doc strings.
pub const WK_TRUNCATION_SUFFIX: &str = "..";

// ---------------------------------------------------------------------------
// Key formatting (shared between TUI and GUI renderers)
// ---------------------------------------------------------------------------

/// Format a `KeyPress` for display in the which-key popup.
/// Shared implementation so TUI and GUI renderers produce identical strings.
pub fn format_keypress(kp: &crate::KeyPress) -> String {
    let mut s = String::new();
    if kp.ctrl {
        s.push_str("C-");
    }
    if kp.alt {
        s.push_str("M-");
    }
    match &kp.key {
        crate::Key::Char(' ') => s.push_str("SPC"),
        crate::Key::Char(c) => s.push(*c),
        crate::Key::Escape => s.push_str("Esc"),
        crate::Key::Enter => s.push_str("Enter"),
        crate::Key::Tab => s.push_str("Tab"),
        crate::Key::Backspace => s.push_str("BS"),
        crate::Key::Up => s.push_str("Up"),
        crate::Key::Down => s.push_str("Down"),
        crate::Key::Left => s.push_str("Left"),
        crate::Key::Right => s.push_str("Right"),
        crate::Key::F(n) => {
            s.push_str(&format!("F{}", n));
        }
        _ => s.push('?'),
    }
    s
}

/// Compute the column layout for which-key entries.
/// Returns `(col_width, num_cols)` — used by both TUI and GUI renderers
/// so the height calculation phase and render phase always agree.
pub fn which_key_column_layout(
    entries: &[crate::WhichKeyEntry],
    available_width: usize,
    separator_width: usize,
    max_desc: usize,
) -> (usize, usize) {
    let max_entry_w = entries
        .iter()
        .map(|e| {
            display_width(&format_keypress(&e.key))
                + separator_width
                + display_width(&e.label).min(max_desc)
        })
        .max()
        .unwrap_or(WK_COL_WIDTH_FALLBACK);
    let col_width = (max_entry_w + WK_COL_PADDING).clamp(WK_COL_WIDTH_MIN, WK_COL_WIDTH_MAX);
    let num_cols = (available_width / col_width).max(1);
    (col_width, num_cols)
}

// ---------------------------------------------------------------------------
// Display width helpers
// ---------------------------------------------------------------------------

/// Return the display width (terminal columns) of a string, under the
/// default width policy (narrow ambiguous width, 0-width control chars).
/// Multi-byte characters like `—` (em dash) are 1 column, CJK characters
/// are 2 columns, control chars are 0.
///
/// Re-exported from `grapheme::display_width` (ADR-087 Rule 2 / Rule 7) --
/// this module does not define its own width computation. Callers that need
/// a non-default policy (e.g. wide ambiguous width) should use
/// `crate::grapheme::display_width_with` directly.
pub use crate::grapheme::display_width;

/// Truncate `s` from the end, keeping at most `max_cols` display columns.
/// If truncation is needed, the last column is replaced with `…` (1 column),
/// so at most `max_cols` display columns are used.
/// Safe for multi-byte / wide characters — never slices mid-grapheme-cluster.
///
/// Cut points accumulate per-grapheme-cluster width (ADR-087 Rule 2), so a
/// family ZWJ emoji or a base+combining-mark pair is never split.
pub fn truncate_end(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let total = display_width(s);
    if total <= max_cols {
        return s.to_string();
    }
    let target = max_cols.saturating_sub(1); // reserve 1 col for '…'
    let byte_idx = crate::grapheme::byte_offset_for_max_width(s, target);
    let mut result = s[..byte_idx].to_string();
    result.push('…');
    result
}

/// Truncate `s` from the start, keeping the last `max_cols` display columns.
/// Prepends `…` if truncation occurs.
/// Safe for multi-byte / wide characters — never slices mid-grapheme-cluster.
pub fn truncate_start(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let total = display_width(s);
    if total <= max_cols {
        return s.to_string();
    }
    let target = max_cols.saturating_sub(1); // reserve 1 col for '…'
    let start = crate::grapheme::byte_offset_for_max_width_from_end(s, target);
    format!("…{}", &s[start..])
}

// ---------------------------------------------------------------------------
// Popup layout helpers (shared between TUI and GUI renderers)
// ---------------------------------------------------------------------------

/// Compute centered popup dimensions.
/// Returns `(width, height, x_offset, y_offset)`.
pub fn centered_popup_dims(
    area_width: usize,
    area_height: usize,
    width_pct: usize,
    height_pct: usize,
    min_width: usize,
    min_height: usize,
) -> (usize, usize, usize, usize) {
    let w = (area_width * width_pct / 100)
        .max(min_width)
        .min(area_width);
    let h = (area_height * height_pct / 100)
        .max(min_height)
        .min(area_height);
    let x = area_width.saturating_sub(w) / 2;
    let y = area_height.saturating_sub(h) / 2;
    (w, h, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_ascii() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn display_width_em_dash() {
        // '—' (U+2014 EM DASH) is 1 display column, 3 bytes
        assert_eq!(display_width("hello—world"), 11);
    }

    #[test]
    fn display_width_cjk() {
        // CJK ideographs are 2 columns each
        assert_eq!(display_width("日本語"), 6);
    }

    #[test]
    fn truncate_end_no_truncation() {
        assert_eq!(truncate_end("hello", 10), "hello");
    }

    #[test]
    fn truncate_end_ascii() {
        let result = truncate_end("hello world", 8);
        assert_eq!(display_width(&result), 8);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_end_em_dash() {
        // "AI Agent — terminal shell (SPC a a)" contains em dash at bytes 9..12
        let s = "AI Agent — terminal shell (SPC a a)";
        // Truncate at various widths — must never panic
        for width in 0..=40 {
            let result = truncate_end(s, width);
            assert!(display_width(&result) <= width);
        }
    }

    #[test]
    fn truncate_end_accented() {
        let s = "café résumé";
        for width in 0..=15 {
            let result = truncate_end(s, width);
            assert!(display_width(&result) <= width);
        }
    }

    #[test]
    fn truncate_end_emoji() {
        let s = "hello 🌍 world";
        for width in 0..=15 {
            let result = truncate_end(s, width);
            assert!(display_width(&result) <= width);
        }
    }

    #[test]
    fn truncate_end_arrow() {
        let s = "item → value";
        for width in 0..=15 {
            let result = truncate_end(s, width);
            assert!(display_width(&result) <= width);
        }
    }

    #[test]
    fn truncate_end_zero() {
        assert_eq!(truncate_end("hello", 0), "");
    }

    #[test]
    fn truncate_start_no_truncation() {
        assert_eq!(truncate_start("hello", 10), "hello");
    }

    #[test]
    fn truncate_start_ascii() {
        let result = truncate_start("hello world", 8);
        assert_eq!(display_width(&result), 8);
        assert!(result.starts_with('…'));
    }

    #[test]
    fn truncate_start_em_dash() {
        let s = "AI Agent — terminal shell";
        for width in 0..=30 {
            let result = truncate_start(s, width);
            assert!(display_width(&result) <= width);
        }
    }

    #[test]
    fn format_keypress_space() {
        let kp = crate::KeyPress {
            key: crate::Key::Char(' '),
            ctrl: false,
            alt: false,
            shift: false,
        };
        assert_eq!(format_keypress(&kp), "SPC");
    }

    #[test]
    fn format_keypress_ctrl_c() {
        let kp = crate::KeyPress {
            key: crate::Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert_eq!(format_keypress(&kp), "C-c");
    }

    #[test]
    fn format_keypress_function_key() {
        let kp = crate::KeyPress {
            key: crate::Key::F(5),
            ctrl: false,
            alt: false,
            shift: false,
        };
        assert_eq!(format_keypress(&kp), "F5");
    }

    #[test]
    fn which_key_column_layout_basic() {
        let entries = vec![
            crate::WhichKeyEntry {
                key: crate::KeyPress {
                    key: crate::Key::Char('a'),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
                label: "+ai".to_string(),
                is_group: true,
                doc: None,
            },
            crate::WhichKeyEntry {
                key: crate::KeyPress {
                    key: crate::Key::Char('b'),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
                label: "+buffer".to_string(),
                is_group: true,
                doc: None,
            },
        ];
        let (col_w, num_cols) = which_key_column_layout(&entries, 80, 1, 40);
        assert!(col_w >= WK_COL_WIDTH_MIN);
        assert!(col_w <= WK_COL_WIDTH_MAX);
        assert!(num_cols >= 1);
    }

    #[test]
    fn which_key_column_layout_narrow() {
        let entries = vec![crate::WhichKeyEntry {
            key: crate::KeyPress {
                key: crate::Key::Char('x'),
                ctrl: false,
                alt: false,
                shift: false,
            },
            label: "toggle-scratch".to_string(),
            is_group: false,
            doc: None,
        }];
        let (col_w, num_cols) = which_key_column_layout(&entries, 30, 1, 40);
        assert_eq!(num_cols, 1); // narrow width forces single column
        assert!(col_w <= 30);
    }

    #[test]
    fn which_key_column_layout_empty() {
        let entries: Vec<crate::WhichKeyEntry> = vec![];
        let (col_w, num_cols) = which_key_column_layout(&entries, 80, 1, 40);
        assert_eq!(col_w, WK_COL_WIDTH_MIN); // fallback clamped to min
        assert!(num_cols >= 1);
    }

    #[test]
    fn centered_popup_dims_basic() {
        let (w, h, x, y) = centered_popup_dims(100, 50, 70, 60, 40, 10);
        assert_eq!(w, 70);
        assert_eq!(h, 30);
        assert_eq!(x, 15);
        assert_eq!(y, 10);
    }

    #[test]
    fn centered_popup_dims_clamped_to_area() {
        let (w, h, _, _) = centered_popup_dims(35, 8, 70, 60, 40, 10);
        assert!(w <= 35);
        assert!(h <= 8);
    }

    #[test]
    fn centered_popup_dims_min_enforced() {
        let (w, h, _, _) = centered_popup_dims(100, 50, 1, 1, 40, 10);
        assert!(w >= 40);
        assert!(h >= 10);
    }
}
