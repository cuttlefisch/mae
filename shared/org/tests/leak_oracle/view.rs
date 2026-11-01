//! Text helpers shared by the leak oracle and `examples/probe.rs` (which
//! includes this file by path): marker scanning and attribute stripping.
//! Standard library only.
#![allow(dead_code)]

use std::collections::BTreeSet;

/// The marker for number `n`: lowercase alphanumeric, so it survives
/// slugging and case folding, and digits only between the fences, so no
/// marker is a substring of another.
pub fn marker(n: u32) -> String {
    format!("qz{n}qz")
}

/// Every marker number `s` holds, case-insensitively.
pub fn scan(s: &str) -> BTreeSet<u32> {
    let b = s.to_ascii_lowercase().into_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if &b[i..i + 2] != b"qz" {
            i += 1;
            continue;
        }
        let digits = b[i + 2..].iter().take_while(|c| c.is_ascii_digit()).count();
        let end = i + 2 + digits;
        let closed = digits > 0 && digits <= 9 && b.get(end..end + 2) == Some(b"qz".as_slice());
        if !closed {
            i += 1;
            continue;
        }
        let n = std::str::from_utf8(&b[i + 2..end]).unwrap_or("0");
        out.insert(n.parse().unwrap_or(0));
        i = end + 2;
    }
    out
}

/// `s` with the VALUE of every `attr` attribute removed: in HTML
/// (`attr="…"`), in HTML inside a JSON string (`attr=\"…\"`), and in hast
/// JSON (`"attr":"…"`).
pub fn strip_attr(s: &str, attr: &str) -> String {
    let escaped = format!("{attr}=\\\"");
    let in_json = strip_between(s, &escaped, |v| v.find("\\\"").unwrap_or(v.len()));
    let html = strip_between(&in_json, &format!("{attr}=\""), |v| {
        v.find('"').unwrap_or(v.len())
    });
    strip_between(&html, &format!("\"{attr}\":\""), json_string_end)
}

fn strip_between(s: &str, open: &str, end: impl Fn(&str) -> usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i + open.len()]);
        let after = &rest[i + open.len()..];
        rest = &after[end(after)..];
    }
    out.push_str(rest);
    out
}

/// The byte offset of the unescaped `"` ending a JSON string.
fn json_string_end(s: &str) -> usize {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if c == '"' && !escaped {
            return i;
        }
        escaped = c == '\\' && !escaped;
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_reads_adjacent_and_cased_markers() {
        let got = scan("aQZ12QZqz3qzqz qzqz4qz qz5 qz6qzz");
        assert_eq!(got, [3, 4, 6, 12].into_iter().collect());
    }

    #[test]
    fn strip_attr_removes_values_in_every_spelling() {
        let s = r#"<a href="/x/qz1qz">qz2qz</a> {"href":"q\"z qz3qz"} "<a href=\"/qz4qz\">qz5qz""#;
        assert_eq!(scan(&strip_attr(s, "href")), [2, 5].into_iter().collect());
        assert_eq!(
            scan(&strip_attr(s, "id")),
            [1, 2, 3, 4, 5].into_iter().collect()
        );
    }
}
