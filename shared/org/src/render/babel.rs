//! Babel's export decision, read and never run: which src blocks, inline
//! src blocks and existing results Org's export removes, from each block's
//! `:exports` header argument (`ob-exp`).
//!
//! `:exports` is resolved as `org-babel-get-src-block-info` resolves it,
//! from in-file declarations only (ADR-112 D4), merged in this order, a
//! later value replacing an earlier one (`org-babel-merge-params`):
//!
//! 1. Org's defaults: `code` for a block, `results` for an inline block
//!    (`org-babel-default-header-args`, `-inline-header-args`);
//! 2. the `header-args` property, then `header-args:LANG`, each inherited
//!    as `org-entry-get` inherits it: the nearest headline (then the file's
//!    property drawer) with a plain `:header-args:` ends the search, the
//!    `:header-args+:` values met on the way append to it, and with no plain
//!    value anywhere `#+PROPERTY: header-args …` (itself `+`-appended)
//!    starts the list;
//! 3. the block's own header arguments, after its language and switches;
//! 4. each `#+HEADER:`/`#+HEADERS:` line, in order.
//!
//! Then, as `org-babel-exp-do-export` acts on the value:
//! - `none`: the block and its results are removed;
//! - `code`: the results are removed;
//! - `results`: the block is removed; existing results stay (nothing is
//!   ever evaluated, so they are the results already in the text);
//! - `both`: nothing is removed.
//!
//! A block's existing results are the element after it carrying an
//! anonymous `#+RESULTS:` and, for a block with a `#+NAME:`, the first
//! element carrying `#+RESULTS: NAME`; an inline block's are the
//! `{{{results(…)}}}` macro after it. "Removed" includes everything
//! attached to the element: its affiliated keywords (a `#+CAPTION:` and the
//! targets and footnote references in it).
//!
//! Deliberate differences, each erring towards hiding (listed in
//! `conformance/render-divergences.toml`): header-argument names and
//! `:exports` values are matched case-insensitively (`:exports NONE` hides;
//! Org warns and exports the block as written); a word other than the four
//! values is ignored rather than making the whole value unknown (`none foo`
//! hides; Org exports as written); a value Org would evaluate (Lisp, or a
//! number, on which Org's export fails) hides the code and the results; a
//! named block's adjacent anonymous results are removed too; and blocks
//! under an archived headline follow their `:exports` (Org's babel pass
//! skips them, so with `org-export-with-archived-trees` set to `t` it
//! exports their code as written).

use crate::kind::SyntaxKind::{self, *};
use crate::kind::{SyntaxElement, SyntaxNode};
use crate::views::{canonical_key, Headline, PropertyDrawer};
use crate::{Document, Span};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// What `:exports` resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exports {
    Code,
    Results,
    Both,
    None,
    /// A value only evaluation could tell: hide everything.
    Opaque,
}

impl Exports {
    pub fn shows_code(self) -> bool {
        matches!(self, Exports::Code | Exports::Both)
    }

    pub fn keeps_results(self) -> bool {
        matches!(self, Exports::Results | Exports::Both)
    }
}

/// The export decisions of one document.
#[derive(Debug)]
pub(crate) struct Babel {
    root: SyntaxNode,
    /// Elements Org's export removes: `(start, end, kind)`.
    hidden: HashSet<(u32, u32, SyntaxKind)>,
    /// `#+PROPERTY:` values, canonical key → value, `+` keys merged.
    global: HashMap<String, String>,
    file_drawer: Option<PropertyDrawer>,
    /// (headline start, canonical property) → its inherited value.
    memo: RefCell<HashMap<(u32, String), Option<String>>>,
}

fn key(n: &SyntaxNode) -> (u32, u32, SyntaxKind) {
    let s = Span::of(n);
    (s.start, s.end, n.kind())
}

/// The text of `n`'s first token of kind `kind`.
fn token(n: &SyntaxNode, kind: SyntaxKind) -> Option<String> {
    n.children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|t| t.kind() == kind)
        .map(|t| t.text().to_string())
}

/// An affiliated keyword's key, aliases translated as `org-element` does.
fn affiliated_key(a: &SyntaxNode) -> String {
    let k = token(a, KEY).unwrap_or_default().to_ascii_uppercase();
    match k.as_str() {
        "DATA" | "LABEL" | "RESNAME" | "SOURCE" | "SRCNAME" | "TBLNAME" => "NAME".into(),
        "HEADERS" => "HEADER".into(),
        _ => k,
    }
}

/// `(key, value)` of each affiliated keyword of `n`, in order. A RESULTS
/// keyword is babel's only when written `RESULTS` (its regexps do not know
/// the `RESULT` alias).
fn affiliated(n: &SyntaxNode) -> impl Iterator<Item = (String, String)> {
    n.children()
        .filter(|c| c.kind() == AFFILIATED_KEYWORD)
        .map(|a| {
            let value = token(&a, VALUE).unwrap_or_default();
            (affiliated_key(&a), value.trim().to_string())
        })
}

impl Babel {
    pub fn new(doc: &Document) -> Babel {
        let root = doc.syntax();
        let mut babel = Babel {
            root: root.clone(),
            hidden: HashSet::new(),
            global: global_properties(doc),
            file_drawer: doc.property_drawer(),
            memo: RefCell::new(HashMap::new()),
        };
        let mut named: HashMap<String, SyntaxNode> = HashMap::new();
        let mut blocks = Vec::new();
        for n in root.descendants() {
            for (k, v) in affiliated(&n) {
                if k == "RESULTS" && !v.is_empty() {
                    named.entry(crate::chars::fold(&v)).or_insert(n.clone());
                }
            }
            if n.kind() == SRC_BLOCK {
                blocks.push(n);
            }
        }
        for b in blocks {
            let e = babel.block_exports(&b);
            if !e.shows_code() {
                babel.hidden.insert(key(&b));
            }
            if !e.keeps_results() {
                for r in results_of(&b, &named) {
                    babel.hidden.insert(key(&r));
                }
            }
        }
        babel
    }

    /// Whether Org's export removes element `n`.
    pub fn hides(&self, n: &SyntaxNode) -> bool {
        self.hidden.contains(&key(n))
    }

    /// `:exports` of a src block.
    fn block_exports(&self, n: &SyntaxNode) -> Exports {
        let line = n
            .children()
            .find(|c| c.kind() == BLOCK_BEGIN)
            .map(|l| l.to_string())
            .unwrap_or_default();
        let (lang, params) = src_line(&line);
        let headers: Vec<String> = affiliated(n)
            .filter(|(k, _)| k == "HEADER")
            .map(|(_, v)| v)
            .collect();
        let headline = n.ancestors().find(|a| a.kind() == HEADLINE);
        self.resolve(Exports::Code, headline, &lang, &params, &headers)
    }

    /// `:exports` of an inline src block at `at` in the document (`None`:
    /// text with no place in it, read at the top of the file).
    pub fn inline_exports(&self, n: &SyntaxNode, at: Option<u32>) -> Exports {
        let lang = token(n, NAME).unwrap_or_default();
        let params = token(n, PARAMETERS).unwrap_or_default();
        let headline = at.and_then(|p| self.headline_at(p));
        self.resolve(Exports::Results, headline, &lang, &params, &[])
    }

    fn headline_at(&self, at: u32) -> Option<SyntaxNode> {
        if at >= u32::from(self.root.text_range().end()) {
            return None;
        }
        let range = rowan::TextRange::empty(at.into());
        let start = match self.root.covering_element(range) {
            rowan::NodeOrToken::Node(n) => n,
            rowan::NodeOrToken::Token(t) => t.parent()?,
        };
        start.ancestors().find(|a| a.kind() == HEADLINE)
    }

    fn resolve(
        &self,
        default: Exports,
        headline: Option<SyntaxNode>,
        lang: &str,
        params: &str,
        headers: &[String],
    ) -> Exports {
        let mut sources = Vec::new();
        sources.extend(self.inherited("HEADER-ARGS", headline.as_ref()));
        if !lang.is_empty() {
            let prop = canonical_key(&format!("header-args:{lang}"));
            sources.extend(self.inherited(&prop, headline.as_ref()));
        }
        sources.push(params.to_string());
        sources.extend(headers.iter().cloned());
        let mut out = default;
        for s in &sources {
            for (k, v) in parse_header_arguments(s) {
                if k.eq_ignore_ascii_case(":exports") {
                    merge_exports(&mut out, v.as_deref());
                }
            }
        }
        out
    }

    /// `prop` (canonical) as `org-entry-get` with inheritance reads it at a
    /// point under `headline` (`None`: the zeroth section).
    fn inherited(&self, prop: &str, headline: Option<&SyntaxNode>) -> Option<String> {
        // The headlines up to the first one whose value is known (memoised,
        // or a plain value of its own), innermost first.
        let mut path: Vec<(SyntaxNode, Vec<String>)> = Vec::new();
        let mut cur = headline.cloned();
        let mut known = loop {
            let Some(h) = cur else {
                break self.file_level(prop);
            };
            let at = (Span::of(&h).start, prop.to_string());
            if let Some(v) = self.memo.borrow().get(&at) {
                break v.clone();
            }
            let drawer = Headline::cast(h.clone()).and_then(|h| h.property_drawer());
            let (base, plus) = local_values(drawer, prop);
            if let Some(b) = base {
                let v = join(Some(b), &plus);
                self.memo.borrow_mut().insert(at, v.clone());
                break v;
            }
            cur = h.ancestors().skip(1).find(|a| a.kind() == HEADLINE);
            path.push((h, plus));
        };
        for (h, plus) in path.into_iter().rev() {
            known = join(known, &plus);
            let at = (Span::of(&h).start, prop.to_string());
            self.memo.borrow_mut().insert(at, known.clone());
        }
        known.filter(|v| v != "nil")
    }

    /// The file level: its property drawer, else `#+PROPERTY:`.
    fn file_level(&self, prop: &str) -> Option<String> {
        let (base, plus) = local_values(self.file_drawer.clone(), prop);
        match base {
            Some(b) => join(Some(b), &plus),
            None => join(self.global.get(prop).cloned(), &plus),
        }
    }
}

/// `base` then `plus`, joined by spaces (`org-property-separators`'s
/// default); `None` if there is nothing.
fn join(base: Option<String>, plus: &[String]) -> Option<String> {
    let mut parts: Vec<String> = base.into_iter().collect();
    parts.extend(plus.iter().cloned());
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// A drawer's plain value of `prop` (the last one wins) and its `prop+`
/// values in order (`org-element--get-node-properties`).
fn local_values(drawer: Option<PropertyDrawer>, prop: &str) -> (Option<String>, Vec<String>) {
    let Some(d) = drawer else {
        return (None, Vec::new());
    };
    let plus_key = format!("{prop}+");
    let mut base = None;
    let mut plus = Vec::new();
    for p in d.properties() {
        if p.key == prop {
            base = Some(p.value);
        } else if p.key == plus_key {
            plus.push(p.value);
        }
    }
    (base, plus)
}

/// `#+PROPERTY: NAME VALUE` lines in order, a `NAME+` appending to `NAME`
/// (`org--update-property-plist`).
fn global_properties(doc: &Document) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for k in doc.keywords() {
        if !k.key().eq_ignore_ascii_case("PROPERTY") {
            continue;
        }
        let value = k.value();
        let Some((name, rest)) = value.split_once([' ', '\t']) else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let (name, append) = match name.strip_suffix('+') {
            Some(n) if !n.is_empty() => (n, true),
            _ => (name, false),
        };
        let name = canonical_key(name);
        match out.get_mut(&name) {
            Some(old) if append => {
                old.push(' ');
                old.push_str(rest);
            }
            _ => {
                out.insert(name, rest.to_string());
            }
        }
    }
    out
}

/// The results elements of src block `n`: the next element when it carries
/// an anonymous `#+RESULTS:`, and the named results of a named block.
fn results_of(n: &SyntaxNode, named: &HashMap<String, SyntaxNode>) -> Vec<SyntaxNode> {
    let mut out = Vec::new();
    if let Some(next) = n.next_sibling() {
        if affiliated(&next).any(|(k, v)| k == "RESULTS" && v.is_empty()) {
            out.push(next);
        }
    }
    let name = affiliated(n)
        .filter(|(k, _)| k == "NAME")
        .map(|(_, v)| v)
        .last()
        .filter(|v| !v.is_empty());
    if let Some(r) = name.and_then(|v| named.get(&crate::chars::fold(&v))) {
        if !out.contains(r) {
            out.push(r.clone());
        }
    }
    out
}

/// Whether the inline src block's results macro `n` (a `MACRO`) is removed:
/// it follows an inline src block (only blanks between) whose `:exports`
/// does not keep results.
pub(crate) fn results_macro_owner(n: &SyntaxNode) -> Option<SyntaxNode> {
    let name = token(n, NAME)?;
    if !name.eq_ignore_ascii_case("results") {
        return None;
    }
    let mut prev = n.prev_sibling_or_token();
    while let Some(p) = prev {
        match p {
            rowan::NodeOrToken::Token(t)
                if t.text()
                    .chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) =>
            {
                prev = t.prev_sibling_or_token();
            }
            rowan::NodeOrToken::Node(s) if s.kind() == INLINE_SRC_BLOCK => return Some(s),
            _ => return None,
        }
    }
    None
}

/// A src block's language and header arguments from its opening line, as
/// `org-element-src-block-parser` splits it: `#+BEGIN_SRC`, then
/// ` +LANG`, then switches (` +-l "…"`, `-i`, `-k`, `-r`, `[-+]n N`),
/// then the parameters.
pub(crate) fn src_line(line: &str) -> (String, String) {
    let line = line.trim_end_matches(['\n', '\r']);
    let lower = line.to_ascii_lowercase();
    let Some(at) = lower.find("begin_src") else {
        return (String::new(), String::new());
    };
    let rest = &line[at + "begin_src".len()..];
    let b = rest.as_bytes();
    let mut pos = 0;
    let spaces = |from: usize| from + b[from..].iter().take_while(|c| **c == b' ').count();
    let blank = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c');
    let mut lang = String::new();
    let s = spaces(0);
    if s > 0 {
        let e = s + b[s..].iter().take_while(|c| !blank(**c)).count();
        if e > s {
            lang = rest[s..e].to_string();
            pos = e;
        }
    }
    loop {
        let s = spaces(pos);
        if s == pos {
            break;
        }
        let r = &rest[s..];
        let next = if let Some(q) = r.strip_prefix("-l \"") {
            // `".+"`, greedy: up to the line's last quote with at least one
            // character before it.
            q.rfind('"').filter(|i| *i > 0).map(|i| s + 4 + i + 1)
        } else if r.starts_with("-i") || r.starts_with("-k") || r.starts_with("-r") {
            Some(s + 2)
        } else if r.starts_with("-n") || r.starts_with("+n") {
            let after = s + 2;
            let sp = after + b[after..].iter().take_while(|c| **c == b' ').count();
            let digits = b[sp..].iter().take_while(|c| c.is_ascii_digit()).count();
            Some(if digits > 0 { sp + digits } else { after })
        } else {
            None
        };
        match next {
            Some(n) => pos = n,
            None => break,
        }
    }
    (lang, rest[pos..].trim().to_string())
}

/// `org-babel-parse-header-arguments`: `:KEY VALUE` pairs, split at a `:`
/// after a blank outside balanced brackets and double quotes.
pub(crate) fn parse_header_arguments(s: &str) -> Vec<(String, Option<String>)> {
    if s.trim().is_empty() {
        return Vec::new();
    }
    let raw = balanced_split(s);
    raw.iter()
        .enumerate()
        .map(|(i, r)| {
            let arg = if i == 0 { r.clone() } else { format!(":{r}") };
            let blank = |c: char| matches!(c, ' ' | '\x0c' | '\t' | '\n' | '\r' | '\x0b');
            let line = arg.split('\n').next().unwrap_or("");
            let start = line.find(|c: char| !blank(c));
            let pair = start.and_then(|k0| {
                let rest = &line[k0..];
                let kend = rest.find(blank)?;
                let after = &rest[kend..];
                let v0 = after.find(|c: char| !blank(c))?;
                Some((rest[..kend].to_string(), after[v0..].to_string()))
            });
            match pair {
                Some((k, v)) => (k, Some(v.trim_end_matches(blank).to_string())),
                None => (arg.trim_end_matches(blank).to_string(), None),
            }
        })
        .collect()
}

/// `org-babel-balanced-split` with `((32 9) . 58)`.
fn balanced_split(s: &str) -> Vec<String> {
    let c: Vec<char> = s.chars().collect();
    let mut result = Vec::new();
    let mut partial: Vec<char> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let past = i.checked_sub(1).map(|p| c[p]);
        if matches!(past, Some(' ' | '\t')) && c[i] == ':' {
            partial.pop();
            if !partial.is_empty() {
                result.push(partial.iter().collect());
                partial.clear();
            }
            i += 1;
        } else if c[i] == '(' || c[i] == '[' {
            let mut open = vec![c[i]];
            let mut j = i + 1;
            while !open.is_empty() && j < c.len() {
                match c[j] {
                    '(' => open.push('('),
                    ']' if open.last() == Some(&'[') => {
                        open.pop();
                    }
                    ')' if open.last() == Some(&'(') => {
                        open.pop();
                    }
                    _ => {}
                }
                j += 1;
            }
            if open.is_empty() {
                partial.extend(&c[i..j]);
                i = j;
            } else {
                partial.push(c[i]);
                i += 1;
            }
        } else if c[i] == '"' && past != Some('\\') {
            let close = (i + 1..c.len()).find(|&k| c[k] == '"' && c[k - 1] != '\\');
            match close {
                Some(k) => {
                    partial.extend(&c[i..=k]);
                    i = k + 1;
                }
                None => {
                    partial.push('"');
                    i += 1;
                }
            }
        } else {
            partial.push(c[i]);
            i += 1;
        }
    }
    if !partial.is_empty() {
        result.push(partial.iter().collect());
    }
    result
}

/// Merges one `:exports` value into `out`: each of `code`, `results`,
/// `both`, `none` replaces the previous one (`org-babel-merge-params`'s
/// exclusive group), any other word is ignored, and a value Org would
/// evaluate is [`Exports::Opaque`].
fn merge_exports(out: &mut Exports, value: Option<&str>) {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    if is_evaluated(v) {
        *out = Exports::Opaque;
        return;
    }
    let v = unquote(v);
    for w in v.split_whitespace() {
        *out = match w.to_ascii_lowercase().as_str() {
            "code" => Exports::Code,
            "results" => Exports::Results,
            "both" => Exports::Both,
            "none" => Exports::None,
            _ => continue,
        };
    }
}

/// `org-babel-read` would not return `v` as a string: Lisp (`(`, `'`, `` ` ``,
/// `[`, `*this*`), which it evaluates, or a number, on which the merge fails.
fn is_evaluated(v: &str) -> bool {
    let lisp = v.starts_with(['(', '\'', '`', '[']) || v == "*this*";
    let number = {
        let d = v.strip_prefix('-').unwrap_or(v);
        let (a, b) = d.split_once('.').unwrap_or((d, ""));
        a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
    };
    lisp || number
}

/// A value that is one Lisp string literal, read; otherwise `v`.
fn unquote(v: &str) -> String {
    let Some(inner) = v.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return v.to_string();
    };
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '"' => return v.to_string(),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opening_line_splits_as_org_element_splits_it() {
        let cases = [
            ("#+begin_src sh -n :exports code\n", ("sh", ":exports code")),
            (
                "#+BEGIN_SRC sh -n -r :exports none",
                ("sh", ":exports none"),
            ),
            (
                "#+begin_src sh -l \"(ref:%s)\" :exports none",
                ("sh", ":exports none"),
            ),
            ("#+begin_src sh +n 10 -k :x y", ("sh", ":x y")),
            ("#+begin_src\tsh :exports none", ("", "sh :exports none")),
            ("#+begin_src", ("", "")),
            ("#+begin_src sh -index :e", ("sh", "ndex :e")),
        ];
        for (line, (lang, params)) in cases {
            assert_eq!(src_line(line), (lang.into(), params.into()), "{line}");
        }
    }

    #[test]
    fn header_arguments_split_outside_quotes_and_brackets() {
        let p = parse_header_arguments(":var x=\":exports none\" :exports code");
        assert_eq!(
            p,
            vec![
                (":var".into(), Some("x=\":exports none\"".into())),
                (":exports".into(), Some("code".into())),
            ]
        );
        let p = parse_header_arguments(":var x=(a :exports b) :results");
        assert_eq!(p[0].1.as_deref(), Some("x=(a :exports b)"));
        assert_eq!(p[1], (":results".into(), None));
    }

    #[test]
    fn exports_values_merge_as_an_exclusive_group() {
        let merged = |vs: &[&str]| {
            let mut e = Exports::Code;
            vs.iter().for_each(|v| merge_exports(&mut e, Some(v)));
            e
        };
        assert_eq!(merged(&["none"]), Exports::None);
        assert_eq!(merged(&["none", "code"]), Exports::Code);
        assert_eq!(merged(&["code none"]), Exports::None);
        assert_eq!(merged(&["\"none\""]), Exports::None);
        assert_eq!(merged(&["NONE"]), Exports::None);
        assert_eq!(merged(&["none foo"]), Exports::None);
        assert_eq!(merged(&["(if x \"code\" \"none\")"]), Exports::Opaque);
        assert_eq!(merged(&["1"]), Exports::Opaque);
        assert_eq!(merged(&["(x)", "both"]), Exports::Both);
        assert_eq!(merged(&["foo"]), Exports::Code);
    }
}
