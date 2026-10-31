//! Line recognisers: one function per `org-element` regexp, on a single line's
//! content (no line ending). Each is a hand-written matcher for the regexp it
//! names, so the crate needs no regex engine (ADR-112 D1: rowan only).
//!
//! Matching is ASCII case-insensitive wherever `org-element` binds
//! `case-fold-search` to `t`.

use crate::text::{
    is_blank, is_space, is_word, non_space_len, starts_with_ci, trim_end_blank, ws_len,
};

/// `^\*+ `: a headline's level, or `None`. Org 9.7 requires a SPACE after the
/// stars (`*\tx` and a lone `*` are not headlines; oracle-confirmed).
pub(crate) fn headline_level(s: &str) -> Option<usize> {
    let stars = s.bytes().take_while(|&b| b == b'*').count();
    (stars > 0 && s.as_bytes().get(stars) == Some(&b' ')).then_some(stars)
}

/// `org-comment-regexp`: `^[ \t]*#( |$)`.
pub(crate) fn is_comment(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    r == "#" || r.starts_with("# ")
}

/// `[ \t]*:( |$)`: a fixed-width line.
pub(crate) fn is_fixed_width(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    r == ":" || r.starts_with(": ")
}

/// `org-element-drawer-re`: `^[ \t]*:([-_[:word:]]+):[ \t]*$`. Returns the
/// name's byte range within `s`.
pub(crate) fn drawer_name(s: &str) -> Option<(usize, usize)> {
    let ws = ws_len(s);
    let r = s[ws..].strip_prefix(':')?;
    let n = r
        .find(|c: char| !(is_word(c) || c == '-' || c == '_'))
        .unwrap_or(r.len());
    let rest = r[n..].strip_prefix(':')?;
    (n > 0 && is_blank(rest)).then_some((ws + 1, ws + 1 + n))
}

/// `^[ \t]*:END:[ \t]*$` (any case).
pub(crate) fn is_drawer_end(s: &str) -> bool {
    drawer_name(s).is_some_and(|(a, b)| s[a..b].eq_ignore_ascii_case("END"))
}

/// `[ \t]*#\+BEGIN_(\S-+)`: the block name's byte range.
pub(crate) fn block_begin(s: &str) -> Option<(usize, usize)> {
    let at = ws_len(s) + 2;
    if !s[ws_len(s)..].starts_with("#+") || !starts_with_ci(&s[at..], "BEGIN_") {
        return None;
    }
    let name = at + 6;
    let n = non_space_len(&s[name..]);
    (n > 0).then_some((name, name + n))
}

/// `^[ \t]*#\+END_(\S-+)[ \t]*$`: the name, for the block-end index.
pub(crate) fn block_end_name(s: &str) -> Option<&str> {
    let at = ws_len(s) + 2;
    if !s[ws_len(s)..].starts_with("#+") || !starts_with_ci(&s[at..], "END_") {
        return None;
    }
    let name = &s[at + 4..];
    let n = non_space_len(name);
    (n > 0 && is_blank(&name[n..])).then_some(&name[..n])
}

/// `org-element-dynamic-block-open-re`: `^[ \t]*#\+BEGIN:[ \t]*(\w+)…`.
pub(crate) fn dynamic_begin(s: &str) -> Option<(usize, usize)> {
    let at = ws_len(s);
    if !starts_with_ci(&s[at..], "#+BEGIN:") {
        return None;
    }
    let name = at + 8 + ws_len(&s[at + 8..]);
    let n = s[name..]
        .find(|c: char| !is_word(c))
        .unwrap_or(s.len() - name);
    (n > 0).then_some((name, name + n))
}

/// `^[ \t]*#\+END:?[ \t]*$` (the dynamic-block parser's closing line).
pub(crate) fn is_dynamic_end(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    if !starts_with_ci(r, "#+END") {
        return false;
    }
    let rest = &r[5..];
    is_blank(rest.strip_prefix(':').unwrap_or(rest))
}

/// `^[ \t]*#\+END:[ \t]*$` exactly (the list scanner's dynamic-block skip).
pub(crate) fn is_dynamic_end_colon(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    starts_with_ci(r, "#+END:") && is_blank(&r[6..])
}

/// `[ \t]*#\+CALL:`.
pub(crate) fn is_babel_call(s: &str) -> bool {
    starts_with_ci(&s[ws_len(s)..], "#+CALL:")
}

/// A keyword line (`[ \t]*#\+\S-+:`): the key's byte range. The key is the
/// longest non-space run before a colon, as `#\+\(\S-*\):` matches greedily.
pub(crate) fn keyword_key(s: &str) -> Option<(usize, usize)> {
    let ws = ws_len(s);
    if !s[ws..].starts_with("#+") {
        return None;
    }
    let key = ws + 2;
    let run = &s[key..key + non_space_len(&s[key..])];
    let colon = run.rfind(':')?;
    // `(1+ (not space)) ":"` needs at least one character before a colon.
    colon_after_first_char(run).then_some((key, key + colon))
}

/// The affiliated keyword on this line, if any: `org-element--affiliated-re`.
/// Returns the end of the `#+KEY[…]:` prefix.
pub(crate) fn affiliated(s: &str) -> Option<usize> {
    let ws = ws_len(s);
    let r = s[ws..].strip_prefix("#+")?;
    let at = ws + 2;
    for dual in ["CAPTION", "RESULTS"] {
        if starts_with_ci(r, dual) {
            let rest = &r[dual.len()..];
            if let Some(after) = rest.strip_prefix(':') {
                return Some(s.len() - after.len());
            }
            if rest.starts_with('[') {
                // `\[\(.*\)\]` is greedy: the LAST `]:` on the line.
                return rest.rfind("]:").map(|i| at + dual.len() + i + 2);
            }
        }
    }
    const REGULAR: [&str; 11] = [
        "DATA", "HEADER", "HEADERS", "LABEL", "NAME", "PLOT", "RESNAME", "RESULT", "SOURCE",
        "SRCNAME", "TBLNAME",
    ];
    for k in REGULAR {
        if starts_with_ci(r, k) && r[k.len()..].starts_with(':') {
            return Some(at + k.len() + 1);
        }
    }
    if starts_with_ci(r, "ATTR_") {
        let n = r[5..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(r.len() - 5);
        if n > 0 && r[5 + n..].starts_with(':') {
            return Some(at + 5 + n + 1);
        }
    }
    None
}

/// `org-footnote-definition-re`: `^\[fn:([-_[:word:]]+)\]` at column 0.
/// Returns the end of the label.
pub(crate) fn footnote_definition(s: &str) -> Option<usize> {
    let r = s.strip_prefix("[fn:")?;
    let n = r
        .find(|c: char| !(is_word(c) || c == '-' || c == '_'))
        .unwrap_or(r.len());
    (n > 0 && r[n..].starts_with(']')).then_some(4 + n + 1)
}

/// `[ \t]*-----+[ \t]*$`.
pub(crate) fn is_horizontal_rule(s: &str) -> bool {
    let r = trim_end_blank(&s[ws_len(s)..]);
    r.len() >= 5 && r.bytes().all(|b| b == b'-')
}

/// `%%(` at column 0.
pub(crate) fn is_diary_sexp(s: &str) -> bool {
    s.starts_with("%%(")
}

/// `[ \t]*|`.
pub(crate) fn is_table_row(s: &str) -> bool {
    s[ws_len(s)..].starts_with('|')
}

/// A table.el rule: `[ \t]*\+(-+\+)+[ \t]*$`.
pub(crate) fn is_tableel_rule(s: &str) -> bool {
    let r = trim_end_blank(&s[ws_len(s)..]);
    let Some(body) = r.strip_prefix('+') else {
        return false;
    };
    !body.is_empty()
        && body.ends_with('+')
        && body
            .split('+')
            .rev()
            .skip(1)
            .all(|seg| !seg.is_empty() && seg.bytes().all(|b| b == b'-'))
}

/// The table-end regexp: a line that is blank or does not start (after
/// indentation) with `|` (org) or `|`/`+` (table.el).
pub(crate) fn ends_table(s: &str, table_el: bool) -> bool {
    match s[ws_len(s)..].chars().next() {
        None => true,
        Some('|') => false,
        Some('+') => !table_el,
        Some(c) => !matches!(c, ' ' | '\t'),
    }
}

/// `[ \t]*#\+TBLFM: +`.
pub(crate) fn is_tblfm(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    starts_with_ci(r, "#+TBLFM:") && r[8..].starts_with(' ')
}

/// The text-line test of `org-element-paragraph-separate` (minus headlines
/// and lists, which the caller checks): a line that MAY start an element.
pub(crate) fn may_separate(s: &str) -> bool {
    if footnote_definition(s).is_some() || is_diary_sexp(s) {
        return true;
    }
    let r = &s[ws_len(s)..];
    match r.as_bytes().first() {
        None | Some(b'|') => true,
        Some(b'+') => is_tableel_rule(s),
        Some(b'#') => is_comment(s) || block_begin(s).is_some() || keyword_like(r),
        Some(b':') => is_fixed_width(s) || drawer_name(s).is_some(),
        Some(b'-') => is_horizontal_rule(s),
        Some(b'\\') => latex_begin(s).is_some(),
        _ => is_clock(s),
    }
}

/// `#\+\S-+(\[.*\])?:` as a separator candidate.
fn keyword_like(r: &str) -> bool {
    let Some(rest) = r.strip_prefix("#+") else {
        return false;
    };
    let run = &rest[..non_space_len(rest)];
    colon_after_first_char(run) || bracketed_keyword_is_dual(r).is_some()
}

/// `(1+ (not space)) ":"` within a non-space run: a colon after the first
/// character (char-aware, so a multi-byte first character cannot split).
fn colon_after_first_char(run: &str) -> bool {
    run.char_indices().skip(1).any(|(_, c)| c == ':')
}

/// `[ \t]*#\+(\S-+)\[.*\]:`: a keyword written with a secondary value. Only
/// dual keywords (CAPTION, RESULTS) may be written that way and still
/// separate paragraphs. Returns `Some(is_dual)` when the shape matches.
pub(crate) fn bracketed_keyword_is_dual(s: &str) -> Option<bool> {
    let r = s[ws_len(s)..].strip_prefix("#+")?;
    let open = r.find('[')?;
    let name = &r[..open];
    if name.is_empty() || name.chars().any(is_space) || !r[open..].contains("]:") {
        return None;
    }
    Some(name.eq_ignore_ascii_case("CAPTION") || name.eq_ignore_ascii_case("RESULTS"))
}

/// `org-element--latex-begin-environment`: the environment name.
pub(crate) fn latex_begin(s: &str) -> Option<&str> {
    let r = s[ws_len(s)..].strip_prefix("\\begin{")?;
    let n = r
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '*'))
        .unwrap_or(r.len());
    (n > 0 && r[n..].starts_with('}')).then_some(&r[..n])
}

/// `\\end{NAME}[ \t]*$`: the name of an environment this line closes.
pub(crate) fn latex_end_name(s: &str) -> Option<&str> {
    let r = trim_end_blank(s).strip_suffix('}')?;
    let at = r.rfind("\\end{")?;
    let name = &r[at + 5..];
    (!name.is_empty()).then_some(name)
}

/// `org-element-planning-line-re`: `^[ \t]*(CLOSED:|DEADLINE:|SCHEDULED:)`.
pub(crate) fn is_planning(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    ["CLOSED:", "DEADLINE:", "SCHEDULED:"]
        .iter()
        .any(|k| starts_with_ci(r, k))
}

/// `org-element-clock-line-re`.
pub(crate) fn is_clock(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    if !starts_with_ci(r, "CLOCK:") {
        return false;
    }
    let r = &r[6..];
    let after_ws = &r[ws_len(r)..];
    if after_ws.len() == r.len() {
        return false; // `(1+ (or ?\t ?\s))` after the colon
    }
    if let Some(rest) = duration(r) {
        return is_blank(rest);
    }
    let Some(rest) = inactive_timestamp(after_ws) else {
        return false;
    };
    if let Some(range) = rest.strip_prefix("--") {
        let Some(rest) = inactive_timestamp(range) else {
            return false;
        };
        return duration(rest).is_some_and(is_blank);
    }
    is_blank(rest)
}

/// `(1+ blank) "=>" (1+ blank) (1+ digit) ":" digit digit`: what follows.
fn duration(s: &str) -> Option<&str> {
    let ws = ws_len(s);
    let r = s[ws..].strip_prefix("=>")?;
    let ws2 = ws_len(r);
    let r = &r[ws2..];
    let d = r.bytes().take_while(u8::is_ascii_digit).count();
    let mm = r[d..].strip_prefix(':')?;
    let ok = ws > 0
        && ws2 > 0
        && d > 0
        && mm.len() >= 2
        && mm.as_bytes()[..2].iter().all(u8::is_ascii_digit);
    ok.then(|| &mm[2..])
}

/// `org-ts-regexp-inactive`: `[YYYY-MM-DD( +DAY)?( +H?H:MM)?]`; returns what
/// follows the closing bracket.
fn inactive_timestamp(s: &str) -> Option<&str> {
    let r = s.strip_prefix('[')?;
    let close = r.find(']')?;
    let inner = &r[..close];
    let b = inner.as_bytes();
    let date_ok = b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit);
    (date_ok && !inner[10..].contains(['\r', '\n'])).then(|| &r[close + 1..])
}

/// A plain-list bullet per `org-item-re` (`org-list-allow-alphabetical` off):
/// `[ \t]*([-+]|[0-9]+[.)])` or `[ \t]+\*`, then blank or end of line. Returns
/// the bullet's byte range.
pub(crate) fn item_bullet(s: &str) -> Option<(usize, usize)> {
    let ws = ws_len(s);
    let r = &s[ws..];
    let n = match r.as_bytes().first()? {
        b'-' | b'+' => 1,
        b'*' if ws > 0 => 1,
        b'0'..=b'9' => {
            let d = r.bytes().take_while(u8::is_ascii_digit).count();
            if !matches!(r.as_bytes().get(d), Some(b'.' | b')')) {
                return None;
            }
            d + 1
        }
        _ => return None,
    };
    let after = r.as_bytes().get(n);
    matches!(after, None | Some(b' ' | b'\t')).then_some((ws, ws + n))
}

/// The paragraph-separator list alternative: `[ \t]*([-+*]|[0-9]+[.)])([ \t]|$)`.
/// Unlike [`item_bullet`] this accepts a `*` at column 0.
pub(crate) fn list_like(s: &str) -> bool {
    let ws = ws_len(s);
    let r = &s[ws..];
    if r.starts_with('*') {
        return matches!(r.as_bytes().get(1), None | Some(b' ' | b'\t'));
    }
    item_bullet(s).is_some()
}

/// A property line inside a property drawer:
/// `[ \t]*:\S-+:([ \t].*)?[ \t]*$`.
pub(crate) fn is_property_line(s: &str) -> bool {
    let r = &s[ws_len(s)..];
    let Some(rest) = r.strip_prefix(':') else {
        return false;
    };
    let run = &rest[..non_space_len(rest)];
    let after = &rest[run.len()..];
    // `\S-+:` greedy with backtracking: some colon in the run, after at least
    // one character, followed by blank or end of line. Only the run's final
    // character can be followed by blank or end of line.
    run.len() >= 2 && run.ends_with(':') && (after.is_empty() || after.starts_with([' ', '\t']))
}

/// `^[ \t]*:PROPERTIES:[ \t]*$` (any case).
pub(crate) fn is_properties_open(s: &str) -> bool {
    drawer_name(s).is_some_and(|(a, b)| s[a..b].eq_ignore_ascii_case("PROPERTIES"))
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
