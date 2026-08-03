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

// ---------------------------------------------------------------------------
// ADR-087 enforcement item 4: named nasty-string corpus (CLAUDE.md #14 --
// adversarial, not confirmation). Measured in the ADR's research: a
// 15-entry hand-curated list beat every general-purpose generator on
// grapheme/width/ZWJ oracles, and its failures are *named* rather than an
// inscrutable generated string. proptest (below) covers the panic
// invariants across random input; this corpus covers the specific Unicode
// mechanisms proptest's `String` generator structurally cannot reach
// (category Cf, which excludes ZWJ, is never generated).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod nasty_corpus {
    use super::*;

    /// One named case: `(name, string, expected_grapheme_count, expected_width_narrow)`.
    /// `expected_width_narrow` is `None` where the "correct" width isn't a
    /// simple fact worth hardcoding (RTL override) -- those cases still get
    /// the full panic/round-trip/truncation treatment, just not a width
    /// equality assertion.
    struct Case {
        name: &'static str,
        s: &'static str,
        graphemes: usize,
        width_narrow: Option<usize>,
    }

    const CORPUS: &[Case] = &[
        // 1. héllo, NFC (single precomposed U+00E9 LATIN SMALL LETTER E WITH ACUTE)
        Case {
            name: "hello_nfc",
            s: "h\u{e9}llo",
            graphemes: 5,
            width_narrow: Some(5),
        },
        // 2. héllo, NFD (e + U+0301 COMBINING ACUTE ACCENT) -- same visible text,
        //    same width/grapheme count as NFC despite being 6 chars not 5.
        Case {
            name: "hello_nfd",
            s: "he\u{301}llo",
            graphemes: 5,
            width_narrow: Some(5),
        },
        // 3. CJK: three wide ideographs.
        Case {
            name: "cjk",
            s: "\u{65e5}\u{672c}\u{8a9e}",
            graphemes: 3,
            width_narrow: Some(6),
        },
        // 4. Family ZWJ emoji (man+ZWJ+woman+ZWJ+girl+ZWJ+boy): one grapheme
        //    cluster, width 2 -- FALSIFIES the deleted per-char-sum
        //    `display_width` (man=2, ZWJ=0, woman=2, ZWJ=0, girl=2, ZWJ=0,
        //    boy=2 sums to 8, not 2). See `falsifies_the_deleted_per_char_sum`.
        Case {
            name: "family_zwj_emoji",
            s: "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}",
            graphemes: 1,
            width_narrow: Some(2),
        },
        // 5. Regional-indicator flag (Japan: two REGIONAL INDICATOR SYMBOL
        //    LETTERs forming one flag grapheme cluster).
        Case {
            name: "flag_jp",
            s: "\u{1f1ef}\u{1f1f5}",
            graphemes: 1,
            width_narrow: Some(2),
        },
        // 6. Skin-tone modifier sequence (waving hand + Fitzpatrick type-4).
        Case {
            name: "skin_tone_modifier",
            s: "\u{1f44b}\u{1f3fd}",
            graphemes: 1,
            width_narrow: Some(2),
        },
        // 7. U+FE0F (VARIATION SELECTOR-16, forces emoji presentation) --
        //    FALSIFIES the deleted per-char-sum implementation: heavy black
        //    heart alone is narrow/ambiguous-ish under a naive per-char
        //    width, FE0F contributes 0, and the pair is never widened to 2.
        Case {
            name: "heart_vs16_emoji",
            s: "\u{2764}\u{fe0f}",
            graphemes: 1,
            width_narrow: Some(2),
        },
        // 8. U+FE0E (VARIATION SELECTOR-15, forces text presentation) --
        //    FALSIFIES the same way in the opposite direction: this must
        //    stay narrow (1), which only the sequence-aware algorithm knows.
        Case {
            name: "heart_vs15_text",
            s: "\u{2764}\u{fe0e}",
            graphemes: 1,
            width_narrow: Some(1),
        },
        // 9. Zalgo: a base char with a pile of combining marks -- one
        //    grapheme cluster, width 1 (every combining mark contributes 0).
        Case {
            name: "zalgo",
            s: "e\u{301}\u{302}\u{303}\u{304}\u{305}\u{306}\u{307}\u{308}\u{309}\u{30a}",
            graphemes: 1,
            width_narrow: Some(1),
        },
        // 10. EAW=Ambiguous (SECTION SIGN, U+00A7) -- width depends on
        //     `ambiguous_wide` (verified against `unicode-width` 0.2.2's own
        //     tables: narrow=1/wide=2; several Greek/Cyrillic letters are
        //     narrow-only in this table version, so this is the more
        //     reliable representative of the Ambiguous category). Covered
        //     separately in `ambiguous_width_policy_changes_ambiguous_char`.
        Case {
            name: "eaw_ambiguous_section_sign",
            s: "\u{a7}",
            graphemes: 1,
            width_narrow: Some(1),
        },
        // 11. ZWSP (ZERO WIDTH SPACE): its own grapheme cluster, width 0.
        Case {
            name: "zwsp",
            s: "\u{200b}",
            graphemes: 1,
            width_narrow: Some(0),
        },
        // 12. Halfwidth katakana: EAW=Halfwidth, always narrow (1) regardless
        //     of the ambiguous-width policy (Halfwidth != Ambiguous).
        Case {
            name: "halfwidth_katakana",
            s: "\u{ff71}",
            graphemes: 1,
            width_narrow: Some(1),
        },
        // 13. Control character: undefined upstream: default policy is 0.
        Case {
            name: "control_char",
            s: "\u{1}",
            graphemes: 1,
            width_narrow: Some(0),
        },
        // 14. RTL override (bidi format control): no hardcoded width
        //     assertion (Rule 6 terminal-rendering territory, out of this
        //     ADR pass's scope) -- still exercised for panic-safety.
        Case {
            name: "rtl_override",
            s: "\u{202e}abc\u{202c}",
            graphemes: 5,
            width_narrow: None,
        },
        // 15. Astral plane: a CJK Extension B ideograph outside the BMP,
        //     encoded as a single Rust `char` (never a surrogate pair --
        //     Rust strings are UTF-8/scalar values, not UTF-16).
        Case {
            name: "astral_cjk",
            s: "\u{20000}",
            graphemes: 1,
            width_narrow: Some(2),
        },
    ];

    #[test]
    fn corpus_has_fifteen_named_cases() {
        assert_eq!(CORPUS.len(), 15);
    }

    #[test]
    fn corpus_grapheme_counts() {
        for c in CORPUS {
            assert_eq!(
                grapheme_count(c.s),
                c.graphemes,
                "case {:?}: grapheme_count",
                c.name
            );
        }
    }

    #[test]
    fn corpus_display_width_narrow() {
        for c in CORPUS {
            if let Some(expected) = c.width_narrow {
                assert_eq!(
                    display_width(c.s),
                    expected,
                    "case {:?}: display_width (default/narrow policy)",
                    c.name
                );
            }
        }
    }

    /// Reproduces the *deleted* `text_utils::display_width` bug (`s.chars()
    /// .map(|c| c.width().unwrap_or(0)).sum()`) so the falsifying cases can
    /// assert against it directly, rather than only asserting the current
    /// (correct) value and trusting a comment that the old one differed.
    fn naive_per_char_sum(s: &str) -> usize {
        s.chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum()
    }

    #[test]
    fn falsifies_the_deleted_per_char_sum() {
        // Cases 4, 7, 8: the ADR's own falsifying trio. Each must show the
        // *correct* per-cluster width disagreeing with the naive per-char
        // sum the deleted implementation used -- proof this corpus would
        // have caught the bug, not just that the new code looks right in
        // isolation.
        let family = CORPUS
            .iter()
            .find(|c| c.name == "family_zwj_emoji")
            .unwrap();
        assert_eq!(display_width(family.s), 2);
        assert_eq!(
            naive_per_char_sum(family.s),
            8,
            "man(2)+ZWJ(0)x3+woman(2)+girl(2)+boy(2)"
        );
        assert_ne!(display_width(family.s), naive_per_char_sum(family.s));

        let vs16 = CORPUS
            .iter()
            .find(|c| c.name == "heart_vs16_emoji")
            .unwrap();
        assert_eq!(display_width(vs16.s), 2);
        assert_ne!(
            display_width(vs16.s),
            naive_per_char_sum(vs16.s),
            "VS16 must widen the preceding heart to 2, which no per-char sum can express"
        );

        let vs15 = CORPUS.iter().find(|c| c.name == "heart_vs15_text").unwrap();
        assert_eq!(display_width(vs15.s), 1);
        // vs15's naive sum happens to also land on 1 for this particular
        // base char (heart's per-char width() is already 1) -- the point of
        // this case is the *emoji* sibling above, not this one; kept in the
        // corpus for the presentation-selector round-trip, not as a second
        // falsifying assertion.
    }

    #[test]
    fn ambiguous_width_policy_changes_ambiguous_char() {
        let c = CORPUS
            .iter()
            .find(|c| c.name == "eaw_ambiguous_section_sign")
            .unwrap();
        let narrow = display_width_with(
            c.s,
            WidthPolicy {
                ambiguous_wide: false,
                control_char_width: 0,
            },
        );
        let wide = display_width_with(
            c.s,
            WidthPolicy {
                ambiguous_wide: true,
                control_char_width: 0,
            },
        );
        assert_eq!(narrow, 1);
        assert_eq!(wide, 2);
    }

    #[test]
    fn halfwidth_katakana_stays_narrow_under_wide_policy() {
        // Regression guard: EAW=Halfwidth must NOT be affected by the
        // ambiguous-width policy (only EAW=Ambiguous is).
        let c = CORPUS
            .iter()
            .find(|c| c.name == "halfwidth_katakana")
            .unwrap();
        let wide = display_width_with(
            c.s,
            WidthPolicy {
                ambiguous_wide: true,
                control_char_width: 0,
            },
        );
        assert_eq!(wide, 1);
    }

    #[test]
    fn control_char_width_policy_applies_in_corpus() {
        let c = CORPUS.iter().find(|c| c.name == "control_char").unwrap();
        let configured = display_width_with(
            c.s,
            WidthPolicy {
                ambiguous_wide: false,
                control_char_width: 4,
            },
        );
        assert_eq!(configured, 4);
    }

    #[test]
    fn corpus_never_panics_across_every_string_api() {
        for c in CORPUS {
            let width = display_width(c.s);
            for budget in 0..=(width + 3) {
                let end = crate::text_utils::truncate_end(c.s, budget);
                let start = crate::text_utils::truncate_start(c.s, budget);
                assert!(
                    display_width(&end) <= budget,
                    "case {:?} truncate_end budget {budget}: {end:?}",
                    c.name
                );
                assert!(
                    display_width(&start) <= budget,
                    "case {:?} truncate_start budget {budget}: {start:?}",
                    c.name
                );
                // Never a mid-grapheme cut: re-running grapheme segmentation
                // on the result must reproduce the same string (i.e. it's
                // already a valid sequence of whole clusters).
                assert_eq!(
                    end.graphemes(true).collect::<String>(),
                    end,
                    "case {:?}: truncate_end not grapheme-clean at budget {budget}",
                    c.name
                );
                assert_eq!(
                    start.graphemes(true).collect::<String>(),
                    start,
                    "case {:?}: truncate_start not grapheme-clean at budget {budget}",
                    c.name
                );
            }
        }
    }

    #[test]
    fn corpus_round_trips_when_budget_covers_full_width() {
        for c in CORPUS {
            let width = display_width(c.s);
            assert_eq!(
                crate::text_utils::truncate_end(c.s, width),
                c.s,
                "case {:?}: truncate_end no-op at full width",
                c.name
            );
            assert_eq!(
                crate::text_utils::truncate_start(c.s, width),
                c.s,
                "case {:?}: truncate_start no-op at full width",
                c.name
            );
        }
    }

    // -----------------------------------------------------------------------
    // checked_byte_boundary: the chokepoint validator's two documented
    // behaviors, tested separately because they're mutually exclusive within
    // one build profile. `cargo test` runs with `debug_assertions` on, so
    // the "clamps and logs in release" path is not exercisable here with an
    // invalid offset -- the `debug_assert!` fires first, by design. That
    // means a blanket "never panics on arbitrary offsets" proptest for this
    // function specifically would just be asserting the debug_assert never
    // fires, i.e. testing against its own documented contract. Instead:
    // valid offsets must always pass through unchanged (no panic), and one
    // adversarial case confirms the debug alarm actually fires on an
    // offset that lands mid-character (CLAUDE.md #14 -- the negative case
    // that must fail is worth more than ten that pass).
    // -----------------------------------------------------------------------

    #[test]
    fn checked_byte_boundary_passes_through_every_valid_offset_unchanged() {
        for c in CORPUS {
            let valid: Vec<usize> =
                c.s.char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(c.s.len()))
                    .collect();
            for offset in valid {
                assert_eq!(
                    checked_byte_boundary(c.s, offset),
                    offset,
                    "case {:?}: a genuinely valid char-boundary offset must pass through unchanged",
                    c.name
                );
            }
        }
    }

    /// The regression this pair of functions exists to prevent.
    ///
    /// Truncating arbitrary external text at a fixed byte budget lands
    /// mid-character routinely — that is the *expected* case, not a caller
    /// bug. Every one of the real call sites is of this shape: shell stdout
    /// at 10_000 bytes, an HTTP body preview at 500, tool output at 200.
    /// Routing those through the asserting validator made every debug build
    /// (including `cargo test`) panic on ordinary non-ASCII output.
    #[test]
    fn flooring_a_byte_budget_mid_character_does_not_panic_in_debug() {
        // 3-byte characters, so a 10-byte budget lands inside the 4th.
        let s = "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}";
        for budget in 0..=s.len() + 4 {
            let cut = floor_char_boundary(s, budget);
            assert!(
                s.is_char_boundary(cut),
                "budget {budget} produced non-boundary offset {cut}"
            );
            assert!(cut <= budget.min(s.len()), "must round DOWN, never up");
            // The real point: this must not panic, and must be sliceable.
            let _ = &s[..cut];
        }
    }

    /// Flooring must be a no-op on input that is already valid, so swapping a
    /// call site from the validator to the floor cannot silently change a
    /// correct offset.
    #[test]
    fn flooring_leaves_every_valid_offset_untouched() {
        for c in CORPUS {
            for (i, _) in c.s.char_indices().chain(std::iter::once((c.s.len(), ' '))) {
                assert_eq!(floor_char_boundary(c.s, i), i, "case {:?}", c.name);
            }
        }
    }

    #[test]
    #[should_panic(expected = "not a valid char boundary")]
    fn checked_byte_boundary_debug_asserts_on_a_mid_character_offset() {
        // "日" is a 3-byte UTF-8 character; byte offset 1 lands inside it.
        // This MUST panic in a debug build -- it is the chokepoint catching
        // exactly the bug class ADR-087 exists to close.
        let _ = checked_byte_boundary("\u{65e5}", 1);
    }
}

// ---------------------------------------------------------------------------
// ADR-087 enforcement item 5: proptest invariants over the panic class.
// Caveat (measured in the ADR's research, confirmed by two independent
// sources): proptest's default `String` generator cannot produce ZWJ
// sequences (category Cf is excluded from the generator's alphabet), so
// these properties cover the panic/width-bound/idempotence class across
// random input; the named corpus above covers ZWJ specifically.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod width_proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn truncate_end_never_panics(s in ".*", max_cols in 0usize..200) {
            let _ = crate::text_utils::truncate_end(&s, max_cols);
        }

        #[test]
        fn truncate_start_never_panics(s in ".*", max_cols in 0usize..200) {
            let _ = crate::text_utils::truncate_start(&s, max_cols);
        }

        #[test]
        fn truncate_end_width_never_exceeds_budget(s in ".*", max_cols in 0usize..200) {
            let result = crate::text_utils::truncate_end(&s, max_cols);
            prop_assert!(display_width(&result) <= max_cols);
        }

        #[test]
        fn truncate_start_width_never_exceeds_budget(s in ".*", max_cols in 0usize..200) {
            let result = crate::text_utils::truncate_start(&s, max_cols);
            prop_assert!(display_width(&result) <= max_cols);
        }

        #[test]
        fn truncate_end_is_a_grapheme_boundary_prefix(s in ".*", max_cols in 0usize..200) {
            let result = crate::text_utils::truncate_end(&s, max_cols);
            // The result stripped of a possible trailing ellipsis must be a
            // byte-for-byte prefix of the input, never a mid-cluster cut.
            let core = result.strip_suffix('\u{2026}').unwrap_or(&result);
            prop_assert!(s.starts_with(core));
        }

        #[test]
        fn truncate_end_is_a_no_op_when_it_already_fits(s in ".*", pad in 0usize..50) {
            let width = display_width(&s);
            let result = crate::text_utils::truncate_end(&s, width + pad);
            prop_assert_eq!(result, s);
        }

        #[test]
        fn truncate_end_is_idempotent(s in ".*", max_cols in 0usize..200) {
            let once = crate::text_utils::truncate_end(&s, max_cols);
            let twice = crate::text_utils::truncate_end(&once, max_cols);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn display_width_never_panics(s in ".*") {
            let _ = display_width(&s);
        }

        // Note: `checked_byte_boundary` is deliberately NOT proptested here
        // with arbitrary offsets. It `debug_assert!`-panics by design on an
        // invalid offset (that's the whole point -- see
        // `checked_byte_boundary_debug_asserts_on_a_mid_character_offset`
        // above), so a "never panics on a random offset" property would
        // just assert the debug alarm never fires, i.e. test against its
        // own documented contract rather than a real invariant. It IS
        // exercised soundly here: every offset the `truncate_*` properties
        // above feed it (via `byte_offset_for_max_width*`) is constructed
        // from real `grapheme_indices` boundaries, so thousands of random
        // strings already stress its valid-input path on every run.
    }
}
