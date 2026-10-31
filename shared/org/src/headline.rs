//! A headline's line: `STARS [TODO] [PRIORITY] [COMMENT] TITLE [:TAGS:]`,
//! as `org-element--headline-parse-title` reads it.

use crate::kind::SyntaxKind::{self, *};
use crate::options::TodoKeywords;
use crate::text::{is_trim, trim_end_blank, ws_len};

/// The tokens of headline line `s`, as `(kind, start, end)` relative to `s`.
/// They cover `s` exactly.
pub(crate) fn tokens(s: &str, todo: &TodoKeywords) -> Vec<(SyntaxKind, usize, usize)> {
    let mut out = Vec::new();
    let mut push = |kind, a: usize, b: usize| {
        if a < b {
            out.push((kind, a, b));
        }
    };
    let stars = s.bytes().take_while(|&b| b == b'*').count();
    push(STARS, 0, stars);
    let mut i = stars + ws_len(&s[stars..]);
    push(WHITESPACE, stars, i);
    if let Some(kw) = todo.match_at(&s[i..]) {
        push(TODO_KEYWORD, i, i + kw.len());
        let j = i + kw.len();
        i = j + ws_len(&s[j..]);
        push(WHITESPACE, j, i);
    }
    if let Some(n) = priority_len(&s[i..]) {
        push(PRIORITY, i, i + n);
        let j = i + n;
        i = j + ws_len(&s[j..]);
        push(WHITESPACE, j, i);
    }
    let rest = &s[i..];
    if rest.starts_with("COMMENT") && (rest.len() == 7 || rest.as_bytes()[7] == b' ') {
        push(COMMENT_MARKER, i, i + 7);
        let j = i + 7;
        i = j + ws_len(&s[j..]);
        push(WHITESPACE, j, i);
    }
    let trimmed = trim_end_blank(s).len().max(i);
    let tags = tags_start(s, i).unwrap_or(trimmed);
    // raw-value is `org-trim`med: a content CR is not part of the title.
    let title = &s[i..tags];
    let title_start = i + (title.len() - title.trim_start_matches(is_trim).len());
    let title_end = (i + title.trim_end_matches(is_trim).len()).max(title_start);
    push(WHITESPACE, i, title_start);
    push(TITLE, title_start, title_end);
    push(WHITESPACE, title_end, tags);
    push(TAGS, tags, trimmed);
    push(WHITESPACE, trimmed, s.len());
    out
}

/// `\[#.\]`: the cookie's byte length.
fn priority_len(s: &str) -> Option<usize> {
    let r = s.strip_prefix("[#")?;
    let c = r.chars().next()?;
    (c != '\n' && r[c.len_utf8()..].starts_with(']')).then(|| 3 + c.len_utf8())
}

/// Where `(:[[:alnum:]_@#%:]+:)[ \t]*$`, searched forward from `from`, first
/// matches: the leftmost colon of the trailing run of tag characters that
/// still leaves at least one character before the final colon.
fn tags_start(s: &str, from: usize) -> Option<usize> {
    let t = trim_end_blank(s);
    if !t.ends_with(':') || t.len() < from + 3 {
        return None;
    }
    let is_tag = |c: char| crate::chars::is_alnum(c) || matches!(c, '_' | '@' | '#' | '%' | ':');
    let run = t
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_tag(c))
        .last()
        .map_or(t.len(), |(k, _)| k);
    let lo = run.max(from);
    t[lo..]
        .find(':')
        .map(|k| lo + k)
        .filter(|&p| p + 2 < t.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{TodoKeywords, TodoSequence};

    fn kinds(s: &str) -> Vec<(SyntaxKind, String)> {
        let todo = TodoKeywords::resolve(
            true,
            &[TodoSequence {
                todo: vec!["TODO".into()],
                done: vec!["DONE".into()],
            }],
            &[],
        );
        tokens(s, &todo)
            .into_iter()
            .map(|(k, a, b)| (k, s[a..b].to_string()))
            .collect()
    }

    #[test]
    fn every_part_of_a_full_headline() {
        let got = kinds("** TODO [#A] COMMENT Title text  :a:b@c:  ");
        let want = [
            (STARS, "**"),
            (WHITESPACE, " "),
            (TODO_KEYWORD, "TODO"),
            (WHITESPACE, " "),
            (PRIORITY, "[#A]"),
            (WHITESPACE, " "),
            (COMMENT_MARKER, "COMMENT"),
            (WHITESPACE, " "),
            (TITLE, "Title text"),
            (WHITESPACE, "  "),
            (TAGS, ":a:b@c:"),
            (WHITESPACE, "  "),
        ];
        assert_eq!(
            got,
            want.iter()
                .map(|(k, s)| (*k, s.to_string()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn tags_are_the_leftmost_trailing_match() {
        assert_eq!(tags_start("* a :x: b :y:", 2), Some(10));
        assert_eq!(tags_start("* a :x:y:", 2), Some(4));
        assert_eq!(tags_start("* a::", 2), None);
        assert_eq!(tags_start("* :t:", 2), Some(2));
        assert_eq!(tags_start("* a :x: ", 2), Some(4));
    }

    #[test]
    fn todo_and_comment_need_a_following_space() {
        let got = kinds("* TODOx COMMENTS");
        assert_eq!(got[2], (TITLE, "TODOx COMMENTS".to_string()));
        let got = kinds("* DONE");
        assert_eq!(got[2], (TODO_KEYWORD, "DONE".to_string()));
    }
}
