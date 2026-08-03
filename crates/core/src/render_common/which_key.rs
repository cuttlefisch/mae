//! Shared which-key popup grid layout — column sizing, scroll clamping,
//! doc-string truncation, and per-cell padding.
//!
//! Previously ~110 lines of this were copy-pasted between
//! `crates/renderer/src/which_key_render.rs` and `crates/gui/src/popup_render.rs`,
//! flagged with an `@ai-caution` telling maintainers to hand-sync the two
//! copies on every change. That comment is the smell this module removes:
//! both renderers now call [`compute_which_key_layout`] and only do their
//! own platform-specific drawing (ratatui `Span`/`Line` for the TUI, direct
//! `SkiaCanvas` calls for the GUI).
//!
//! `which_key_column_layout` (column width / column count) was already
//! shared via `text_utils`; this module covers the rest of the grid: total
//! rows, scroll clamping, the above/below overflow indicators, and each
//! cell's key/label/doc truncation and padding.

use crate::keymap::WhichKeyEntry;
use crate::text_utils::{display_width, format_keypress, truncate_end, WK_DOC_MIN_WIDTH};

/// One rendered cell in the which-key grid, already truncated and measured —
/// backends only need to draw the strings at the given style/position.
#[derive(Debug, Clone)]
pub struct WhichKeyCell {
    /// Grid row, 0-based, *not* counting the "above" overflow indicator line.
    pub row: usize,
    pub col: usize,
    pub is_group: bool,
    pub key_text: String,
    pub key_width: usize,
    /// Truncated to fit the column.
    pub label_text: String,
    pub label_width: usize,
    /// Truncated to fit the remaining column space; `None` if there's no
    /// room (`< WK_DOC_MIN_WIDTH` remaining) or the entry has no doc.
    pub doc_text: Option<String>,
    /// Column offset (from the cell's column start) where the separator
    /// begins — i.e. `key_width`. Provided so backends don't need to
    /// recompute it from `key_text`.
    pub sep_offset: usize,
    /// Column offset where the label begins (`sep_offset + separator_width`).
    pub label_offset: usize,
    /// Column offset where the doc string begins (`label_offset +
    /// label_width + 1`), meaningful only when `doc_text.is_some()`.
    pub doc_offset: usize,
    /// Trailing spaces needed to pad this cell out to `col_width`. Only the
    /// TUI needs this explicitly (ratatui `Span`s concatenate with no
    /// gaps); the GUI positions absolutely and can ignore it.
    pub trailing_padding: usize,
}

/// Full computed layout for the which-key popup grid.
#[derive(Debug, Clone)]
pub struct WhichKeyLayout {
    pub col_width: usize,
    pub num_cols: usize,
    /// Number of populated grid rows (not counting above/below indicators).
    pub grid_rows: usize,
    pub cells: Vec<WhichKeyCell>,
    /// Count of entries scrolled off above, if any.
    pub above_count: Option<usize>,
    /// Count of entries scrolled off below, if any.
    pub below_count: Option<usize>,
}

/// Compute the full which-key grid layout: column sizing (reusing
/// `which_key_column_layout`), scroll clamping, above/below indicators, and
/// each visible entry's truncated key/label/doc strings.
///
/// `inner_width`/`inner_height` are the popup's content area (inside its
/// border); `sep_width` is `display_width(separator)`, already computed by
/// the caller (both backends need it before this call anyway, to size the
/// popup itself); `requested_scroll` is `editor.which_key_scroll`, clamped
/// here to the last valid page.
pub fn compute_which_key_layout(
    entries: &[WhichKeyEntry],
    inner_width: usize,
    inner_height: usize,
    sep_width: usize,
    max_desc: usize,
    requested_scroll: usize,
) -> WhichKeyLayout {
    let (col_width, num_cols) =
        crate::text_utils::which_key_column_layout(entries, inner_width, sep_width, max_desc);

    let total_rows = entries.len().div_ceil(num_cols.max(1));
    let max_scroll = total_rows.saturating_sub(inner_height);
    let scroll = requested_scroll.min(max_scroll);

    let skip_entries = scroll * num_cols;
    let show_above = scroll > 0;
    let show_below = total_rows > scroll + inner_height;

    let effective_max_rows = if show_above && show_below {
        inner_height.saturating_sub(2)
    } else if show_above || show_below {
        inner_height.saturating_sub(1)
    } else {
        inner_height
    };

    let visible_entries = entries.get(skip_entries..).unwrap_or(&[]);
    let mut cells = Vec::new();
    let mut row = 0usize;
    let mut col = 0usize;
    let mut displayed = 0usize;

    for entry in visible_entries {
        if row >= effective_max_rows {
            break;
        }

        let key_text = format_keypress(&entry.key);
        let key_width = display_width(&key_text);

        let max_label = col_width.saturating_sub(key_width + sep_width + 1);
        let label_width_raw = display_width(&entry.label);
        let label_text = if label_width_raw > max_label {
            truncate_end(&entry.label, max_label)
        } else {
            entry.label.clone()
        };
        let label_width = display_width(&label_text);

        let used = key_width + sep_width + label_width;

        let mut doc_text = None;
        let mut doc_total_width = 0usize;
        if !entry.is_group {
            if let Some(ref doc) = entry.doc {
                let remaining = col_width.saturating_sub(used + 2);
                if remaining > WK_DOC_MIN_WIDTH {
                    let trunc = truncate_end(doc, remaining);
                    doc_total_width = 1 + display_width(&trunc); // 1-col gap + content
                    doc_text = Some(trunc);
                }
            }
        }

        let sep_offset = key_width;
        let label_offset = sep_offset + sep_width;
        let doc_offset = label_offset + label_width + 1;
        let trailing_padding = col_width.saturating_sub(used + doc_total_width);

        cells.push(WhichKeyCell {
            row,
            col,
            is_group: entry.is_group,
            key_text,
            key_width,
            label_text,
            label_width,
            doc_text,
            sep_offset,
            label_offset,
            doc_offset,
            trailing_padding,
        });

        displayed += 1;
        col += 1;
        if col >= num_cols {
            col = 0;
            row += 1;
        }
    }
    // Count the final partial row if any cells were placed in it.
    let grid_rows = if col > 0 { row + 1 } else { row };

    WhichKeyLayout {
        col_width,
        num_cols,
        grid_rows,
        cells,
        above_count: show_above.then_some(skip_entries),
        below_count: show_below
            .then(|| entries.len() - skip_entries - displayed)
            .filter(|n| *n > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, KeyPress};

    fn entry(key: char, label: &str, is_group: bool, doc: Option<&str>) -> WhichKeyEntry {
        WhichKeyEntry {
            key: KeyPress {
                key: Key::Char(key),
                ctrl: false,
                alt: false,
                shift: false,
            },
            label: label.to_string(),
            is_group,
            doc: doc.map(|s| s.to_string()),
        }
    }

    #[test]
    fn basic_grid_no_overflow() {
        let entries = vec![
            entry('a', "save", false, Some("save the buffer")),
            entry('b', "quit", false, None),
        ];
        let layout = compute_which_key_layout(&entries, 80, 24, 1, 40, 0);
        assert_eq!(layout.cells.len(), 2);
        assert!(layout.above_count.is_none());
        assert!(layout.below_count.is_none());
        assert_eq!(layout.cells[0].key_text, "a");
        assert_eq!(layout.cells[0].label_text, "save");
        assert_eq!(layout.cells[0].doc_text.as_deref(), Some("save the buffer"));
    }

    #[test]
    fn doc_omitted_for_group_entries() {
        let entries = vec![entry('a', "group", true, Some("should not show"))];
        let layout = compute_which_key_layout(&entries, 80, 24, 1, 40, 0);
        assert!(layout.cells[0].doc_text.is_none());
    }

    #[test]
    fn label_truncated_when_column_too_narrow() {
        let entries = vec![entry(
            'a',
            "a very long label that will not fit in a narrow column",
            false,
            None,
        )];
        // Force a narrow column via a tiny max_desc so column width is small.
        let layout = compute_which_key_layout(&entries, 80, 24, 1, 10, 0);
        assert!(display_width(&layout.cells[0].label_text) <= layout.col_width);
    }

    #[test]
    fn scroll_clamped_to_last_page() {
        let entries: Vec<_> = (0..20)
            .map(|i| entry((b'a' + (i % 26) as u8) as char, "x", false, None))
            .collect();
        // inner_height=2 rows, num_cols will be small -> total_rows > 2
        let layout = compute_which_key_layout(&entries, 20, 2, 1, 5, 9_999);
        // Whatever scroll ended up clamped to, we must not have skipped past
        // the point where zero entries remain to render.
        assert!(
            !layout.cells.is_empty(),
            "clamped scroll must still show entries"
        );
    }

    #[test]
    fn above_and_below_indicators_shrink_grid_rows() {
        let entries: Vec<_> = (0..30)
            .map(|i| entry((b'a' + (i % 26) as u8) as char, "x", false, None))
            .collect();
        // 1 column (narrow width), 5 visible rows, scrolled to the middle.
        let layout = compute_which_key_layout(&entries, 10, 5, 1, 3, 2);
        assert!(layout.above_count.is_some());
        assert!(layout.below_count.is_some());
        // Both indicators shown -> effective grid rows <= inner_height - 2.
        assert!(layout.grid_rows <= 3);
    }

    #[test]
    fn non_ascii_key_and_label_width_is_display_width() {
        // A CJK label must be measured/truncated by display columns, not
        // byte or char count (ADR-087).
        let entries = vec![entry('a', "日本語ラベルテキスト", false, None)];
        let layout = compute_which_key_layout(&entries, 12, 24, 1, 40, 0);
        let cell = &layout.cells[0];
        assert!(
            cell.label_width <= layout.col_width,
            "label width {} must fit column width {}",
            cell.label_width,
            layout.col_width
        );
    }

    #[test]
    fn zwj_emoji_label_does_not_panic_and_fits_budget() {
        // Family ZWJ emoji: a naive per-char width sum would wildly
        // overcount and could also cut mid-cluster on truncation.
        let zwj_family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let entries = vec![entry('a', zwj_family, false, None)];
        let layout = compute_which_key_layout(&entries, 15, 24, 1, 40, 0);
        let cell = &layout.cells[0];
        assert!(cell.label_width <= layout.col_width);
    }
}
