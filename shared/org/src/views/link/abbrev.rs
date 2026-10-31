//! `#+LINK:` abbreviations (`org-link-abbrev-alist-local`) and
//! `org-link-expand-abbrev`, which rewrites a bracket link's raw link.

use crate::chars::is_space;
use crate::kind::SyntaxNode;
use crate::views::Keyword;

/// A document's link abbreviations, in Org's lookup order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkAbbrevs {
    /// (key, replacement): later `#+LINK:` lines first, as `nreverse` leaves
    /// them, so `assoc` finds the last definition of a key.
    entries: Vec<(String, String)>,
}

impl LinkAbbrevs {
    /// The `#+LINK: KEY REPLACEMENT` keywords of the document under `root`.
    pub fn of(root: &SyntaxNode) -> LinkAbbrevs {
        let mut entries: Vec<(String, String)> = root
            .descendants()
            .filter_map(Keyword::cast)
            .filter(|k| k.key().eq_ignore_ascii_case("LINK"))
            .filter_map(|k| parse(&k.value()))
            .collect();
        entries.reverse();
        LinkAbbrevs { entries }
    }

    /// `org-link-expand-abbrev`: `KEY` or `KEY:TAG` (`KEY::TAG`) with a
    /// known KEY becomes its replacement, with `%s` (or `%h`, URL-encoded)
    /// replaced by TAG, or TAG appended.
    pub fn expand(&self, link: &str) -> String {
        let (key, tag) = match link.find(':') {
            Some(k) => {
                let rest = &link[k + 1..];
                (&link[..k], Some(rest.strip_prefix(':').unwrap_or(rest)))
            }
            None => (link, None),
        };
        let Some((_, rpl)) = self.entries.iter().find(|(k, _)| k == key) else {
            return link.to_string();
        };
        let tag = tag.unwrap_or("");
        if calls_function(rpl) {
            link.to_string()
        } else if rpl.contains("%s") {
            rpl.replacen("%s", tag, 1)
        } else if rpl.contains("%h") {
            rpl.replacen("%h", &hexify(tag), 1)
        } else {
            format!("{rpl}{tag}")
        }
    }
}

/// `%(\([^)]+\))` in a replacement.
fn calls_function(rpl: &str) -> bool {
    rpl.match_indices("%(").any(|(i, _)| {
        let rest = &rpl[i + 2..];
        rest.find(')').is_some_and(|k| k > 0)
    })
}

/// A `#+LINK:` value: `"KEY WITH SPACES" REPLACEMENT` (the last suitable
/// quote ends the key) or `KEY REPLACEMENT`.
fn parse(value: &str) -> Option<(String, String)> {
    quoted(value).or_else(|| {
        let n = value
            .char_indices()
            .find(|&(_, c)| is_space(c))
            .map_or(value.len(), |(i, _)| i);
        let rest = &value[n..];
        let rpl = rest.trim_start_matches([' ', '\t']);
        (n > 0 && rpl.len() < rest.len() && !rpl.is_empty() && !rpl.contains('\n'))
            .then(|| (value[..n].to_string(), rpl.to_string()))
    })
}

/// `\`"\(.+[^\]\)"[ \t]+\(.+\)`.
fn quoted(value: &str) -> Option<(String, String)> {
    let inner = value.strip_prefix('"')?;
    inner.match_indices('"').rev().find_map(|(q, _)| {
        let key = &inner[..q];
        let rest = &inner[q + 1..];
        let rpl = rest.trim_start_matches([' ', '\t']);
        let ok = key.chars().count() >= 2
            && !key.ends_with('\\')
            && rpl.len() < rest.len()
            && !rpl.is_empty();
        ok.then(|| (key.to_string(), rpl.to_string()))
    })
}

/// `url-hexify-string`: every byte outside `[A-Za-z0-9._~-]` as `%XX`.
fn hexify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abbrevs(lines: &[(&str, &str)]) -> LinkAbbrevs {
        let mut entries: Vec<(String, String)> = lines
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        entries.reverse();
        LinkAbbrevs { entries }
    }

    #[test]
    fn expansion_follows_org_link_expand_abbrev() {
        let a = abbrevs(&[
            ("git", "https://g.org/?id=%s"),
            ("doc", "https://d.org/#%h"),
            ("wiki", "https://w.org/"),
            ("dup", "first:"),
            ("dup", "last:"),
            ("fn", "%(my-function)"),
        ]);
        assert_eq!(a.expand("git:4bbd"), "https://g.org/?id=4bbd");
        assert_eq!(a.expand("git::4bbd"), "https://g.org/?id=4bbd");
        assert_eq!(a.expand("git"), "https://g.org/?id=");
        assert_eq!(a.expand("doc:a b/é"), "https://d.org/#a%20b%2F%C3%A9");
        assert_eq!(a.expand("wiki:Page"), "https://w.org/Page");
        // The last definition of a key wins.
        assert_eq!(a.expand("dup:x"), "last:x");
        // Unknown keys and function replacements are left alone.
        assert_eq!(a.expand("other:x"), "other:x");
        assert_eq!(a.expand("fn:x"), "fn:x");
        // The key is everything before the FIRST colon, case-sensitively.
        assert_eq!(a.expand("Git:4bbd"), "Git:4bbd");
    }

    #[test]
    fn link_keyword_values_parse_like_org_set_regexps_and_options() {
        assert_eq!(
            parse("git https://x/%s"),
            Some(("git".into(), "https://x/%s".into()))
        );
        assert_eq!(
            parse("\"my key\"\thttps://y"),
            Some(("my key".into(), "https://y".into()))
        );
        // A key alone, or a key separated by a no-break space, is no abbreviation.
        assert_eq!(parse("lonely"), None);
        assert_eq!(parse("a\u{a0}b"), None);
    }
}
