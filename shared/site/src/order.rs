//! The order a reader expects of a list of titles or tags: A to Z, ignoring
//! case and accents (ADR-119 Amendment 3).
//!
//! Sorting by bytes put every capital before every lower-case letter
//! ("Zebra" before "apple") and every accented initial after "z" ("Índice",
//! "Élan"), which no reader of an es-ES or fr-FR site reads as A to Z. The
//! key here is the text decomposed (NFD), with its combining marks dropped,
//! in lower case: a first-level comparison close to CLDR's root collation
//! for Latin script. It is not a language's tailoring: es-ES orders "ñ"
//! after "n" as a letter of its own, and this key orders it with "n". Ties
//! fall back to the text itself, so the order is total and deterministic.

use std::cmp::Ordering;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

/// The key [`cmp_text`] compares first.
pub fn sort_key(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// `a` against `b` in reading order: by [`sort_key`], then by the text.
pub fn cmp_text(a: &str, b: &str) -> Ordering {
    sort_key(a).cmp(&sort_key(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defects the byte order had, each pinned: case, an accented
    /// initial, and a decomposed accent that must sort as its composed form.
    #[test]
    fn titles_sort_as_a_reader_reads_them() {
        let mut v = vec![
            "Zebra",
            "apple",
            "\u{cd}ndice",
            "indigo",
            "Ivory",
            "\u{c9}lan",
            "eagle",
            "E\u{301}tude",
            "Ezra",
        ];
        v.sort_by(|a, b| cmp_text(a, b));
        assert_eq!(
            v,
            [
                "apple",
                "eagle",
                "\u{c9}lan",
                "E\u{301}tude",
                "Ezra",
                "\u{cd}ndice",
                "indigo",
                "Ivory",
                "Zebra",
            ]
        );
    }

    /// A total order: texts that share a key are still ordered, and only
    /// equal texts compare equal, so no list depends on its input order.
    #[test]
    fn the_order_is_total_and_antisymmetric() {
        let words = [
            "a", "A", "\u{e1}", "a\u{301}", "\u{c1}", "b", "", " ", "\u{df}", "ss", "SS",
        ];
        for a in words {
            for b in words {
                let ab = cmp_text(a, b);
                assert_eq!(ab, cmp_text(b, a).reverse(), "{a:?} {b:?}");
                assert_eq!(ab == Ordering::Equal, a == b, "{a:?} {b:?}");
            }
        }
    }
}
