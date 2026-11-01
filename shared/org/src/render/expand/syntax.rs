//! Macro syntax as Org's export reads it: a macro object's name and
//! arguments, the regexp `org-macro-replace-all` searches parsed keywords
//! with, and how arguments split and substitute.

use super::Found;
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;

/// A macro object's start, end (post-blank excluded: Org keeps the blanks
/// after a macro), name and raw arguments (`None` without parentheses).
pub(super) fn object_macro(n: &SyntaxNode) -> Option<Found> {
    let start = usize::from(n.text_range().start());
    let mut name = None;
    let mut args = None;
    let mut end = start;
    let mut open = false;
    // The arguments are every token between the parentheses (a value
    // across lines is several).
    for t in n.children_with_tokens().filter_map(|e| e.into_token()) {
        match t.kind() {
            NAME => name = Some(t.text().to_string()),
            MARKUP if t.text() == "(" => open = true,
            MARKUP => {}
            WHITESPACE if !open || t.text_range().end() == n.text_range().end() => continue,
            _ if open => args.get_or_insert_with(String::new).push_str(t.text()),
            _ => continue,
        }
        end = usize::from(t.text_range().end());
    }
    let args = if open {
        Some(args.unwrap_or_default())
    } else {
        None
    };
    Some((start, end, name?, args))
}

/// Every macro object of `tree`, in order.
pub(super) fn object_macros(tree: &SyntaxNode) -> Vec<Found> {
    tree.descendants()
        .filter(|d| d.kind() == MACRO && !in_citation(d))
        .filter_map(|d| object_macro(&d))
        .collect()
}

/// Whether macro object `n` is inside a citation: Org's export does not
/// expand it there (`org-macro-replace-all` never sees it as a macro), and
/// it exports as nothing (Emacs 30.2: `[cite:@k {{{n}}}] {{{n}}}` gives
/// `(??, ????) 1`).
pub(super) fn in_citation(n: &SyntaxNode) -> bool {
    n.ancestors().any(|a| a.kind() == CITATION)
}

/// Whether node property `n` is an `EXPORT_` property of a parsed keyword
/// (`EXPORT_TITLE`, `EXPORT_TITLE+`, any case).
pub(crate) fn is_export_property(n: &SyntaxNode) -> bool {
    let key = n
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == NAME)
        .map(|t| t.text().to_ascii_uppercase())
        .unwrap_or_default();
    let key = key.strip_suffix('+').unwrap_or(&key);
    key.strip_prefix("EXPORT_")
        .is_some_and(|k| super::PARSED_KEYWORDS.contains(&k))
}

/// `org-element-macro-parser` at `i` of `s`: `{{{NAME}}}` or
/// `{{{NAME(ARGS)}}}`, NAME `[a-zA-Z][-a-zA-Z0-9_]*`, ARGS the shortest
/// text without a NUL before `)}}}`. Its end, name and arguments.
///
/// `close(p)` is the first `)}}}` at or after `p`.
fn macro_at(
    s: &str,
    i: usize,
    close: &mut impl FnMut(usize) -> Option<usize>,
) -> Option<(usize, String, Option<String>)> {
    let b = s.as_bytes();
    let n = i + 3;
    if !s[i..].starts_with("{{{") || !b.get(n).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let ne = n + b[n..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b'_')
        .count();
    let name = s[n..ne].to_string();
    if b.get(ne) == Some(&b'(') {
        if let Some(k) = close(ne + 1) {
            let args = &s[ne + 1..k];
            if !args.contains('\0') {
                return Some((k + 4, name, Some(args.to_string())));
            }
        }
    }
    s[ne..].starts_with("}}}").then(|| (ne + 3, name, None))
}

/// Every macro `org-macro-replace-all`'s regexp (`{{{[-A-Za-z0-9_]`)
/// finds starting in `s[from..to]`, read by the macro parser on `s`, in
/// order. After one that is left (a `results` macro without template) the
/// search goes on inside it, as Org's does; after any other it goes on
/// after it. Linear: the `)}}}` searches go forward through `s` once.
pub(super) fn raw_macros(s: &str, from: usize, to: usize) -> Vec<Found> {
    let mut out = Vec::new();
    let mut last: Option<(usize, Option<usize>)> = None;
    let mut close = |p: usize| {
        if let Some((q, c)) = last {
            if q <= p && c.is_none_or(|c| c >= p) {
                return c;
            }
        }
        let c = s[p..].find(")}}}").map(|k| p + k);
        last = Some((p, c));
        c
    };
    let mut i = from;
    while let Some(k) = s.get(i..to).and_then(|r| r.find("{{{")) {
        let at = i + k;
        let next = s[at + 3..].chars().next();
        if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            i = at + 3;
            continue;
        }
        match macro_at(s, at, &mut close) {
            Some((end, name, args)) => {
                let left = fold(&name) == "results";
                out.push((at, end, name, args));
                i = if left { at + 4 } else { end };
            }
            None => i = at + 4,
        }
    }
    out
}

pub(super) fn fold(s: &str) -> String {
    crate::chars::fold(s)
}

/// `org-element-macro-parser`'s arguments then `org-macro-extract-arguments`:
/// the text trimmed, each run of blanks (space, tab, CR, newline) one space,
/// split at each comma not escaped (a run of N backslashes before a comma
/// halved, an odd run escaping it). `None` without parentheses.
pub(crate) fn extract_arguments(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?.trim_matches(crate::text::is_trim);
    let mut collapsed = String::with_capacity(raw.len());
    for c in raw.chars() {
        let blank = matches!(c, ' ' | '\t' | '\r' | '\n');
        if !(blank && collapsed.ends_with(' ')) {
            collapsed.push(if blank { ' ' } else { c });
        }
    }
    let mut args = vec![String::new()];
    let mut slashes = 0usize;
    for c in collapsed.chars() {
        let cur = args.last_mut().expect("one argument at least");
        match c {
            '\\' => slashes += 1,
            ',' => {
                cur.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    cur.push(',');
                } else {
                    args.push(String::new());
                }
                slashes = 0;
            }
            c => {
                cur.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                cur.push(c);
            }
        }
    }
    let cur = args.last_mut().expect("one argument at least");
    cur.extend(std::iter::repeat_n('\\', slashes));
    Some(args)
}

/// `org-macro-expand` on a text template: each `$N` the N-th argument or
/// nothing, `$0` the first (`(nth (1- 0) args)` is `(nth -1 args)`).
pub(crate) fn substitute(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(k) = rest.find('$') {
        out.push_str(&rest[..k]);
        let digits = rest[k + 1..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            out.push('$');
            rest = &rest[k + 1..];
            continue;
        }
        let n = rest[k + 1..k + 1 + digits].parse::<usize>().ok();
        let i = n.map(|n| n.saturating_sub(1));
        if let Some(a) = i.and_then(|i| args.get(i)) {
            out.push_str(a);
        }
        rest = &rest[k + 1 + digits..];
    }
    out.push_str(rest);
    out
}

/// `(string-match-p "\\`(eval\\>" value)`: a template Org evaluates.
pub(super) fn is_eval(t: &str) -> bool {
    t.strip_prefix("(eval")
        .is_some_and(|rest| !rest.starts_with(crate::chars::is_word))
}
