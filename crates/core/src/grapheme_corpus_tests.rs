//! The named adversarial Unicode corpus and the chokepoint tests for
//! [`super::super`]'s text-index helpers (ADR-087 enforcement items 3 and 4).
//!
//! Split from `grapheme_tests.rs` to stay under the 500-line test ceiling. The
//! corpus is deliberately hand-curated rather than generated: ADR-087 records
//! that a 15-entry named list beat every general-purpose generator on the
//! grapheme, width and ZWJ oracles, and — unlike a generated case — its
//! failures are named.

use super::super::*;

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

    // Only meaningful where `debug_assert!` is compiled in. CI runs
    // `cargo nextest run --release`, where `debug_assertions` is off, the
    // assert vanishes, the function clamps instead of panicking, and a bare
    // `#[should_panic]` therefore FAILS. The release path is covered instead by
    // `checked_byte_boundary_clamps_rather_than_panicking_in_release` below, so
    // both build profiles are asserted rather than one being skipped silently.
    /// The release-profile half of the contract: with `debug_assertions` off the
    /// assert is gone, and the documented behaviour is clamp-and-log — never a
    /// panic in a user's build. Without this, a release build's behaviour at the
    /// chokepoint would be entirely untested.
    #[cfg(not(debug_assertions))]
    #[test]
    fn checked_byte_boundary_clamps_rather_than_panicking_in_release() {
        // Byte 1 lands inside the 3-byte "\u{65e5}".
        assert_eq!(checked_byte_boundary("\u{65e5}", 1), 0);
        // Past the end clamps to the length, still on a boundary.
        assert_eq!(checked_byte_boundary("\u{65e5}", 99), 3);
    }

    #[cfg(debug_assertions)]
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
#[path = "grapheme_proptests.rs"]
mod grapheme_proptests;
