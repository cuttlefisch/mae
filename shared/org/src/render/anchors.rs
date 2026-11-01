//! Anchor ids: one namespace per page, collision-free by construction.
//!
//! Every id is a fixed PREFIX naming its kind, then a body:
//! - `id-` + enc(`:ID:`) for a KB node (a headline's section, or a list
//!   item);
//! - `cid-` + enc(`CUSTOM_ID`) for a headline's section;
//! - `h-` + a slug of the headline's title, on EVERY headline's heading
//!   (the anchor a KB link's `#fragment` lands on, see [`slug`]);
//! - `t-` + enc(text) for a `<<target>>`; `r-` + enc(text) for a radio target;
//! - `fn-N` and `fnr-N` (`fnr-N-K` for the K-th repeat) for footnotes.
//!
//! The prefixes are distinct and none is a prefix of another, and within a
//! kind every body is unique (a repeat gets `-2`, `-3`, … until free), so no
//! two anchors on a page can collide. `enc` keeps ASCII letters, digits, `_`
//! and `-`, and writes every other byte of the UTF-8 as `.` plus two
//! lower-case hex digits: injective (a `.` is never kept, so it always
//! starts an escape), deterministic, and limited to `[A-Za-z0-9_.-]`, which
//! needs no escaping in HTML, URL fragments or CSS identifiers after the
//! prefix. A Unicode confusable is therefore never an id character: it is
//! its bytes, spelled out.

use std::collections::{HashMap, HashSet};

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

/// A headline title as a slug: ASCII letters and digits, lower-cased; every
/// run of anything else is one `-`; no `-` at either end; at most
/// [`SLUG_MAX`] bytes. Empty when the title has no ASCII letter or digit.
///
/// Idempotent (`slug(slug(t)) == slug(t)`), so a KB link fragment written
/// as a slug (`id:x#rope-internals`) and one written as the title
/// (`id:x#Rope internals`) name the same heading.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            let sep = dash && !out.is_empty();
            // The separator and the character must both fit: a slug never
            // ends in `-` and never exceeds the limit.
            if out.len() + usize::from(sep) + 1 > SLUG_MAX {
                break;
            }
            if sep {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    out
}

/// The anchor every rendered headline's heading carries: `h-` + the slug of
/// its title, or `h` when the slug is empty (before any repeat suffix).
pub fn heading_anchor(title: &str) -> String {
    let s = slug(title);
    if s.is_empty() {
        "h".to_string()
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
    /// `base` if free, else `base-2`, `base-3`, … the first free one.
    pub fn claim(&mut self, base: String) -> String {
        if self.used.insert(base.clone()) {
            return base;
        }
        let n = self.next.entry(base.clone()).or_insert(2);
        loop {
            let candidate = format!("{base}-{n}");
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

/// Whether `id` has the shape every anchor this module builds has.
pub fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
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
    fn repeats_get_the_first_free_suffix() {
        let mut ids = Ids::default();
        assert_eq!(ids.claim("h-a".into()), "h-a");
        assert_eq!(ids.claim("h-a-2".into()), "h-a-2");
        assert_eq!(ids.claim("h-a".into()), "h-a-3");
        assert_eq!(slug("  Hello, World! "), "hello-world");
        assert_eq!(slug("Café au lait"), "caf-au-lait");
        assert_eq!(slug("¿?"), "");
    }
}
