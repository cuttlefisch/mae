//! # MiniDialog geometry — single source of truth (shared by GUI + TUI)
//!
//! The blocking/confirm `MiniDialogState` overlay (host-key TOFU prompt, discard
//! confirms, rename/setup inputs, …) was drawn by each backend with a **hard-coded,
//! content-blind** box: `width = 50`, `height = 4 + fields.len()`. Long content —
//! notably the ~55-char `SHA256:…` host-key fingerprint, which must be fully
//! readable for the out-of-band trust compare — overflowed and was **clipped**
//! (B-23). The two backends duplicated the same formula, the geometry twin of the
//! overlay-priority duplication fixed via [`super::overlay`].
//!
//! [`mini_dialog_layout`] computes the box geometry AND the wrapped content lines
//! once; both `render_mini_dialog` implementations consume it, so the dialog grows
//! to its content (clamped to the screen, wrapping when it must) and can't truncate
//! differently per backend. Unit-tested like the overlay resolver.

use crate::command_palette::MiniDialogState;
use crate::text_utils::display_width;

/// Keep the dialog this many cells off each screen edge.
const SCREEN_MARGIN: usize = 2;
/// Border (1) + inner padding (1) on each side = 4 cells of horizontal chrome.
const H_CHROME: usize = 4;
/// A sane minimum inner width so tiny prompts still look like a dialog.
const MIN_INNER: usize = 24;

/// One drawable row inside the dialog border, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogLine {
    /// Display-only text (a wrapped confirm/notification body line).
    Text(String),
    /// An editable field row: `label` + current `value`/`placeholder`, possibly the
    /// active (cursor) field. The backend draws the value + cursor itself.
    Field {
        index: usize,
        label: String,
        value: String,
        placeholder: String,
        active: bool,
    },
    /// A dim footer hint line (input dialogs).
    Hint(String),
}

/// Computed dialog box geometry + the rows to draw inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogLayout {
    /// Top-left corner (centered on screen), in cells.
    pub col: usize,
    pub row: usize,
    /// Outer box size (includes the border), clamped to the screen.
    pub width: usize,
    pub height: usize,
    /// Content rows to draw inside the border, top to bottom.
    pub lines: Vec<DialogLine>,
}

impl DialogLayout {
    /// Inner content width (box minus border + padding).
    pub fn inner_width(&self) -> usize {
        self.width.saturating_sub(H_CHROME)
    }
}

/// Compute a content-adaptive layout for `dialog` within a `max_cols` × `max_rows`
/// screen. The box grows to fit the title, body, and fields, wrapping long content
/// (word-wrap, hard-breaking over-long tokens like a fingerprint) and clamping to
/// the screen so nothing is silently clipped.
pub fn mini_dialog_layout(
    dialog: &MiniDialogState,
    max_cols: usize,
    max_rows: usize,
) -> DialogLayout {
    let avail_cols = max_cols.saturating_sub(SCREEN_MARGIN * 2);
    let max_inner = avail_cols.saturating_sub(H_CHROME).max(1);

    // Desired inner width = widest piece of content (pre-wrap), clamped to the
    // screen. Title sits in the border, so budget its width too.
    let title_w = display_width(dialog.title()) + 2; // padded " title "
    let confirm = dialog.is_confirm();

    let mut desired = title_w;
    if confirm {
        for field in &dialog.fields {
            for seg in field.label.split('\n') {
                desired = desired.max(display_width(seg));
            }
        }
    } else {
        for field in &dialog.fields {
            let shown = if field.value.is_empty() {
                &field.placeholder
            } else {
                &field.value
            };
            desired = desired.max(display_width(&field.label) + 2 + display_width(shown));
        }
        desired = desired.max(display_width(INPUT_HINT));
    }
    let inner = desired.clamp(MIN_INNER.min(max_inner), max_inner);

    // Build the rows, wrapping confirm body lines to the final inner width.
    let mut lines: Vec<DialogLine> = Vec::new();
    if confirm {
        for field in &dialog.fields {
            for seg in field.label.split('\n') {
                for wrapped in wrap_hard(seg, inner) {
                    lines.push(DialogLine::Text(wrapped));
                }
            }
        }
    } else {
        for (index, field) in dialog.fields.iter().enumerate() {
            lines.push(DialogLine::Field {
                index,
                label: field.label.clone(),
                value: field.value.clone(),
                placeholder: field.placeholder.clone(),
                active: index == dialog.active_field,
            });
        }
        lines.push(DialogLine::Hint(INPUT_HINT.to_string()));
    }

    let width = (inner + H_CHROME).min(avail_cols).max(H_CHROME + 1);
    // Height: top+bottom border + content rows, clamped to the screen.
    let max_inner_rows = max_rows
        .saturating_sub(SCREEN_MARGIN * 2)
        .saturating_sub(2)
        .max(1);
    if lines.len() > max_inner_rows {
        lines.truncate(max_inner_rows);
    }
    let height = lines.len() + 2;

    let col = max_cols.saturating_sub(width) / 2;
    let row = max_rows.saturating_sub(height) / 2;

    DialogLayout {
        col,
        row,
        width,
        height,
        lines,
    }
}

/// Footer hint for input dialogs.
pub const INPUT_HINT: &str = "Tab: next  Enter: apply  Esc: cancel";

/// Word-wrap `s` to `width` display columns, hard-breaking any single token wider
/// than `width` (so a space-less fingerprint still wraps instead of overflowing).
pub fn wrap_hard(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![s.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;

    let flush = |cur: &mut String, cur_w: &mut usize, out: &mut Vec<String>| {
        out.push(std::mem::take(cur));
        *cur_w = 0;
    };

    for word in s.split(' ') {
        let ww = display_width(word);
        if ww > width {
            // Token longer than the line — hard-break it across chunks.
            if !cur.is_empty() {
                flush(&mut cur, &mut cur_w, &mut out);
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for ch in word.chars() {
                let cw = display_width(ch.encode_utf8(&mut [0u8; 4]));
                if chunk_w + cw > width && !chunk.is_empty() {
                    out.push(std::mem::take(&mut chunk));
                    chunk_w = 0;
                }
                chunk.push(ch);
                chunk_w += cw;
            }
            cur = chunk;
            cur_w = chunk_w;
        } else {
            let sep = usize::from(!cur.is_empty());
            if cur_w + sep + ww > width {
                flush(&mut cur, &mut cur_w, &mut out);
                cur.push_str(word);
                cur_w = ww;
            } else {
                if sep == 1 {
                    cur.push(' ');
                    cur_w += 1;
                }
                cur.push_str(word);
                cur_w += ww;
            }
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::{MiniDialogContext, MiniDialogState};

    fn tofu_dialog() -> MiniDialogState {
        // What the host-key TOFU prompt actually builds (notify Modal arm).
        let q = "Trust daemon at 192.168.1.137:9480?\n  \
                 SHA256:07aWfiNGm690ZcPzxEWvCSTYgkIz+Dw7Db0RPOKK7Ls  (first connect — accept & pin?)\n[y/N]";
        MiniDialogState::confirm(q, MiniDialogContext::Notification { notif_id: 1 })
    }

    #[test]
    fn fingerprint_is_fully_visible_not_clipped() {
        let d = tofu_dialog();
        let layout = mini_dialog_layout(&d, 120, 40);
        let body: String = layout
            .lines
            .iter()
            .filter_map(|l| match l {
                DialogLine::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The full fingerprint survives (the old fixed-50 box clipped it).
        assert!(
            body.contains("SHA256:07aWfiNGm690ZcPzxEWvCSTYgkIz+Dw7Db0RPOKK7Ls"),
            "full fingerprint must be present + readable; got:\n{body}"
        );
        // Box grew past the old fixed 50 to fit the ~70-col line, but stays on screen.
        assert!(
            layout.width > 50,
            "box grows to content (was hard-coded 50)"
        );
        assert!(
            layout.width <= 120 - 2,
            "box stays within the screen margin"
        );
    }

    #[test]
    fn wraps_when_screen_is_narrow() {
        let d = tofu_dialog();
        let layout = mini_dialog_layout(&d, 40, 30);
        assert!(layout.width <= 40 - 2, "clamped to a narrow screen");
        // The fingerprint token is hard-broken across lines but every char survives.
        let joined: String = layout
            .lines
            .iter()
            .filter_map(|l| match l {
                DialogLine::Text(t) => Some(t.replace(' ', "")),
                _ => None,
            })
            .collect();
        assert!(joined.contains("07aWfiNGm690ZcPzxEWvCSTYgkIz"));
        assert!(layout.lines.len() > 3, "long body wrapped to multiple rows");
    }

    #[test]
    fn wrap_hard_breaks_overlong_token() {
        let lines = wrap_hard("aaaaaaaaaa", 4);
        assert_eq!(lines, vec!["aaaa", "aaaa", "aa"]);
        // Word boundaries preferred when they fit.
        assert_eq!(wrap_hard("ab cd ef", 5), vec!["ab cd", "ef"]);
    }

    #[test]
    fn input_dialog_has_field_rows_and_hint() {
        let d = MiniDialogState::single_input(
            "New name",
            "value",
            "placeholder",
            MiniDialogContext::FileSaveAs,
        );
        let layout = mini_dialog_layout(&d, 100, 30);
        assert!(matches!(
            layout.lines.first(),
            Some(DialogLine::Field { .. })
        ));
        assert!(matches!(layout.lines.last(), Some(DialogLine::Hint(_))));
    }
}
