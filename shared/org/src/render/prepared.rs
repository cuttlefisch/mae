//! The document a renderer renders: the source, or the parse of its text
//! with its macros expanded (see `expand`), and how to go back from it.

use super::build::DocCx;
use super::expand::Map;
use super::RenderDiagnostic;
use crate::kind::SyntaxKind::*;
use crate::{Document, SemanticNode, Semantics, Span};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) struct Prepared<'d> {
    pub doc: Cow<'d, Document>,
    pub sem: Cow<'d, Semantics>,
    /// `None`: nothing was expanded.
    map: Option<Map>,
    /// Expanded headline start → the source text of its title, which its
    /// slug is made from (anchors do not depend on macro definitions).
    titles: HashMap<u32, String>,
    /// Why nothing renders.
    pub failed: Option<RenderDiagnostic>,
    /// Environment substitutions (`MacroEnvironment`).
    pub notes: Vec<RenderDiagnostic>,
}

/// `doc` (semantics `sem`) as its export reads it: macros expanded, the
/// result parsed and projected with the options `doc` and `sem` were made
/// with.
pub(crate) fn prepare<'d>(doc: &'d Document, sem: &'d Semantics) -> Prepared<'d> {
    let mut p = Prepared {
        doc: Cow::Borrowed(doc),
        sem: Cow::Borrowed(sem),
        map: None,
        titles: HashMap::new(),
        failed: None,
        notes: Vec::new(),
    };
    let source = doc.syntax().to_string();
    let mut map = None;
    match super::expand::expand(doc) {
        Ok(None) => {}
        Err(d) => {
            p.failed = Some(d);
            return p;
        }
        Ok(Some(e)) => {
            p.adopt(doc.reparse(&e.text));
            map = Some(e.map);
            p.notes = e.notes;
        }
    }
    // Org's second expansion, after babel: the results macros it keeps
    // (in a keyword's value too, where no macro object shows them).
    if source.contains("{{{") {
        let removed = removed_results(&p.doc, &p.sem);
        let m = map.clone().unwrap_or_default();
        match super::expand::expand_results(&p.doc, m, source.len(), removed) {
            Ok(None) => {}
            Err(d) => {
                p.failed = Some(d);
                return p;
            }
            Ok(Some(e)) => {
                p.adopt(doc.reparse(&e.text));
                map = Some(e.map);
                p.notes.extend(e.notes);
            }
        }
    }
    if let Some(m) = map {
        p.titles = titles(&p.doc, &m, &source);
        p.map = Some(m);
    }
    p
}

/// The starts of the results macros of `doc` that babel removes: macro
/// objects whose `:exports` hides them, and in the value of a keyword the
/// export parses (where Org's second expansion finds them too) those its
/// `:exports` hides there; in an `EXPORT_` property, all (fail closed).
fn removed_results(doc: &Document, sem: &Semantics) -> HashSet<usize> {
    let is_results = |n: &crate::SyntaxNode| {
        n.kind() == MACRO
            && n.children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == NAME)
                .is_some_and(|t| t.text().eq_ignore_ascii_case("results"))
    };
    let (opts, resolver) = (super::RenderOptions::default(), super::Resolver::default());
    let mode = super::build::Mode::Page;
    let mut b = super::build::Builder::new(doc, sem, &opts, &resolver, mode, Default::default());
    let mut out = HashSet::new();
    for n in doc.syntax().descendants() {
        if is_results(&n) {
            if b.babel_hides_object(&n) {
                out.insert(usize::from(n.text_range().start()));
            }
            continue;
        }
        let keyword = n.kind() == KEYWORD
            && crate::Keyword::cast(n.clone()).is_some_and(|k| {
                let key = k.key().to_ascii_uppercase();
                super::expand::PARSED_KEYWORDS.contains(&key.as_str())
            });
        let property = n.kind() == NODE_PROPERTY && super::expand::is_export_property(&n);
        if !(keyword || property) {
            continue;
        }
        let Some((value, at)) = crate::semantics::value_token(&n) else {
            continue;
        };
        let tree = crate::green::objects(&value, PARAGRAPH);
        for m in tree.descendants().filter(is_results) {
            let hides = property || b.with_base(at, |b| b.babel_hides_object(&m));
            if hides {
                out.insert(at + usize::from(m.text_range().start()));
            }
        }
    }
    out
}

/// Each expanded headline's title, as written in the source.
fn titles(doc: &Document, map: &Map, source: &str) -> HashMap<u32, String> {
    doc.syntax()
        .descendants()
        .filter(|n| n.kind() == HEADLINE_TITLE)
        .filter_map(|t| {
            let h = t.parent()?;
            let s = map.to_source(Span::of(&t));
            let text = source.get(s.range())?.to_string();
            Some((Span::of(&h).start, text))
        })
        .collect()
}

impl<'d> Prepared<'d> {
    /// Renders `d` (an expansion of the source) from now on, with its
    /// semantics derived under the source's options.
    fn adopt(&mut self, d: Document) {
        let s = crate::semantics_with(&d, &self.sem.options);
        self.doc = Cow::Owned(d);
        self.sem = Cow::Owned(s);
    }

    /// The builders' shared reading of the document.
    pub fn cx(&self) -> Rc<DocCx<'_>> {
        DocCx::with_titles(&self.doc, &self.sem, self.titles.clone())
    }

    /// A diagnostic of the rendered text, at its place in the source.
    pub fn to_source(&self, d: RenderDiagnostic) -> RenderDiagnostic {
        match &self.map {
            Some(m) => d.at(m.to_source(d.span())),
            None => d,
        }
    }

    /// A source span, in the rendered text.
    pub fn to_rendered(&self, s: Span) -> Span {
        self.map.as_ref().map_or(s, |m| m.to_expanded(s))
    }

    /// Each node of `source` (the source's semantics) and its node in the
    /// rendered text: the one with its id (the n-th of that id for the
    /// n-th), or none when the expansion removed it.
    pub fn counterparts<'s>(
        &'s self,
        source: &'s Semantics,
    ) -> Vec<(&'s SemanticNode, Option<&'s SemanticNode>)> {
        if self.map.is_none() {
            return source.nodes.iter().map(|n| (n, Some(n))).collect();
        }
        let mut by_id: HashMap<&str, Vec<&SemanticNode>> = HashMap::new();
        for n in &self.sem.nodes {
            by_id.entry(n.id.as_str()).or_default().push(n);
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        source
            .nodes
            .iter()
            .map(|n| {
                let k = seen.entry(n.id.as_str()).or_insert(0);
                let m = by_id.get(n.id.as_str()).and_then(|v| v.get(*k)).copied();
                *k += 1;
                (n, m)
            })
            .collect()
    }
}
