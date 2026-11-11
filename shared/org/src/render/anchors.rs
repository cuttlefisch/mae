//! Anchor ids: one namespace per page, collision-free by construction.
//!
//! Every id is a fixed PREFIX naming its kind, then a body:
//! - `id-` + enc(`:ID:`) for a KB node (a headline's section, or a list
//!   item);
//! - `cid-` + enc(`CUSTOM_ID`) for a headline's section;
//! - `h-` + [`slug`] of the headline's title (`h` when the slug is empty),
//!   on EVERY headline's heading: the anchor a KB link's `#fragment` lands
//!   on. A repeat gets `--2`, `--3`, … until free;
//! - `t-N` for the N-th distinct `<<target>>` of the page and `r-N` for the
//!   N-th radio target, in order of first appearance: opaque, as ox-html's
//!   generated ids are, so no attribute spells a target's name (Org never
//!   shows a dedicated target's text). Targets are file-local: a link
//!   reaches one only on its own page, by name, through the page's map;
//! - `fn-N` and `fnr-N` (`fnr-N-K` for the K-th repeat) for footnotes.
//!
//! The prefixes are distinct and none is a prefix of another, and within a
//! kind every body is unique, so no two anchors on a page can collide. A
//! slug never contains `--` nor starts with `-`, so a heading's repeat
//! suffix can never spell another title's slug (`#x--2` is the second `x`,
//! never a title "x 2", which is `h-x-2`).
//!
//! `enc` keeps ASCII letters, digits, `_` and `-`, and writes every other
//! byte of the UTF-8 as `.` plus two lower-case hex digits: injective (a
//! `.` is never kept, so it always starts an escape), deterministic, and
//! limited to `[A-Za-z0-9_.-]`. A slug keeps letters and digits of any
//! script (Emacs's `[[:alnum:]]`), case-folded by Emacs's table, so a
//! heading anchor may hold non-ASCII letters (`h-глава-2`); every id needs
//! no escaping in HTML (attribute values are escaped as always) and is
//! percent-encoded where it becomes part of another page's URL.

use crate::chars::{canon, is_alnum};
use crate::seeded::{HashMap, HashSet};

/// `enc` of the module doc.
pub fn encode_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push('.');
            out.push(hex(b >> 4));
            out.push(hex(b & 0xf));
        }
    }
    out
}

fn hex(n: u8) -> char {
    char::from_digit(u32::from(n), 16).unwrap_or('0')
}

/// The most bytes of a title a slug keeps (before its prefix and any
/// repeat suffix).
pub const SLUG_MAX: usize = 64;

/// A text as a slug: its letters and digits (Emacs's `[[:alnum:]]`, any
/// script), each case-folded by Emacs's case-fold table (`canon`, the
/// crate's generated one, so every peer and toolchain agrees: what
/// case-insensitive matching equates, lower case for most letters, but a
/// few, such as `İ`, fold to themselves); every run of anything else is one
/// `-`; no `-` at either end; at most [`SLUG_MAX`] bytes, cut on a char
/// boundary and never before a `-`. Empty when the text has no letter or
/// digit. Never contains `--`. Idempotent: `slug(slug(t)) == slug(t)`.
///
/// A heading's anchor slugs its TITLE AS RENDERED (inline-only, babel
/// objects and export snippets dropped: `H_2O levels` is `H2O levels`), and
/// a KB link's `#fragment` is rendered the same way before it is slugged.
/// So a fragment written as the title (`id:x#H_2O levels`) and one written
/// as its slug (`id:x#h2o-levels`) both name that heading. In that
/// rendering a link is its description only, and a link without one adds
/// nothing, so no anchor depends on a resolver or on the rest of the page.
///
/// Slugging loses distinctions, by design: titles that differ only in
/// case, punctuation or blanks share a slug (`A b`, `A-b` and `a, b` are
/// all `a-b`). The second such heading gets `--2`, and a fragment naming
/// either lands on the first.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.chars() {
        let k = canon(c);
        if is_alnum(k) {
            let sep = dash && !out.is_empty();
            // The separator and the character must both fit: a slug never
            // ends in `-` and never exceeds the limit.
            if out.len() + usize::from(sep) + k.len_utf8() > SLUG_MAX {
                break;
            }
            if sep {
                out.push('-');
            }
            dash = false;
            out.push(k);
        } else {
            dash = true;
        }
    }
    out
}

/// The anchor of a heading whose title has no letter or digit. Not the bare
/// `h` it was: an element id becomes a property of `window` in a browser, and
/// `h` is a plausible script name; `h--untitled` is no JavaScript identifier,
/// and neither a slug (which never holds `--`) nor a repeat suffix (`--` and
/// digits) can spell it.
pub const UNTITLED_ANCHOR: &str = "h--untitled";

/// The anchor every rendered headline's heading carries: `h-` + the slug of
/// its title, or [`UNTITLED_ANCHOR`] when the slug is empty (before any repeat
/// suffix).
pub fn heading_anchor(title: &str) -> String {
    let s = slug(title);
    if s.is_empty() {
        UNTITLED_ANCHOR.to_string()
    } else {
        format!("h-{s}")
    }
}

/// The ids already given on a page.
#[derive(Debug, Default)]
pub(crate) struct Ids {
    used: HashSet<String>,
    /// Per base, the next suffix to try: N repeats of one title cost O(N),
    /// not a scan from `-2` per repeat (O(N²); `tests/scaling.rs`).
    next: HashMap<String, usize>,
}

impl Ids {
    /// `base` if free, else `base--2`, `base--3`, … the first free one.
    /// A slug holds no `--`, so a repeat never spells another title's
    /// anchor.
    pub fn claim(&mut self, base: String) -> String {
        if self.used.insert(base.clone()) {
            return base;
        }
        let n = self.next.entry(base.clone()).or_insert(2);
        loop {
            let candidate = format!("{base}--{n}");
            *n += 1;
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    /// Claims `id` only if it is free.
    pub fn try_claim(&mut self, id: String) -> Option<String> {
        self.used.insert(id.clone()).then_some(id)
    }
}

/// Whether `id` has the shape every anchor this module builds has: an
/// ASCII letter, then ASCII letters, digits, `_`, `.` and `-`, and the
/// non-ASCII letters and digits a slug keeps (case-folded by Emacs's case
/// table, so never a char the table would change).
pub fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '-')
                || (!c.is_ascii() && is_alnum(c) && canon(c) == c)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_injective_on_the_escape_character_and_its_spellings() {
        // `.` is always escaped, so `a.2e` (written) and `a.` differ.
        assert_eq!(encode_id("a."), "a.2e");
        assert_eq!(encode_id("a.2e"), "a.2e2e");
        assert_ne!(encode_id("a."), encode_id("a.2e"));
        assert_eq!(encode_id("kb:x/y"), "kb.3ax.2fy");
        assert_eq!(encode_id("\"<a b>"), ".22.3ca.20b.3e");
        // A Cyrillic `а` is its bytes, never a Latin `a`.
        assert_eq!(encode_id("\u{430}"), ".d0.b0");
    }

    #[test]
    fn repeats_get_a_suffix_no_slug_can_spell() {
        let mut ids = Ids::default();
        assert_eq!(ids.claim(heading_anchor("a")), "h-a");
        // A title "a 2" is `h-a-2`, never the second "a".
        assert_eq!(ids.claim(heading_anchor("a 2")), "h-a-2");
        assert_eq!(ids.claim(heading_anchor("a")), "h-a--2");
        assert_eq!(ids.claim(heading_anchor("a")), "h-a--3");
        assert_eq!(ids.claim(heading_anchor("¿?")), "h--untitled");
        assert_eq!(ids.claim(heading_anchor("")), "h--untitled--2");
        assert_eq!(slug("  Hello, World! "), "hello-world");
        assert_eq!(slug("Café au lait"), "café-au-lait");
        assert_eq!(slug("Глава 2"), "глава-2");
        assert_eq!(slug("ÉCOLE--Straße"), "école-straße");
        assert_eq!(slug("中文 标题"), "中文-标题");
        assert_eq!(slug("¿?"), "");
        assert_eq!(slug("x 2"), "x-2");
        // Emacs's table, not Rust's `to_lowercase` (which a toolchain
        // update may change): a final sigma folds to sigma, the Kelvin
        // sign stays itself.
        assert_eq!(slug("Λόγος ς"), "λόγοσ-σ");
        assert_eq!(slug("5 \u{212a}"), "5-\u{212a}");
    }

    /// Idempotence rests on the case table: a kept char's canonical form is
    /// itself kept, and canonical twice is canonical once. Every char.
    #[test]
    fn the_case_table_keeps_slugs_idempotent() {
        for u in 0..=0x10FFFFu32 {
            let Some(c) = char::from_u32(u) else { continue };
            let k = canon(c);
            if is_alnum(k) {
                assert_eq!(canon(k), k, "U+{u:04X}");
                assert!(is_valid_id(&format!("h-{k}")), "U+{u:04X}");
            }
        }
    }

    #[test]
    fn a_cut_slug_ends_on_a_char_and_never_on_a_dash() {
        let t = format!("{} {}", "a".repeat(SLUG_MAX - 1), "b");
        assert_eq!(slug(&t), "a".repeat(SLUG_MAX - 1));
        let t = format!("{}é", "a".repeat(SLUG_MAX - 1));
        assert_eq!(slug(&t), "a".repeat(SLUG_MAX - 1));
        let t = "я".repeat(SLUG_MAX);
        assert_eq!(slug(&t).len(), SLUG_MAX);
    }
}
