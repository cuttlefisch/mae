//! Citations, as Org's basic processor (`oc-basic`, the default) exports
//! one whose keys it cannot resolve: MAE reads no bibliography, so every
//! key is unknown, and a key is never shown (registered: citations).
//!
//! Measured in Emacs 30.2 (`emacs -Q`, no bibliography), per style
//! (`[cite/STYLE/VARIANT:…]`), each reference giving an author `??` and a
//! year (below):
//! - default (no style, or any unknown one): `(PRE REFS SUF)`, each
//!   reference `pre ??, YEAR suf`, references joined by `, `; the
//!   parentheses dropped by a `bare` variant (`b`, `bare`, `bc`,
//!   `bare-caps`);
//! - `author`/`a`: `PRE REFS SUF`, each `pre ?? suf`;
//! - `noauthor`/`na`: `(PRE REFS SUF)` (bare: no parentheses), each
//!   `pre YEAR suf`;
//! - `text`/`t` (and `note`/`ft`): `PRE REFS SUF`, each `pre ?? (YEAR) suf`
//!   (bare: `pre ?? YEAR suf`);
//! - `nocite`/`n`: nothing;
//! - `numeric`/`nb`: `(PRE NUMBERS SUF)`, a bare variant too, with the
//!   affixes of the only reference, else the citation's, and each key's
//!   number its order of first citation in the page (`1, 2`, a run `1-3`).
//!
//! The year of an unknown key is `oc-basic`'s disambiguation of the shared
//! unknown author and year: the page's first key cited gets `????` the
//! first time and nothing after; every other key `a`.
//!
//! Prefixes and suffixes are rendered as objects where Emacs shows them; a
//! citation holding a construct the export hides (an inline src block, a
//! babel call, an export snippet, a results macro) shows none of them
//! (fail closed: Org exports an inline src block's code there). A macro in
//! a citation is never expanded (Org's export does not see it as one) and
//! renders nothing, as in Emacs.
//! Not reproduced: a `note` citation outside a footnote, which Org moves
//! into a new footnote, renders in place as `text`.

use super::build::Builder;
use super::objects::holds_hidden;
use super::tree::{push, text, Element, Node, Tag};
use crate::kind::SyntaxKind::*;
use crate::kind::{SyntaxElement, SyntaxNode};
use crate::seeded::HashMap;
use rowan::NodeOrToken;

/// What a page's citations so far decide about the next ones.
#[derive(Debug, Default)]
pub(crate) struct Citations {
    /// The first key cited (its first year was `????`).
    first: Option<String>,
    /// Key → its number, in order of first citation.
    numbers: HashMap<String, usize>,
}

/// One reference: its prefix, key and suffix.
struct Reference {
    prefix: Vec<SyntaxElement>,
    key: String,
    suffix: Vec<SyntaxElement>,
}

/// A citation's parts: style, variant, global prefix, references, global
/// suffix.
struct Parts {
    style: String,
    variant: String,
    prefix: Vec<SyntaxElement>,
    references: Vec<Reference>,
    suffix: Vec<SyntaxElement>,
}

fn parts(n: &SyntaxNode) -> Parts {
    let mut children = n.children_with_tokens().peekable();
    let opening = children
        .next()
        .and_then(SyntaxElement::into_token)
        .map(|t| t.text().to_string())
        .unwrap_or_default();
    let spec = opening
        .get(5..)
        .and_then(|s| s.strip_suffix(':'))
        .and_then(|s| s.strip_prefix('/'))
        .unwrap_or("");
    let (style, variant) = spec.split_once('/').unwrap_or((spec, ""));
    let mut p = Parts {
        style: style.to_string(),
        variant: variant.to_string(),
        prefix: Vec::new(),
        references: Vec::new(),
        suffix: Vec::new(),
    };
    // The blanks after the colon are no prefix.
    children.next_if(|c| c.kind() == WHITESPACE);
    let rest: Vec<SyntaxElement> = children.collect();
    let first = rest.iter().position(|c| c.kind() == CITATION_REFERENCE);
    let last = rest.iter().rposition(|c| c.kind() == CITATION_REFERENCE);
    let (Some(first), Some(last)) = (first, last) else {
        return p;
    };
    // Before the first reference: the global prefix and its `;`.
    p.prefix = rest[..first]
        .iter()
        .filter(|c| !is_markup(c))
        .cloned()
        .collect();
    for r in rest[first..=last].iter().filter_map(|c| c.as_node()) {
        p.references.push(reference(r));
    }
    // After the last: the global suffix, then the blanks and `]` before
    // the post-blank.
    let tail: Vec<SyntaxElement> = rest[last + 1..].to_vec();
    let close = tail.iter().rposition(is_markup).unwrap_or(tail.len());
    let mut suffix = tail[..close].to_vec();
    if suffix.last().is_some_and(|c| c.kind() == WHITESPACE) {
        suffix.pop();
    }
    p.suffix = suffix;
    p
}

fn is_markup(c: &SyntaxElement) -> bool {
    c.as_token().is_some_and(|t| t.kind() == MARKUP)
}

fn reference(r: &SyntaxNode) -> Reference {
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut key = None;
    for c in r.children_with_tokens() {
        match &c {
            NodeOrToken::Token(t) if t.kind() == NAME => key = Some(t.text().to_string()),
            NodeOrToken::Token(t) if t.kind() == MARKUP => {}
            _ if key.is_none() => prefix.push(c),
            _ => suffix.push(c),
        }
    }
    Reference {
        prefix,
        key: key.unwrap_or_default(),
        suffix,
    }
}

impl Citations {
    /// `org-cite-basic--get-year` of an unknown key.
    fn year(&mut self, key: &str) -> &'static str {
        match &self.first {
            None => {
                self.first = Some(key.to_string());
                "????"
            }
            Some(k) if k == key => "",
            Some(_) => "a",
        }
    }

    /// `org-cite-basic--citation-numbers`: sorted, runs compacted.
    fn numbers(&self, keys: &[String]) -> String {
        let mut ns: Vec<usize> = keys
            .iter()
            .filter_map(|k| self.numbers.get(k))
            .copied()
            .collect();
        ns.sort_unstable();
        let mut out = String::new();
        let mut i = 0;
        while i < ns.len() {
            let mut j = i;
            while j + 1 < ns.len() && ns[j + 1] == ns[j] + 1 {
                j += 1;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            if j >= i + 2 {
                out.push_str(&format!("{}-{}", ns[i], ns[j]));
            } else {
                out.push_str(&ns[i].to_string());
                if j > i {
                    out.push_str(&format!(", {}", ns[j]));
                }
            }
            i = j + 1;
        }
        out
    }
}

impl Builder<'_> {
    /// Whether citation `n` is `nocite` (`n`), which exports nothing; its
    /// keys are numbered all the same (Org lists every cited key).
    pub fn nocite(&mut self, n: &SyntaxNode) -> bool {
        let p = parts(n);
        let nocite = matches!(p.style.as_str(), "nocite" | "n");
        if nocite {
            self.number(&p);
        }
        nocite
    }

    fn number(&mut self, p: &Parts) {
        for r in &p.references {
            let next = self.citations.numbers.len() + 1;
            self.citations.numbers.entry(r.key.clone()).or_insert(next);
        }
    }

    /// A citation, as `oc-basic` exports an unresolved one (see the module).
    pub fn citation(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        if self.in_doc_title {
            return;
        }
        let p = parts(n);
        let affixes = !holds_hidden(n);
        self.number(&p);
        let bare = matches!(p.variant.as_str(), "bare" | "bare-caps" | "b" | "bc");
        let mut inner = Vec::new();
        let style = p.style.as_str();
        if matches!(style, "nocite" | "n") {
            return;
        }
        // `numeric` is parenthesised whatever the variant (`oc-basic`).
        let numeric = matches!(style, "numeric" | "nb");
        let paren =
            numeric || !bare && !matches!(style, "author" | "a" | "text" | "t" | "note" | "ft");
        if paren {
            push(&mut inner, text("("));
        }
        if numeric {
            let source = match &p.references[..] {
                [only] => (&only.prefix, &only.suffix),
                _ => (&p.prefix, &p.suffix),
            };
            self.affix(source.0, affixes, &mut inner);
            let keys: Vec<String> = p.references.iter().map(|r| r.key.clone()).collect();
            push(&mut inner, text(&self.citations.numbers(&keys)));
            self.affix(source.1, affixes, &mut inner);
        } else {
            self.affix(&p.prefix, affixes, &mut inner);
            for (i, r) in p.references.iter().enumerate() {
                if i > 0 {
                    push(&mut inner, text(", "));
                }
                self.affix(&r.prefix, affixes, &mut inner);
                let year = self.citations.year(&r.key);
                let shown = match style {
                    "author" | "a" => "??".to_string(),
                    "noauthor" | "na" => year.to_string(),
                    "text" | "t" | "note" | "ft" if bare => format!("?? {year}"),
                    "text" | "t" | "note" | "ft" => format!("?? ({year})"),
                    _ => format!("??, {year}"),
                };
                push(&mut inner, text(&shown));
                self.affix(&r.suffix, affixes, &mut inner);
            }
            self.affix(&p.suffix, affixes, &mut inner);
        }
        if paren {
            push(&mut inner, text(")"));
        }
        let e = Element::new(Tag::Span).class("org-citation").with(inner);
        push(out, e.into());
    }

    /// A prefix or suffix: its text and objects, when `shown`.
    fn affix(&mut self, parts: &[SyntaxElement], shown: bool, out: &mut Vec<Node>) {
        if !shown {
            return;
        }
        self.parts(parts.iter().cloned(), false, out);
    }
}
