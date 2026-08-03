//! ADR-087 Rule 1/Rule 4: the single named conversion between MAE's byte
//! cursor columns and an LSP `Position.character`.
//!
//! @ai-caution: [text-index-domain] **MAE does not negotiate
//! `positionEncoding`.** The LSP specification's default is `utf-16`, and
//! what MAE has always sent — and still sends after the Rule 4 byte
//! migration — is a **UTF-32 (Unicode scalar) count**. That is an interop
//! bug on any line containing an astral-plane character (emoji, rare CJK,
//! musical symbols): MAE says 1, a spec-conforming server expects 2.
//!
//! It is ADR-087 **Rule 5**, deliberately *out of scope* for the Rule 4
//! change, and it is recorded here rather than fixed because Rule 4 made it
//! newly visible: before the migration `cursor_col` was already a char index,
//! so the wire value came out of a bare `as u32` cast with nothing to read.
//! Now every producer and consumer goes through this module, so Rule 5 is a
//! two-function change (`byte_col_to_lsp_character` /
//! `lsp_character_to_byte_col` switch to `encode_utf16().count()` and its
//! inverse) plus the `initialize` handshake — instead of a hunt across the
//! call sites.
//!
//! Until then these two functions are **exactly** the pre-migration
//! behaviour, so the Rule 4 change is behaviour-preserving at the LSP
//! boundary rather than trading one wrong encoding for another.

/// Byte column on a line -> the `character` field of an LSP `Position`.
///
/// Emits a **Unicode scalar (UTF-32) count**, matching what MAE has always
/// sent. See the module docs: the spec default is UTF-16 and closing that gap
/// is Rule 5.
pub fn byte_col_to_lsp_character(line: &str, byte_col: usize) -> u32 {
    let b = crate::grapheme::floor_char_boundary(line, byte_col);
    line[..b].chars().count() as u32
}

/// The `character` field of an inbound LSP `Position` -> a byte column.
///
/// Inbound positions are **not trusted**: a `character` past the end of the
/// line, or one landing mid-cluster because the server counted in a different
/// encoding, is clamped to a grapheme-cluster boundary rather than panicking
/// a slice downstream (ADR-087's chokepoint discipline).
pub fn lsp_character_to_byte_col(line: &str, character: u32) -> usize {
    let byte = crate::grapheme::char_idx_to_byte_idx(line, character as usize);
    crate::grapheme::snap_to_grapheme_boundary(line, byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nasty-string corpus ADR-087 enforcement item 4 asks for, applied
    /// to this boundary. Each case is named so a failure says *what* broke.
    fn corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ascii", "hello world"),
            ("cjk", "日本語のテキスト"),
            ("combining", "e\u{0301}cole cafe\u{0301}"),
            ("zwj-family", "👨‍👩‍👧‍👦 family"),
            ("skin-tone", "👍🏽 ok"),
            ("regional-flags", "🇯🇵🇺🇸 flags"),
            ("hangul-jamo", "\u{1100}\u{1161}\u{11A8} han"),
            ("astral-cjk", "\u{20000}\u{2A6B2} rare"),
            ("bom-then-text", "\u{FEFF}text"),
            ("bidi-override", "a\u{202E}bc\u{202C}d"),
            ("virama", "\u{0915}\u{094D}\u{0937} ksha"),
            ("wide-and-ambiguous", "→│日a"),
            ("khmer-width-3", "\u{17D8} sign"),
            ("mixed", "a日👨‍👩‍👧b\u{0301}c"),
            ("empty", ""),
        ]
    }

    #[test]
    fn round_trips_at_every_char_boundary_of_the_nasty_corpus() {
        for (name, line) in corpus() {
            for (byte_col, _) in line.char_indices().chain(std::iter::once((line.len(), ' '))) {
                let ch = byte_col_to_lsp_character(line, byte_col);
                let back = lsp_character_to_byte_col(line, ch);
                // `back` snaps to a grapheme boundary, so it may move left of
                // a mid-cluster char boundary. The invariant that must hold is
                // that it never moves *past* the original and always lands on
                // a real char boundary.
                assert!(
                    back <= byte_col,
                    "{name}: round trip moved right: {byte_col} -> {ch} -> {back}"
                );
                assert!(
                    line.is_char_boundary(back),
                    "{name}: round trip produced a mid-UTF-8 offset {back}"
                );
            }
        }
    }

    #[test]
    fn round_trips_exactly_at_every_grapheme_boundary() {
        use unicode_segmentation::UnicodeSegmentation;
        for (name, line) in corpus() {
            for (byte_col, _) in line
                .grapheme_indices(true)
                .chain(std::iter::once((line.len(), "")))
            {
                let ch = byte_col_to_lsp_character(line, byte_col);
                assert_eq!(
                    lsp_character_to_byte_col(line, ch),
                    byte_col,
                    "{name}: grapheme-boundary round trip failed at byte {byte_col}"
                );
            }
        }
    }

    #[test]
    fn an_out_of_range_inbound_character_clamps_instead_of_panicking() {
        for (name, line) in corpus() {
            for character in [u32::MAX, 10_000, line.len() as u32 + 7] {
                let col = lsp_character_to_byte_col(line, character);
                assert!(col <= line.len(), "{name}: clamp overshot the line");
                assert!(line.is_char_boundary(col), "{name}: clamp split a char");
            }
        }
    }

    #[test]
    fn a_mid_cluster_inbound_character_snaps_back_to_the_cluster_start() {
        // A server counting UTF-16 hands MAE a `character` that, read as a
        // scalar count, lands inside a ZWJ sequence. It must not split it.
        let line = "👨‍👩‍👧‍👦x";
        let cluster_len = line.len() - 1;
        for character in 1..7u32 {
            let col = lsp_character_to_byte_col(line, character);
            assert!(
                col == 0 || col == cluster_len,
                "character {character} landed at {col}, inside the ZWJ cluster"
            );
        }
    }

    #[test]
    fn it_still_emits_scalar_counts_not_utf16_units() {
        // Pins the *known* Rule 5 gap so closing it is a deliberate, visible
        // change rather than an accident. U+20000 is one scalar, two UTF-16
        // code units.
        let line = "\u{20000}a";
        assert_eq!(byte_col_to_lsp_character(line, 4), 1);
        assert_eq!(line.encode_utf16().count(), 3);
    }
}
