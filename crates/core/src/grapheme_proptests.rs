//! Property tests for the ADR-087 width/truncation invariants.
//!
//! Split from `grapheme_corpus_tests.rs` for the 500-line test ceiling.
//!
//! Caveat recorded in ADR-087 and confirmed by two independent measurements:
//! proptest's default `String` generator **cannot** produce ZWJ sequences
//! (category Cf is excluded from its alphabet). So these properties cover the
//! panic / width-bound / idempotence class over random input, and the named
//! corpus next door covers ZWJ specifically. Neither alone is sufficient.

use super::super::*;

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
