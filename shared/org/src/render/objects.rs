//! Objects (inline): emphasis, code, entities, scripts, targets, …

use super::anchors::encode_id;
use super::build::{Builder, Mode};
use super::tree::{push, text, Attr, Element, Node, Tag};
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

/// The key a radio target and its links share: whitespace collapsed, case
/// folded as Emacs folds it (radio links match case-insensitively).
pub(crate) fn radio_key(s: &str) -> String {
    crate::chars::fold(&collapse(s))
}

impl Builder<'_> {
    /// The objects under `n` (tokens are text).
    pub fn objects(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        for c in n.children_with_tokens() {
            match c {
                NodeOrToken::Token(t) => push(out, text(t.text())),
                NodeOrToken::Node(o) if o.kind() == AFFILIATED_KEYWORD => {}
                NodeOrToken::Node(o) => self.deeper(&o, |b| b.object(&o, out)),
            }
        }
    }

    /// An object's contents: its children but its delimiters and post-blank.
    fn contents(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let parts: Vec<_> = n.children_with_tokens().collect();
        let end = parts.len() - usize::from(post_blank(n).is_some());
        for c in parts.into_iter().take(end) {
            match c {
                NodeOrToken::Token(t) if t.kind() == MARKUP => {}
                NodeOrToken::Token(t) => push(out, text(t.text())),
                NodeOrToken::Node(o) => self.deeper(&o, |b| b.object(&o, out)),
            }
        }
    }

    fn wrap(&mut self, n: &SyntaxNode, e: Element, out: &mut Vec<Node>) {
        let mut inner = Vec::new();
        self.contents(n, &mut inner);
        push(out, e.with(inner).into());
    }

    /// One object, then its post-blank.
    pub fn object(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
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
            MACRO => push(out, span("org-macro").with(vec![text(&raw(n))]).into()),
            CITATION => push(out, span("org-citation").with(vec![text(&raw(n))]).into()),
            EXPORT_SNIPPET | INLINE_BABEL_CALL => {}
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
        let key = collapse(&inner_text(n));
        let mut e = Element::new(Tag::Span).class("org-target");
        if !key.is_empty() && !self.found.targets.contains_key(&key) {
            if let Some(id) = self.ids.try_claim(format!("t-{}", encode_id(&key))) {
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
        let key = radio_key(&inner_text(n));
        if self.mode != Mode::Meta && !key.is_empty() && !self.found.radios.contains_key(&key) {
            if let Some(id) = self.ids.try_claim(format!("r-{}", encode_id(&key))) {
                self.found.radios.insert(key, id.clone());
                e = e.attr(Attr::Id, id);
            }
        }
        push(out, e.with(inner).into());
    }
}

/// The text of an object's non-markup tokens (its contents, verbatim),
/// without its post-blank.
fn inner_text(n: &SyntaxNode) -> String {
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
