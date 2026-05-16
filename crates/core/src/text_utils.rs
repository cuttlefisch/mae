//! Text display utilities: safe truncation, display width, which-key layout constants.
//!
//! @ai-caution: [which-key] All string truncation MUST use truncate_end() / truncate_start() —
//! never raw &s[..n] which panics on multi-byte chars. All position calculations MUST use
//! display_width() not .len() which counts bytes.

use unicode_width::UnicodeWidthChar;

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

/// Return the display width (terminal columns) of a string.
/// Multi-byte characters like `—` (em dash) are 1 column,
/// CJK characters are 2 columns, control chars are 0.
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Truncate `s` from the end, keeping at most `max_cols` display columns.
/// If truncation is needed, the last column is replaced with `…` (1 column),
/// so at most `max_cols` display columns are used.
/// Safe for multi-byte / wide characters — never slices mid-character.
pub fn truncate_end(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let total = display_width(s);
    if total <= max_cols {
        return s.to_string();
    }
    let target = max_cols.saturating_sub(1); // reserve 1 col for '…'
    let mut cols = 0;
    for (byte_idx, ch) in s.char_indices() {
        let w = ch.width().unwrap_or(0);
        if cols + w > target {
            let mut result = s[..byte_idx].to_string();
            result.push('…');
            return result;
        }
        cols += w;
    }
    // Shouldn't reach here given total > max_cols, but be safe
    s.to_string()
}

/// Truncate `s` from the start, keeping the last `max_cols` display columns.
/// Prepends `…` if truncation occurs.
/// Safe for multi-byte / wide characters.
pub fn truncate_start(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let total = display_width(s);
    if total <= max_cols {
        return s.to_string();
    }
    let target = max_cols.saturating_sub(1); // reserve 1 col for '…'
    let mut cols = 0;
    let mut start = s.len();
    for (i, ch) in s.char_indices().rev() {
        let w = ch.width().unwrap_or(0);
        if cols + w > target {
            break;
        }
        cols += w;
        start = i;
    }
    format!("…{}", &s[start..])
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
}
