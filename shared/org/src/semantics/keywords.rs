//! Which keywords the semantics read, and where: each one as the Org
//! function that gives it meaning reads it (verified against Emacs 30.2 /
//! Org 9.7.11, `tests/semantics_regions.rs`).
//!
//! - `#+SELECT_TAGS`, `#+EXCLUDE_TAGS` and `#+TAGS` (group tags): the export
//!   deletes COMMENT subtrees before it reads its options
//!   (`org-export--annotate-info`: `org-export--delete-comment-trees`, then
//!   `org-set-regexps-and-options` and `org-export-get-environment`), so one
//!   inside a COMMENT subtree does not count. Nor does a `#+` line inside a
//!   block, which is no keyword at all.
//! - `#+LINK`: deleted the same way, but the export's buffer copy inherits
//!   the original buffer's abbreviations, which `org-set-regexps-and-options`
//!   replaces only when it finds a `#+LINK` keyword. So the abbreviations
//!   are those outside COMMENT subtrees when there is one, and otherwise
//!   every `#+LINK` keyword, COMMENT subtrees included (as `org-mode` read
//!   them).
//! - `#+TITLE`: `org-get-title` (see [`title`]): every line that starts
//!   `#+TITLE:`, in any context (a COMMENT subtree, a block), the values
//!   joined with a space. This is Org's rule, not MAE's.
//! - `#+SETUPFILE`: `org-mode` reads it wherever it is, and it can declare
//!   TODO keywords that change the parse itself: diagnosed wherever it is a
//!   keyword (see `semantics`).
//! - `#+FILETAGS` and MAE's own keywords: the zeroth section only (the
//!   `semantics` module doc), which a COMMENT subtree can never hold.

use crate::chars::{is_alnum, is_space};
use crate::kind::SyntaxNode;
use crate::link_grammar::org_trim;
use crate::text::LineEnding;
use crate::views::{Headline, Keyword, LinkAbbrevs};
use crate::Document;
use rowan::WalkEvent;
use std::collections::{HashMap, HashSet, VecDeque};

/// Every keyword element outside COMMENT subtrees, in document order.
pub(crate) fn outside_comment_trees(root: &SyntaxNode) -> Vec<Keyword> {
    uncommented(root).filter_map(Keyword::cast).collect()
}

/// Every node of `root`, in document order, outside the COMMENT subtrees:
/// the buffer as Org's export reads it to collect settings and templates
/// and to run babel, after `org-export--delete-comment-trees`.
pub(crate) fn uncommented(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    let mut walk = root.preorder();
    std::iter::from_fn(move || loop {
        match walk.next()? {
            WalkEvent::Enter(n) if Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) => {
                walk.skip_subtree()
            }
            WalkEvent::Enter(n) => return Some(n),
            WalkEvent::Leave(_) => {}
        }
    })
}

/// The tags the `key` keywords among `keywords` declare (words split on
/// Org's blanks), or `configured` if there is none.
pub(crate) fn declared_tags(keywords: &[Keyword], key: &str, configured: &[String]) -> Vec<String> {
    let declared: Vec<&Keyword> = keywords
        .iter()
        .filter(|k| k.key().eq_ignore_ascii_case(key))
        .collect();
    if declared.is_empty() {
        return configured.to_vec();
    }
    declared
        .iter()
        .flat_map(|k| split_words(&k.value()))
        .collect()
}

/// `split-string` with its default separators: runs of ASCII space, form
/// feed, tab, LF, CR and vertical tab. A no-break space is not one.
pub(crate) fn split_words(value: &str) -> Vec<String> {
    value
        .split([' ', '\x0c', '\t', '\n', '\r', '\x0b'])
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The `#+LINK:` abbreviations the export applies (see the module doc).
pub(crate) fn export_link_abbrevs(root: &SyntaxNode, outside: &[Keyword]) -> LinkAbbrevs {
    let is_link = |k: &Keyword| k.key().eq_ignore_ascii_case("LINK");
    if outside.iter().any(is_link) {
        LinkAbbrevs::of_keywords(outside.iter().cloned())
    } else {
        LinkAbbrevs::of(root)
    }
}

/// One token of `org-tag-string-to-alist`'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    StartGroup,
    EndGroup,
    GroupTags,
    Tag(String),
}

/// The group tags `#+TAGS:` keywords declare (`org-tag-groups-alist`).
/// Declared configuration (ADR-112 D4), read from the text alone.
#[derive(Debug, Default)]
pub(crate) struct TagGroups {
    /// Each group tag and its members, the first group of a name winning
    /// (`assoc`).
    groups: HashMap<String, Vec<String>>,
}

impl TagGroups {
    /// The groups of the `#+TAGS:` keywords among `keywords`.
    pub(crate) fn new(keywords: &[Keyword]) -> TagGroups {
        let values: Vec<String> = keywords
            .iter()
            .filter(|k| k.key().eq_ignore_ascii_case("TAGS"))
            .map(|k| k.value())
            .collect();
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for (tag, members) in alist_to_groups(&string_to_alist(&values.join("\n"))) {
            groups.entry(tag).or_insert(members);
        }
        TagGroups { groups }
    }

    /// `(cl-mapcan (lambda (tag) (org-tags-expand tag t)) tags)` as a set:
    /// each tag, and the members of every group reached from it.
    /// Iterative, and each tag is expanded once, however the groups chain.
    /// A `{regexp}` member is a tag like any other: Emacs's export compares
    /// it with `member`, so it matches only a tag spelled `{regexp}` (none
    /// can be: braces are not tag chars), and no regexp is ever run.
    pub(crate) fn expand(&self, tags: &[String]) -> Vec<String> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out = Vec::new();
        let mut queue: VecDeque<&str> = tags.iter().map(String::as_str).collect();
        while let Some(t) = queue.pop_front() {
            if !seen.insert(t) {
                continue;
            }
            out.push(t.to_string());
            if let Some(members) = self.groups.get(t) {
                queue.extend(members.iter().map(String::as_str));
            }
        }
        out
    }
}

/// `org-tag-string-to-alist` (Org 9.7.11), keeping what groups need: each
/// line's tokens split on blanks; `{`/`[` open a group, `}`/`]` close it,
/// `:` marks the group tag's members; a tag (`org-tag-re`, or `{…}`,
/// optionally followed by a `(k)` key) outside a group with `:` is dropped
/// when already listed.
fn string_to_alist(s: &str) -> Vec<Token> {
    let mut alist: Vec<Token> = Vec::new();
    let mut listed: HashSet<String> = HashSet::new();
    let mut group_flag = false;
    for line in s.split('\n').filter(|l| !l.is_empty()) {
        let tokens = split_words(line);
        for (i, token) in tokens.iter().enumerate() {
            // `(nth 1 tokens)` once `token` is popped: two on from it.
            let colon_next = tokens.get(i + 2).is_some_and(|t| t == ":");
            match token.as_str() {
                "{" | "[" => {
                    alist.push(Token::StartGroup);
                    group_flag |= colon_next;
                }
                "}" | "]" => {
                    alist.push(Token::EndGroup);
                    group_flag = false;
                }
                ":" => alist.push(Token::GroupTags),
                t => {
                    // Outside a group with `:`, a tag already listed is dropped.
                    let tag = tag_of_token(t).filter(|tag| group_flag || !listed.contains(*tag));
                    if let Some(tag) = tag {
                        listed.insert(tag.to_string());
                        alist.push(Token::Tag(tag.to_string()));
                    }
                }
            }
        }
    }
    alist
}

/// `\`\(org-tag-re\|{.+?}\)\(?:(\(.\))\)?\'`: the tag of a token, if it
/// is one.
fn tag_of_token(t: &str) -> Option<&str> {
    let key_or_end = |rest: &str| {
        let mut cs = rest.chars();
        rest.is_empty() || (cs.next() == Some('(') && cs.next().is_some() && cs.as_str() == ")")
    };
    let tag_char = |c: char| is_alnum(c) || matches!(c, '_' | '@' | '#' | '%');
    let n = t.find(|c: char| !tag_char(c)).unwrap_or(t.len());
    if n > 0 && key_or_end(&t[n..]) {
        return Some(&t[..n]);
    }
    if t.starts_with('{') {
        // The shortest `{.+?}` the rest of the token allows.
        return t
            .match_indices('}')
            .map(|(j, _)| j)
            .filter(|&j| j >= 2)
            .find(|&j| key_or_end(&t[j + 1..]))
            .map(|j| &t[..=j]);
    }
    None
}

/// `org-tag-alist-to-groups`: a group is the tag after an opening brace,
/// then the tags after its `:` up to the closing brace. An opening brace
/// inside a group restarts it, so that group is lost (as in Org).
fn alist_to_groups(alist: &[Token]) -> Vec<(String, Vec<String>)> {
    #[derive(PartialEq)]
    enum Status {
        Out,
        Open,
        Append,
    }
    let mut groups = Vec::new();
    let mut status = Status::Out;
    let mut current: Vec<String> = Vec::new();
    for token in alist {
        match token {
            Token::StartGroup => status = Status::Open,
            Token::EndGroup => {
                if status == Status::Append && !current.is_empty() {
                    let tag = current.remove(0);
                    groups.push((tag, std::mem::take(&mut current)));
                }
                status = Status::Out;
                current.clear();
            }
            Token::GroupTags => status = Status::Append,
            Token::Tag(t) => match status {
                Status::Append => current.push(t.clone()),
                Status::Open => current = vec![t.clone()],
                Status::Out => {}
            },
        }
    }
    groups
}

/// The file's title as `org-get-title` gives it (Emacs 30.2): every line
/// of the buffer that starts with `#+TITLE:` (blanks before it, any case),
/// in ANY context, read as `org-element-keyword-parser` reads a keyword
/// line (the key runs to the last colon of its first word); the values
/// joined with one space each, then `org-trim`med. `None` when there is
/// no such line or the result is empty: so `#+title: a` then `#+title:`
/// is `a`, and an empty value does not clear the title.
pub(crate) fn title(doc: &Document) -> Option<String> {
    let text = crate::text::source_of(&doc.syntax());
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let lines = buffer_lines(text, doc.line_ending());
    keyword_value(lines.into_iter(), "title", true)
        .map(|t| org_trim(&t).to_string())
        .filter(|t| !t.is_empty())
}

/// `org-macro--find-keyword-value`: the value of the first `#+NAME:` line
/// of `lines` (any case, as Emacs folds it), or with `collect` every one's,
/// each after a space, the whole trimmed; `None` when no line matches. A
/// line matches Org's regexp `^[ \t]*#\+NAME:` and then is read by its
/// keyword parser: the key runs to the LAST colon of the word after `#+`
/// (`#+title:x: y` has the value `y`), the value is the rest, trimmed.
pub(crate) fn keyword_value<'a>(
    lines: impl Iterator<Item = &'a str>,
    name: &str,
    collect: bool,
) -> Option<String> {
    let mut joined: Option<String> = None;
    for line in lines {
        let l = line.trim_start_matches([' ', '\t']);
        let Some(rest) = l.strip_prefix("#+") else {
            continue;
        };
        let mut chars = rest.chars();
        let named = name
            .chars()
            .all(|c| chars.next().map(crate::chars::canon) == Some(crate::chars::canon(c)));
        if !named || chars.next() != Some(':') {
            continue;
        }
        let word = rest.find(is_space).unwrap_or(rest.len());
        let colon = rest[..word].rfind(':').unwrap_or(name.len());
        let value = org_trim(&rest[colon + 1..]);
        if !collect {
            return Some(value.to_string());
        }
        let j = joined.get_or_insert_with(String::new);
        j.push(' ');
        j.push_str(value);
    }
    joined.map(|j| org_trim(&j).to_string())
}

/// The lines of `text` as Emacs's buffer holds them under `eol`.
pub(crate) fn buffer_lines(text: &str, eol: LineEnding) -> Vec<&str> {
    match eol {
        LineEnding::Lf => text.split('\n').collect(),
        LineEnding::CrLf => text.split("\r\n").collect(),
        LineEnding::Cr => text.split('\r').collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(tags_lines: &[&str]) -> Vec<(String, Vec<String>)> {
        alist_to_groups(&string_to_alist(&tags_lines.join("\n")))
    }

    /// Transliterations of `org-tag-string-to-alist` and
    /// `org-tag-alist-to-groups`, on inputs Emacs was asked about
    /// (`tests/semantics_regions.rs` checks the export they drive).
    #[test]
    fn tag_groups_parse_like_org() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            groups(&["[ noexport : private ]"]),
            [("noexport".to_string(), s(&["private"]))]
        );
        assert_eq!(
            groups(&["{ G : x y }", "{ P : {P@.+} }"]),
            [
                ("G".to_string(), s(&["x", "y"])),
                ("P".to_string(), s(&["{P@.+}"]))
            ]
        );
        // A brace inside a group restarts it: G is lost, [ y : w ] kept.
        assert_eq!(
            groups(&["{ G : x { y z } } [ y : w ]"]),
            [("y".to_string(), s(&["w"]))]
        );
        // No `:` makes no group; keys `(k)` are dropped from tags.
        assert_eq!(
            groups(&["{ a b }", "{ W(w) : L(l) C }"]),
            [("W".to_string(), s(&["L", "C"]))]
        );
        assert_eq!(tag_of_token("work(w)"), Some("work"));
        assert_eq!(tag_of_token("{a}(b)"), Some("{a}"));
        assert_eq!(tag_of_token("{}"), None);
        assert_eq!(tag_of_token("a(bc)"), None);
        assert_eq!(tag_of_token("é_1@#%"), Some("é_1@#%"));
    }

    #[test]
    fn group_expansion_follows_chains_once() {
        let mut g = TagGroups::default();
        g.groups.insert("a".into(), vec!["b".into(), "c".into()]);
        g.groups.insert("b".into(), vec!["a".into(), "d".into()]);
        let mut got = g.expand(&["a".to_string()]);
        got.sort();
        assert_eq!(got, ["a", "b", "c", "d"]);
        // Case-sensitive, as `assoc` and `member` are.
        assert_eq!(g.expand(&["A".to_string()]), ["A"]);
    }
}
