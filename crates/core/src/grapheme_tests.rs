//! Tests for [`super`] — the ADR-087 text-index-domain helpers.
//!
//! Extracted from `grapheme.rs` under CLAUDE.md's "inline tests dominate"
//! remedy: the module was 1,004 lines of which ~61% were tests, which is the
//! documented trigger for sibling extraction. Declared with `#[path]` from
//! `grapheme.rs` so these keep private-item access, matching the
//! `permission_tests.rs` precedent in `mae-scheme`.

use super::*;

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
#[path = "grapheme_corpus_tests.rs"]
mod grapheme_corpus_tests;
