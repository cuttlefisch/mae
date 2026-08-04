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
/// implicit behavior; `bool::default()`/`usize::default()` already give
/// exactly these values) or from the live editor options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WidthPolicy {
    /// Resolve East_Asian_Width=Ambiguous code points as 2 columns (CJK
    /// convention) instead of the default 1 column (non-CJK convention).
    pub ambiguous_wide: bool,
    /// Display width assigned to a control character, which
    /// `unicode-width` reports as `None` (undefined) rather than a number.
    pub control_char_width: usize,
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
/// Walks per-grapheme-cluster rather than delegating to `unicode-width`'s
/// whole-string `UnicodeWidthStr::width(s)`/`width_cjk(s)` directly, because
/// that whole-string algorithm has its own hardcoded control-character
/// handling (a raw control byte contributes width 1 there, e.g. `"\r\n"` is
/// deliberately 1 not 2) which disagrees with -- and cannot be overridden by
/// -- `policy.control_char_width`. Every other cross-character width rule
/// `unicode-width` implements (ZWJ sequences, regional-indicator flag
/// pairs, emoji modifier/presentation sequences, CRLF) binds characters
/// that Unicode's own extended-grapheme-cluster rules (UAX #29) already
/// group into a single cluster, so computing width per-cluster (via
/// `unicode-width`'s per-cluster algorithm, still exact within a cluster)
/// and summing is equivalent to the whole-string computation for every case
/// except control characters -- which is exactly the one case this
/// function needs to diverge on.
pub fn display_width_with(s: &str, policy: WidthPolicy) -> usize {
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

/// Display width (terminal cells) of `s`'s first `byte_idx` bytes, under
/// `policy`.
///
/// **This is the byte-column -> screen-column conversion** (ADR-087 Rule 1),
/// and the replacement for feeding a cursor column to
/// [`display_width_up_to_grapheme`] — which takes a *grapheme index* and was
/// one of the four domains `Window::cursor_col` was silently read as.
/// `byte_idx` is floored to a char boundary, so a column that drifted
/// mid-sequence measures short rather than panicking.
pub fn display_width_of_prefix_with(s: &str, byte_idx: usize, policy: WidthPolicy) -> usize {
    let b = floor_char_boundary(s, byte_idx);
    display_width_with(&s[..b], policy)
}

// ---------------------------------------------------------------------------
// ADR-087 Rule 4 — byte offsets that sit on grapheme-cluster boundaries.
//
// These are the *only* sanctioned way to step or clamp a cursor column, which
// is a byte offset from the start of a line. `+ 1` / `- 1` on such an offset
// is a bug on any non-ASCII line; these functions are the named conversions
// Rule 1 requires in its place.
// ---------------------------------------------------------------------------

/// Byte offset of the first grapheme-cluster boundary strictly after
/// `byte_idx`, or `s.len()` if there is none. One "move right".
pub fn next_grapheme_boundary(s: &str, byte_idx: usize) -> usize {
    let from = floor_char_boundary(s, byte_idx);
    s.grapheme_indices(true)
        .map(|(i, g)| i + g.len())
        .find(|&end| end > from)
        .unwrap_or(s.len())
}

/// Byte offset of the last grapheme-cluster boundary strictly before
/// `byte_idx`, or 0 if there is none. One "move left".
pub fn prev_grapheme_boundary(s: &str, byte_idx: usize) -> usize {
    let from = floor_char_boundary(s, byte_idx);
    let mut last = 0;
    for (i, _) in s.grapheme_indices(true) {
        if i >= from {
            break;
        }
        last = i;
    }
    last
}

/// Round `byte_idx` down to the grapheme-cluster boundary at or before it,
/// clamped to `s.len()`. `s.len()` is itself always a boundary.
///
/// This is the chokepoint every externally-sourced or arithmetic-derived
/// cursor column passes through (`Buffer::snap_col_to_grapheme`). Unlike
/// [`checked_byte_boundary`] it does not assert: landing mid-cluster is the
/// *expected* case for a restored session file, an LSP position, or a mouse
/// click, not evidence of an upstream bug.
pub fn snap_to_grapheme_boundary(s: &str, byte_idx: usize) -> usize {
    if byte_idx >= s.len() {
        return s.len();
    }
    let target = floor_char_boundary(s, byte_idx);
    let mut last = 0;
    for (i, _) in s.grapheme_indices(true) {
        if i > target {
            break;
        }
        last = i;
    }
    last
}

/// Convert a **char** (Unicode scalar) index into a byte offset. Saturates at
/// `s.len()` when `char_idx` runs past the end.
///
/// The `char -> byte` half of Rule 1's named conversions: used wherever a
/// legacy char-domain column (a pre-migration session file, an LSP
/// `Position.character` under the spec's default encoding, a ropey 1.x
/// computation) crosses into MAE's byte domain.
pub fn char_idx_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
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
/// Round `byte_idx` **down** to the nearest char boundary, with no assertion.
///
/// This is the "clamp a budget" counterpart to [`checked_byte_boundary`], and
/// choosing between them matters:
///
/// - [`checked_byte_boundary`] means *"this offset should already be valid; if
///   it is not, something upstream computed it wrongly"* — so it
///   `debug_assert`s. Use it for offsets derived from grapheme/char iteration.
/// - `floor_char_boundary` means *"cut this text at roughly N bytes"* — where
///   landing mid-character is the **expected** case, not a bug. Truncating
///   arbitrary external text (shell output, an HTTP response body, tool output)
///   at a fixed byte budget lands mid-character routinely, and asserting on
///   that would panic every debug build on ordinary input.
///
/// Conflating the two is how the chokepoint ended up asserting on its own
/// normal case. Mirrors the unstable `str::floor_char_boundary`.
pub fn floor_char_boundary(s: &str, byte_idx: usize) -> usize {
    let mut idx = byte_idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

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
    for (byte_idx, g) in s
        .grapheme_indices(true)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
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
#[path = "grapheme_tests.rs"]
mod grapheme_tests;
