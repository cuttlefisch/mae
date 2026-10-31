//! Emacs's character classes and case folding, as Org's regexps see them.
//!
//! The data is GENERATED from Emacs itself by
//! `conformance/gen-char-tables.el` (see `chartables.rs`): Unicode's own
//! properties differ from Emacs's syntax table in ways that move structure
//! (a no-break space is whitespace to Emacs, so a property key containing
//! one is not a property; a ß and ẞ fold together, SS does not).

use crate::chartables::{
    ALNUM, ALPHA, CANON, CATEGORIES, DELIM, LINEBREAK, SCRIPTS, WHITESPACE, WORD,
    WORD_COMBINING_CATEGORIES, WORD_SEPARATING_CATEGORIES,
};

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

/// Emacs `[[:alpha:]]` (rx `letter`).
pub(crate) fn is_alpha(c: char) -> bool {
    in_ranges(ALPHA, c)
}

/// Punctuation, paren or string-quote syntax: `\s.`, `\s(`, `\s)`, `\s"`.
pub(crate) fn is_delim(c: char) -> bool {
    in_ranges(DELIM, c)
}

/// Category `|` (`\c|`): characters a line may break after.
pub(crate) fn is_linebreak(c: char) -> bool {
    in_ranges(LINEBREAK, c)
}

/// The `char-script-table` entry of `c`, as an index into `SCRIPT_NAMES`.
fn script(c: char) -> u16 {
    let u = c as u32;
    let k = SCRIPTS.partition_point(|&(_, last, _)| last < u);
    SCRIPTS.get(k).map_or(u16::MAX, |&(_, _, s)| s)
}

/// Does `c` have category `cat` (one the word-boundary lists name)?
fn has_category(c: char, cat: char) -> bool {
    CATEGORIES
        .iter()
        .find(|&&(k, _)| k == cat)
        .is_some_and(|&(_, table)| in_ranges(table, c))
}

/// `WORD_BOUNDARY_P (c1, c2)` (category.h): is there a word boundary
/// between two word-syntax chars `c1` then `c2`? Never between two chars
/// below 256; otherwise `word_boundary_p` (category.c): chars of one script
/// are split only by a `word-separating-categories` pair, chars of different
/// scripts are split unless a `word-combining-categories` pair joins them.
/// So `参见http` and `яhttp` have a word start at `h`, `éhttp` does not.
pub(crate) fn word_boundary(c1: char, c2: char) -> bool {
    if (c1 as u32) < 256 && (c2 as u32) < 256 {
        return false;
    }
    let (pairs, default) = if script(c1) == script(c2) {
        (WORD_SEPARATING_CATEGORIES, false)
    } else {
        (WORD_COMBINING_CATEGORIES, true)
    };
    let flips = pairs.iter().any(|&(a, b)| {
        a.is_none_or(|k| has_category(c1, k) && !has_category(c2, k))
            && b.is_none_or(|k| !has_category(c1, k) && has_category(c2, k))
    });
    default != flips
}

/// `\<` at a char `c2` preceded by `c1` (`None` at the start of the region,
/// `AT_STRINGS_BEG`): a word char that does not continue a word.
pub(crate) fn word_start(c1: Option<char>, c2: char) -> bool {
    is_word(c2) && c1.is_none_or(|c1| !is_word(c1) || word_boundary(c1, c2))
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
    use crate::chartables::SCRIPT_NAMES;

    #[test]
    fn tables_are_sorted_and_disjoint() {
        for t in [WHITESPACE, WORD, ALNUM, DELIM, ALPHA, LINEBREAK] {
            assert!(
                t.windows(2).all(|w| w[0].1 < w[1].0),
                "ranges overlap or are unsorted"
            );
            assert!(t.iter().all(|&(a, b)| a <= b));
        }
        assert!(CANON.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(!crate::chartables::EMACS_VERSION.is_empty());
    }

    /// `(looking-at "\\<")` between the two chars of an org-mode buffer
    /// holding exactly them, measured in Emacs 30.2: scripts (Han, Cyrillic,
    /// fullwidth Latin, Thai, Hangul, Greek, Arabic), the kana and Han
    /// category pairs, combining marks (category `^`), and the controls
    /// that must stay joined (`é`, digits, two Thai letters, Latin-1 pairs).
    #[test]
    fn word_starts_measured_in_emacs_30() {
        let measured = [
            ("参", "h", true),
            ("я", "h", true),
            ("ａ", "h", true),
            ("é", "h", false),
            ("1", "h", false),
            ("ก", "h", true),
            ("h", "ก", true),
            ("ก", "ข", false),
            ("a", "\u{301}", false),
            ("ก", "\u{e31}", false),
            ("h", "\u{e31}", false),
            ("漢", "ひ", false),
            ("漢", "カ", false),
            ("ひ", "カ", true),
            ("カ", "ひ", false),
            ("ひ", "漢", true),
            ("한", "h", true),
            ("α", "h", true),
            ("ب", "h", true),
            ("ä", "h", false),
            ("h", "я", true),
            ("ｈ", "ａ", false),
            ("é", "я", true),
            ("ǅ", "h", false),
        ];
        for (a, b, want) in measured {
            let (c1, c2) = (a.chars().next().unwrap(), b.chars().next().unwrap());
            assert_eq!(word_start(Some(c1), c2), want, "{a:?} {b:?}");
        }
        assert!(word_start(None, 'h'));
        assert!(!word_start(None, '-'));
        assert_eq!(SCRIPT_NAMES[script('я') as usize], "cyrillic");
        assert_eq!(SCRIPT_NAMES[script('\u{e01}') as usize], "thai");
    }

    #[test]
    fn script_runs_tile_every_code_point() {
        assert_eq!(SCRIPTS.first().map(|r| r.0), Some(0));
        assert_eq!(SCRIPTS.last().map(|r| r.1), Some(0x10FFFF));
        assert!(SCRIPTS.windows(2).all(|w| w[0].1 + 1 == w[1].0));
        assert!(SCRIPTS
            .iter()
            .all(|&(_, _, s)| (s as usize) < SCRIPT_NAMES.len()));
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
        // Syntax delimiters: `"` is string syntax, `(` and `]` parens, `,` and
        // `«` punctuation; `'` and `$` are WORD syntax in Org, so not delimiters.
        for c in ['"', '(', ']', ',', '\u{ab}', ':'] {
            assert!(is_delim(c), "{c:?}");
        }
        for c in ['\'', '$', 'a', ' ', '\u{a0}'] {
            assert!(!is_delim(c), "{c:?}");
        }
        assert!(is_alpha('\u{e9}') && is_alpha('\u{301}') && !is_alpha('1') && !is_alpha('_'));
        assert!(is_linebreak('\u{4e2d}') && !is_linebreak('a'));
    }
}
