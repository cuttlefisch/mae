//! Objects (inline): emphasis, code, entities, scripts, targets, …

use super::build::{Builder, Mode};
use super::tree::{push, text, text_owned, Attr, Element, Node, Tag};
use crate::kind::SyntaxKind::{self, *};
use crate::kind::SyntaxNode;
use crate::semantics::plain::collapse;
use rowan::NodeOrToken;

/// The UTF-8 replacement of entity `name` (`org-entities`).
pub(crate) fn entity_utf8(name: &str) -> Option<&'static str> {
    let names = crate::entities::ENTITY_NAMES;
    let i = names
        .binary_search_by(|n| n.as_bytes().cmp(name.as_bytes()))
        .ok()?;
    crate::entities::ENTITY_UTF8.get(i).copied()
}

/// An object's post-blank: the blanks Org counts after it, which are text
/// after the element, not in it.
fn post_blank(n: &SyntaxNode) -> Option<String> {
    match n.last_child_or_token() {
        Some(NodeOrToken::Token(t)) if t.kind() == WHITESPACE => Some(t.text().to_string()),
        _ => None,
    }
}

/// `n`'s source without its post-blank.
pub(crate) fn raw(n: &SyntaxNode) -> String {
    let s = n.to_string();
    let cut = post_blank(n).map_or(0, |p| p.len());
    s[..s.len() - cut].to_string()
}

fn token_text(n: &SyntaxNode, kind: SyntaxKind) -> String {
    n.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == kind)
        .map(|t| t.text().to_string())
        .collect()
}

/// `org-html-convert-special-strings` (`org-export-with-special-strings`,
/// on by default) on one plain-text object, its regexps in their order,
/// each over the whole string left to right: `\-` a soft hyphen, `---`
/// then `--` before a character that is not `-` an em and an en dash,
/// `...` an ellipsis. A string's last dashes, with nothing after them in
/// the same object, stay. A string holding none of `\-`, `--` and `...`
/// is returned as it is, without a copy.
pub(crate) fn special_strings(s: String) -> String {
    // One pass for the pairs every rewrite needs (`...` holds `..`).
    let rewrites = |w: &[u8]| matches!(w, [b'\\' | b'-', b'-'] | [b'.', b'.']);
    if !s.as_bytes().windows(2).any(rewrites) {
        return s;
    }
    rewrite_special_strings(&s)
}

/// [`special_strings`]' rewrites, run whatever `s` holds.
fn rewrite_special_strings(s: &str) -> String {
    let s = s.replace("\\-", "\u{ad}");
    let s = dashes(&s, 3, '\u{2014}');
    let s = dashes(&s, 2, '\u{2013}');
    s.replace("...", "\u{2026}")
}

/// `-{n}([^-])` becomes `with` and the character, scanned left to right.
fn dashes(s: &str, n: usize, with: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let run = rest.bytes().take(n).take_while(|&b| b == b'-').count();
        let next = rest[run..].chars().next();
        match next {
            Some(after) if run == n && after != '-' => {
                out.push(with);
                out.push(after);
                rest = &rest[n + after.len_utf8()..];
            }
            _ => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

/// The key of a NAME: what a radio target and its links share, what a
/// target and a link searching for it share (Org matches both with blanks
/// collapsed and case folded, as Emacs folds it), and what `replace_body`
/// compares every name by. One function, so no two names the renderer
/// treats as one can differ to the edit check (review round 4, M2).
pub(crate) fn name_key(s: &str) -> String {
    crate::chars::fold(&collapse(s))
}

impl Builder<'_> {
    /// The objects under `n` (tokens are text).
    pub fn objects(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        self.parts(n.children_with_tokens(), false, out);
    }

    /// An object's contents: its children but its delimiters and post-blank.
    fn contents(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let parts: Vec<_> = n.children_with_tokens().collect();
        let end = parts.len() - usize::from(post_blank(n).is_some());
        self.parts(parts.into_iter().take(end), true, out);
    }

    /// Objects and the plain text between them, in order: each run of
    /// tokens is one plain-text object ([`Builder::plain_text`]). With
    /// `markup`, a markup token (a delimiter) ends a run and shows
    /// nothing; an affiliated keyword shows nothing.
    pub fn parts(
        &mut self,
        parts: impl IntoIterator<Item = crate::kind::SyntaxElement>,
        markup: bool,
        out: &mut Vec<Node>,
    ) {
        let mut plain = String::new();
        for c in parts {
            match c {
                NodeOrToken::Token(t) if markup && t.kind() == MARKUP => {
                    self.plain_text(&mut plain, out)
                }
                NodeOrToken::Token(t) => plain.push_str(t.text()),
                NodeOrToken::Node(o) if o.kind() == AFFILIATED_KEYWORD => {}
                NodeOrToken::Node(o) => {
                    self.plain_text(&mut plain, out);
                    self.deeper(&o, |b| b.object(&o, out));
                }
            }
        }
        self.plain_text(&mut plain, out);
    }

    /// One plain-text object (`plain`, emptied): its special strings
    /// converted as ox-html converts them, on a page and in metadata; not
    /// in search text or a slug.
    fn plain_text(&self, plain: &mut String, out: &mut Vec<Node>) {
        if plain.is_empty() {
            return;
        }
        let s = std::mem::take(plain);
        if self.mode == Mode::Text || self.slugging {
            push(out, text_owned(s));
        } else {
            push(out, text_owned(special_strings(s)));
        }
    }

    fn wrap(&mut self, n: &SyntaxNode, e: Element, out: &mut Vec<Node>) {
        let mut inner = Vec::new();
        self.contents(n, &mut inner);
        push(out, e.with(inner).into());
    }

    /// Whether results macro `n` is one Org's babel leaves as written
    /// (see [`super::babel::Babel::left_as_written`]), though MAE hides it.
    pub fn org_keeps_results(&self, n: &SyntaxNode) -> bool {
        super::babel::results_macro_owner(n).is_some_and(|owner| {
            let at = self.babel_position(&owner);
            self.babel().left_as_written(&owner, at)
        })
    }

    /// Whether Org's export removes object `n` for babel's `:exports`: an
    /// inline src block that does not export its code, or the results macro
    /// of one, or of an inline babel call, that does not keep its results
    /// (Org keeps an inline call's: a registered divergence,
    /// `inline-call-results`).
    pub fn babel_hides_object(&self, n: &SyntaxNode) -> bool {
        match n.kind() {
            INLINE_SRC_BLOCK if self.slugging || self.in_value => true,
            INLINE_SRC_BLOCK => !self
                .babel()
                .inline_exports(n, self.babel_position(n))
                .shows_code(),
            MACRO => super::babel::results_macro_owner(n).is_some_and(|owner| {
                if self.slugging || self.in_value {
                    return true;
                }
                let at = self.babel_position(&owner);
                let e = match owner.kind() {
                    INLINE_SRC_BLOCK => self.babel().inline_exports(&owner, at),
                    _ => self.babel().inline_call_exports(&owner, at),
                };
                !e.keeps_results()
            }),
            _ => false,
        }
    }

    /// One object, then its post-blank (nothing, post-blank included, for
    /// an object Org's export removes).
    ///
    /// Where an object is shown as its source text (`raw`), that IS what is
    /// shown, never a fallback for something hidden (review B1): a cookie,
    /// a LaTeX fragment and a citation are drawn as written (registered:
    /// latex, macros-citations), and a macro only in `replace_body`'s
    /// comparison of a document whose macros fail (both sides as written);
    /// an undefined footnote reference shows nothing (see `notes`); an
    /// entity is only parsed for a name in
    /// `org-entities`, so its source is never reached; and no other object
    /// kind reaches the last arm (a table cell is drawn by its row, a
    /// citation reference inside its citation).
    pub fn object(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        // A `nocite` citation exports nothing, its post-blank with it, as
        // an object babel removes does (see `cite`).
        if self.babel_hides_object(n) || (n.kind() == CITATION && self.nocite(n)) {
            return;
        }
        let span = |c: &str| Element::new(Tag::Span).class(c);
        let code = |c: Option<&str>, s: String| {
            let e = Element::new(Tag::Code);
            let e = c.map_or(e.clone(), |c| e.class(c));
            Node::from(e.with(vec![text(&s)]))
        };
        match n.kind() {
            BOLD => self.wrap(n, Element::new(Tag::Strong), out),
            ITALIC => self.wrap(n, Element::new(Tag::Em), out),
            UNDERLINE => self.wrap(n, span("org-underline"), out),
            STRIKE_THROUGH => self.wrap(n, Element::new(Tag::Del), out),
            SUBSCRIPT => self.wrap(n, Element::new(Tag::Sub), out),
            SUPERSCRIPT => self.wrap(n, Element::new(Tag::Sup), out),
            CODE => push(out, code(None, inner_text(n))),
            VERBATIM => push(out, code(Some("org-verbatim"), inner_text(n))),
            ENTITY => {
                let name = token_text(n, NAME);
                let s = entity_utf8(&name).map_or_else(|| raw(n), str::to_string);
                push(out, text(&s));
            }
            LINE_BREAK => {
                push(out, Element::new(Tag::Br).into());
                push(out, text("\n"));
            }
            LINK => self.link(n, out),
            TARGET => self.target(n, out),
            RADIO_TARGET => self.radio_target(n, out),
            FOOTNOTE_REFERENCE => self.footnote_reference(n, out),
            TIMESTAMP => {
                let t = Element::new(Tag::Time).class("org-timestamp");
                push(out, t.with(vec![text(&token_text(n, VALUE))]).into());
            }
            STATISTICS_COOKIE => push(out, span("org-cookie").with(vec![text(&raw(n))]).into()),
            LATEX_FRAGMENT => push(out, code(Some("org-latex"), raw(n))),
            INLINE_SRC_BLOCK => self.inline_src(n, out),
            // Every macro Org's export expands was expanded before this
            // parse, a results macro babel keeps included (see `expand`),
            // and one babel removes was dropped above. A macro left was
            // never one to Org's export where it stands (a property value,
            // a COMMENT subtree): it shows nothing.
            MACRO if self.raw_macros() => push(out, text(&raw(n))),
            MACRO => {}
            // As Org's basic processor exports an unresolved one: no key
            // shown (see `cite`).
            CITATION => self.citation(n, out),
            EXPORT_SNIPPET | INLINE_BABEL_CALL => {}
            // Not reached (see above); drawn as written rather than lost.
            _ => push(out, text(&raw(n))),
        }
        if let Some(p) = post_blank(n) {
            push(out, text(&p));
        }
    }

    fn inline_src(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let lang = token_text(n, NAME);
        let mut e = Element::new(Tag::Code).class("org-inline-src");
        if super::links::is_valid_language(&lang) {
            e = e.class(&format!("language-{lang}"));
        }
        push(out, e.with(vec![text(&token_text(n, VALUE))]).into());
    }

    /// `<<target>>`: an empty anchor (Org does not show a target's text).
    fn target(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        if self.mode == Mode::Meta {
            return;
        }
        let key = name_key(&collapse(&inner_text(n)));
        let mut e = Element::new(Tag::Span).class("org-target");
        if !key.is_empty() && !self.found.targets.contains_key(&key) {
            // Opaque, numbered in order of first appearance: Org never shows
            // a target's name (ox-html gives it a generated id), so no
            // attribute spells it either.
            let n = self.found.targets.len() + 1;
            if let Some(id) = self.ids.try_claim(format!("t-{n}")) {
                // Nor does a link landing here without a description show
                // it: Org shows "No description for this link", MAE the
                // resolver's placeholder (registered: target-links).
                self.found.targets.insert(key, id.clone());
                e = e.attr(Attr::Id, id);
            }
        }
        push(out, e.into());
    }

    /// `<<<radio>>>`: its text, an anchor for its radio links.
    fn radio_target(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let mut inner = Vec::new();
        self.contents(n, &mut inner);
        let mut e = Element::new(Tag::Span).class("org-radio-target");
        let key = name_key(&inner_text(n));
        if self.mode != Mode::Meta && !key.is_empty() && !self.found.radios.contains_key(&key) {
            // Numbered, as ox-html's generated reference: the text is shown,
            // the id does not spell it.
            let n = self.found.radios.len() + 1;
            if let Some(id) = self.ids.try_claim(format!("r-{n}")) {
                self.found.radios.insert(key, id.clone());
                e = e.attr(Attr::Id, id);
            }
        }
        push(out, e.with(inner).into());
    }
}

/// Whether an object a babel `:exports` or the HTML export can hide (an
/// inline src block, an inline babel call, an export snippet, or a results
/// macro) is in `n`.
pub(crate) fn holds_hidden(n: &SyntaxNode) -> bool {
    n.descendants().skip(1).any(|d| {
        matches!(
            d.kind(),
            INLINE_SRC_BLOCK | INLINE_BABEL_CALL | EXPORT_SNIPPET
        ) || (d.kind() == MACRO && token_text(&d, NAME).eq_ignore_ascii_case("results"))
    })
}

/// The text of an object's non-markup tokens (its contents, verbatim),
/// without its post-blank.
pub(crate) fn inner_text(n: &SyntaxNode) -> String {
    let parts: Vec<_> = n
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .collect();
    let end = parts.len() - usize::from(post_blank(n).is_some());
    parts
        .into_iter()
        .take(end)
        .filter(|t| t.kind() != MARKUP)
        .map(|t| t.text().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4000))]

        /// The no-copy path returns only what the rewrites would: over the
        /// characters they read (and their neighbours), with `.` and `-`
        /// common enough that `--`, `---`, `...` and `\-` all occur.
        #[test]
        fn special_strings_skips_only_what_it_would_not_change(
            s in "[-.\\\\a é\n]{0,24}"
        ) {
            prop_assert_eq!(special_strings(s.clone()), rewrite_special_strings(&s));
        }
    }
}
