use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ---------------------------------------------------------------------------
// ADR-087 Rule 7: mae-core is the sole owner of the width/segmentation call.
// No other crate imports `unicode_width`/`unicode_segmentation` directly —
// they go through the free functions in this module (or `text_utils`, which
// re-exports this module's `display_width`). This is what makes Rule 3's
// width policy enforceable from one place instead of N call sites each
// picking their own default.
// ---------------------------------------------------------------------------

/// Count grapheme clusters in a string slice.
///
/// This is the correct unit for cursor movement — one "move right"
/// should advance by one grapheme, not one char. A grapheme cluster
/// may contain multiple chars (e.g., emoji ZWJ sequences, combining marks).
pub fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}

// ---------------------------------------------------------------------------
// Width policy (ADR-087 Rule 3, CLAUDE.md principle #7)
// ---------------------------------------------------------------------------
//
// Display width is not a property of text alone (ADR-087's central
// constraint) — for two specific classes of code point, the "right" answer
// depends on a policy choice MAE cannot make on the user's behalf:
//
//   - East_Asian_Width=Ambiguous code points (box-drawing, Greek, Cyrillic
//     subsets, etc.) are 1 column in a non-CJK context and 2 in a CJK one.
//     `unicode-width` calls these `width()` (narrow) and `width_cjk()`
//     (wide) respectively.
//   - Control characters (U+0000..=U+001F, U+007F..=U+009F) have no
//     Unicode-defined display width at all -- `unicode-width` returns
//     `None`. MAE previously collapsed this to a silent `.unwrap_or(0)`.
//
// Both are registered as real OptionRegistry options (`ambiguous_width`,
// `control_char_width` in `options.rs`) rather than baked-in constants.

/// Resolved width-computation policy. Build via `WidthPolicy::default()`
/// (narrow ambiguous width, 0-width control chars -- matches MAE's prior,
/// implicit behavior) or from the live editor options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidthPolicy {
    /// Resolve East_Asian_Width=Ambiguous code points as 2 columns (CJK
    /// convention) instead of the default 1 column (non-CJK convention).
    pub ambiguous_wide: bool,
    /// Display width assigned to a control character, which
    /// `unicode-width` reports as `None` (undefined) rather than a number.
    pub control_char_width: usize,
}

impl Default for WidthPolicy {
    fn default() -> Self {
        WidthPolicy {
            ambiguous_wide: false,
            control_char_width: 0,
        }
    }
}

/// Width of a single character under `policy`. Returns `policy.control_char_width`
/// for control characters, which `unicode-width` itself leaves undefined.
///
/// **Do not assume the result is <= 2.** U+17D8 (Khmer Sign Beyyal) is 3
/// columns wide; treating width as always 0/1/2 is a documented ADR-087 bug
/// class (`grep -n '_ => 2'` for the anti-pattern this guards against).
pub fn char_width_with(c: char, policy: WidthPolicy) -> usize {
    let w = if policy.ambiguous_wide {
        UnicodeWidthChar::width_cjk(c)
    } else {
        UnicodeWidthChar::width(c)
    };
    w.unwrap_or(policy.control_char_width)
}

/// Width of a single character under a caller-supplied fallback for control
/// characters, bypassing the `WidthPolicy`/OptionRegistry default.
///
/// Reserved for call sites where the fallback is dictated by a *rendering*
/// invariant rather than a user-facing policy choice -- e.g. glyph-by-glyph
/// GUI cell advance, where a fallback of 0 would draw two glyphs on top of
/// each other regardless of what the user's `control_char_width` option
/// says. This is intentionally narrow; prefer `char_width_with` /
/// `display_width_with` everywhere else so the option has one meaning.
pub fn char_width_or(c: char, fallback: usize) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(fallback)
}

/// Get display width of a string under the default policy (narrow ambiguous
/// width, 0-width control chars) -- accounts for CJK, emoji, combining
/// marks, ZWJ sequences, and other multi-char clusters whose width differs
/// from the sum of their characters' widths (see `unicode-width`'s own
/// docs). This is the correct unit for screen column positioning: a CJK
/// character is 2 cells wide, a combining mark is 0 cells wide, ASCII is 1
/// cell wide.
///
/// Note: undefined for strings containing a tab -- a tab's width depends on
/// the column it starts at, which this function does not take. Callers with
/// tabs must expand them first (see `display_region::compute_tab_regions`).
pub fn display_width(s: &str) -> usize {
    display_width_with(s, WidthPolicy::default())
}

/// `display_width`, parameterized by `WidthPolicy` (ADR-087 Rule 3).
///
/// When `policy.control_char_width == 0` this delegates to
/// `unicode-width`'s whole-string algorithm directly, which is both faster
/// and *more* correct than summing per-grapheme widths: it tracks state
/// across the whole string for cases like `"\r\n"` (width 1, not 2) and
/// regional-indicator pairing that a per-cluster walk would not preserve
/// across a cluster boundary. Only when a non-default `control_char_width`
/// requires overriding what `unicode-width` bakes in for control chars do
/// we fall back to a per-grapheme-cluster walk (still correct for ZWJ
/// sequences, since those bind within a single cluster).
pub fn display_width_with(s: &str, policy: WidthPolicy) -> usize {
    if policy.control_char_width == 0 {
        return if policy.ambiguous_wide {
            UnicodeWidthStr::width_cjk(s)
        } else {
            UnicodeWidthStr::width(s)
        };
    }
    s.graphemes(true).map(|g| grapheme_width(g, policy)).sum()
}

/// Width of one grapheme cluster under `policy`. A cluster that is a single
/// control character honors `policy.control_char_width`; anything else
/// (including a multi-char cluster like a ZWJ sequence or CRLF) goes
/// through `unicode-width`'s own per-cluster algorithm, which is exact
/// within a cluster boundary.
fn grapheme_width(g: &str, policy: WidthPolicy) -> usize {
    let mut chars = g.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let w = if policy.ambiguous_wide {
            UnicodeWidthChar::width_cjk(c)
        } else {
            UnicodeWidthChar::width(c)
        };
        return w.unwrap_or(policy.control_char_width);
    }
    if policy.ambiguous_wide {
        UnicodeWidthStr::width_cjk(g)
    } else {
        UnicodeWidthStr::width(g)
    }
}

/// Convert a grapheme index to a byte offset in the string.
///
/// Returns the byte offset of the start of grapheme at `grapheme_idx`,
/// or the string length if `grapheme_idx >= grapheme_count(s)`.
pub fn grapheme_to_byte_offset(s: &str, grapheme_idx: usize) -> usize {
    s.grapheme_indices(true)
        .nth(grapheme_idx)
        .map(|(byte_off, _)| byte_off)
        .unwrap_or(s.len())
}

/// Convert a grapheme index to a char offset in the string.
///
/// Returns the char offset corresponding to the start of the grapheme
/// at `grapheme_idx`, or total char count if out of bounds.
pub fn grapheme_to_char_offset(s: &str, grapheme_idx: usize) -> usize {
    let byte_off = grapheme_to_byte_offset(s, grapheme_idx);
    s[..byte_off].chars().count()
}

/// Get the display width of the first `grapheme_idx` graphemes of a string.
///
/// Used by the renderer to convert a cursor column (grapheme index) to
/// a screen column (display width).
pub fn display_width_up_to_grapheme(s: &str, grapheme_idx: usize) -> usize {
    s.graphemes(true)
        .take(grapheme_idx)
        .map(UnicodeWidthStr::width)
        .sum()
}

// ---------------------------------------------------------------------------
// ADR-087 chokepoint validator (enforcement item 3)
// ---------------------------------------------------------------------------

/// Every offset -> slice conversion in `grapheme`/`text_utils` passes
/// through this. In debug builds an invalid boundary is a bug we want
/// caught immediately -- `debug_assert!` panics with a message naming the
/// offset and string length. In release builds we cannot justify crashing
/// the whole editor over a display computation, so we clamp to the nearest
/// valid `char` boundary at or before `byte_idx` and log it, rather than
/// let `&s[..byte_idx]` panic.
///
/// This is the ~30-line mechanism ADR-087 calls out as covering "the whole
/// panic class" instead of a type-system refactor (rejected -- see the
/// ADR's newtype discussion): every `truncate_end`/`truncate_start`/
/// `byte_offset_for_max_width*` cut point is produced by walking
/// `grapheme_indices`, so it should always already be on both a grapheme
/// and a char boundary; this function is the last line of defense for a
/// caller that constructs a byte offset some other way (e.g. arithmetic on
/// `.len()`, the exact bug class fixed at the `popup_render.rs` call
/// sites).
pub fn checked_byte_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx <= s.len() && s.is_char_boundary(byte_idx) {
        return byte_idx;
    }
    debug_assert!(
        false,
        "checked_byte_boundary: offset {byte_idx} is not a valid char boundary in a \
         {}-byte string (ADR-087 chokepoint) -- this indicates a text-index domain bug \
         upstream (a byte offset built from `.len()`/char-count arithmetic instead of a \
         grapheme/byte conversion)",
        s.len()
    );
    tracing::warn!(
        byte_idx,
        str_len = s.len(),
        "checked_byte_boundary: clamping invalid byte offset to nearest char boundary"
    );
    let mut idx = byte_idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

// ---------------------------------------------------------------------------
// Grapheme-cluster-safe width clamping (ADR-087 Rule 2)
// ---------------------------------------------------------------------------

/// Return the byte offset of the grapheme-cluster boundary such that
/// `&s[..offset]` has display width `<= max_width`, walking forward from
/// the start. Never lands mid-cluster (e.g. between a ZWJ and its base, or
/// before a combining mark) because the cut points considered are exactly
/// the boundaries `unicode-segmentation` reports, not `char_indices()`
/// positions.
///
/// This is the shared engine behind `text_utils::truncate_end`; call sites
/// that need a bare width clamp with no ellipsis (e.g. a column-padded
/// table cell) can use it directly.
pub fn byte_offset_for_max_width_with(s: &str, max_width: usize, policy: WidthPolicy) -> usize {
    let mut cols = 0;
    for (byte_idx, g) in s.grapheme_indices(true) {
        let w = grapheme_width(g, policy);
        if cols + w > max_width {
            return checked_byte_boundary(s, byte_idx);
        }
        cols += w;
    }
    s.len()
}

/// `byte_offset_for_max_width_with` under the default `WidthPolicy`.
pub fn byte_offset_for_max_width(s: &str, max_width: usize) -> usize {
    byte_offset_for_max_width_with(s, max_width, WidthPolicy::default())
}

/// The mirror of `byte_offset_for_max_width_with`, walking backward from the
/// end: returns the byte offset such that `&s[offset..]` has display width
/// `<= max_width`. Shared engine behind `text_utils::truncate_start`.
pub fn byte_offset_for_max_width_from_end_with(
    s: &str,
    max_width: usize,
    policy: WidthPolicy,
) -> usize {
    let mut cols = 0;
    let mut start = s.len();
    for (byte_idx, g) in s.grapheme_indices(true).collect::<Vec<_>>().into_iter().rev() {
        let w = grapheme_width(g, policy);
        if cols + w > max_width {
            break;
        }
        cols += w;
        start = byte_idx;
    }
    checked_byte_boundary(s, start)
}

/// `byte_offset_for_max_width_from_end_with` under the default `WidthPolicy`.
pub fn byte_offset_for_max_width_from_end(s: &str, max_width: usize) -> usize {
    byte_offset_for_max_width_from_end_with(s, max_width, WidthPolicy::default())
}

/// Get the grapheme count of a ropey line (excluding trailing newline).
///
/// Convenience for cursor movement: line length in graphemes, not chars.
pub fn line_grapheme_count(line: &ropey::RopeSlice) -> usize {
    let s: String = line.chars().collect();
    let trimmed = s.trim_end_matches('\n');
    grapheme_count(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_grapheme_count() {
        assert_eq!(grapheme_count("hello"), 5);
        assert_eq!(grapheme_count(""), 0);
        assert_eq!(grapheme_count(" "), 1);
    }

    #[test]
    fn ascii_display_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn cjk_display_width() {
        // Each CJK character is 2 cells wide
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("世界"), 4);
        assert_eq!(grapheme_count("你好"), 2);
    }

    #[test]
    fn emoji_display_width() {
        // Basic emoji are typically 2 cells wide
        assert_eq!(grapheme_count("👋"), 1);
        assert_eq!(display_width("👋"), 2);
    }

    #[test]
    fn combining_character() {
        // é can be e + combining acute accent (2 chars, 1 grapheme)
        let s = "e\u{0301}"; // e + combining acute accent
        assert_eq!(grapheme_count(s), 1);
        assert_eq!(s.chars().count(), 2); // but 2 chars
    }

    #[test]
    fn mixed_ascii_cjk_emoji() {
        let s = "hi你好👋";
        assert_eq!(grapheme_count(s), 5); // h, i, 你, 好, 👋
                                          // h=1, i=1, 你=2, 好=2, 👋=2 = 8
        assert_eq!(display_width(s), 8);
    }

    #[test]
    fn grapheme_to_char_offset_ascii() {
        assert_eq!(grapheme_to_char_offset("hello", 0), 0);
        assert_eq!(grapheme_to_char_offset("hello", 2), 2);
        assert_eq!(grapheme_to_char_offset("hello", 5), 5);
    }

    #[test]
    fn grapheme_to_char_offset_combining() {
        let s = "e\u{0301}x"; // é (2 chars) + x (1 char)
        assert_eq!(grapheme_to_char_offset(s, 0), 0); // start of é
        assert_eq!(grapheme_to_char_offset(s, 1), 2); // start of x (char offset 2)
    }

    #[test]
    fn display_width_up_to_grapheme_cjk() {
        let s = "a你b好c";
        // a=1, 你=2, b=1, 好=2, c=1
        assert_eq!(display_width_up_to_grapheme(s, 0), 0);
        assert_eq!(display_width_up_to_grapheme(s, 1), 1); // after 'a'
        assert_eq!(display_width_up_to_grapheme(s, 2), 3); // after '你'
        assert_eq!(display_width_up_to_grapheme(s, 3), 4); // after 'b'
        assert_eq!(display_width_up_to_grapheme(s, 4), 6); // after '好'
        assert_eq!(display_width_up_to_grapheme(s, 5), 7); // after 'c'
    }

    #[test]
    fn line_grapheme_count_strips_newline() {
        let rope = ropey::Rope::from_str("hello\nworld\n");
        assert_eq!(line_grapheme_count(&rope.line(0)), 5);
        assert_eq!(line_grapheme_count(&rope.line(1)), 5);
    }

    #[test]
    fn line_grapheme_count_cjk() {
        let rope = ropey::Rope::from_str("你好世界\n");
        assert_eq!(line_grapheme_count(&rope.line(0)), 4);
    }
}
