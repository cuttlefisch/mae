//! Emacs's character classes and case folding, as Org's regexps see them.
//!
//! The data is GENERATED from Emacs itself by
//! `conformance/gen-char-tables.el` (see `chartables.rs`): Unicode's own
//! properties differ from Emacs's syntax table in ways that move structure
//! (a no-break space is whitespace to Emacs, so a property key containing
//! one is not a property; a ß and ẞ fold together, SS does not).

use crate::chartables::{ALNUM, CANON, WHITESPACE, WORD};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    let k = table.partition_point(|&(_, end)| end < c);
    table.get(k).is_some_and(|&(start, _)| start <= c)
}

/// Emacs whitespace syntax: what `\s-` matches and `\S-` excludes.
pub(crate) fn is_space(c: char) -> bool {
    in_ranges(WHITESPACE, c)
}

/// Emacs word syntax: `\w`, `[[:word:]]`.
pub(crate) fn is_word(c: char) -> bool {
    in_ranges(WORD, c)
}

/// Emacs `[[:alnum:]]`.
pub(crate) fn is_alnum(c: char) -> bool {
    in_ranges(ALNUM, c)
}

/// The char `c` folds to under `case-fold-search`: two chars match iff
/// their canonical chars are equal.
pub(crate) fn canon(c: char) -> char {
    let u = c as u32;
    CANON
        .binary_search_by_key(&u, |&(from, _)| from)
        .ok()
        .and_then(|i| char::from_u32(CANON[i].1))
        .unwrap_or(c)
}

/// `s` with every char canonicalised: two names match case-insensitively
/// (Emacs regexps, char by char) iff their folds are equal.
pub(crate) fn fold(s: &str) -> String {
    s.chars().map(canon).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_disjoint() {
        for t in [WHITESPACE, WORD, ALNUM] {
            assert!(
                t.windows(2).all(|w| w[0].1 < w[1].0),
                "ranges overlap or are unsorted"
            );
            assert!(t.iter().all(|&(a, b)| a <= b));
        }
        assert!(CANON.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(!crate::chartables::EMACS_VERSION.is_empty());
    }

    #[test]
    fn spot_checks_measured_in_emacs_30() {
        // Whitespace: NBSP, EM SPACE, IDEOGRAPHIC SPACE, ZWSP, NNBSP yes; VT no.
        for c in [
            '\u{a0}', '\u{2003}', '\u{3000}', '\u{200b}', '\u{202f}', ' ', '\t',
        ] {
            assert!(is_space(c), "{c:?}");
        }
        assert!(!is_space('\u{b}'));
        // Word: emoji, middle dot, NEL, LINE SEPARATOR, combining acute yes;
        // feminine ordinal, circled 1 no; superscript 2 yes (Emacs syntax).
        for c in [
            '\u{1f600}',
            '\u{b7}',
            '\u{85}',
            '\u{2028}',
            '\u{301}',
            'é',
            '$',
        ] {
            assert!(is_word(c), "{c:?}");
        }
        for c in ['\u{aa}', '\u{2460}', '-', ':'] {
            assert!(!is_word(c), "{c:?}");
        }
        // Alnum: ª and combining acute yes; ², ①, emoji no.
        assert!(is_alnum('\u{aa}') && is_alnum('\u{301}') && is_alnum('é'));
        assert!(!is_alnum('\u{b2}') && !is_alnum('\u{2460}') && !is_alnum('\u{1f600}'));
        // Folding: ß~ẞ and σ~ς fold; Kelvin, long s and dotless i do not.
        assert_eq!(canon('\u{1e9e}'), canon('\u{df}'));
        assert_eq!(canon('\u{3c2}'), canon('\u{3c3}'));
        assert_ne!(canon('\u{212a}'), canon('k'));
        assert_ne!(canon('\u{17f}'), canon('s'));
        assert_ne!(canon('\u{131}'), canon('i'));
        assert_eq!(canon('S'), 's');
    }
}
