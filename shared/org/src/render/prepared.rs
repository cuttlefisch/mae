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
    /// The document as written.
    source: &'d Document,
    /// Not expanded: macros are shown as written (see [`unexpanded`]).
    raw: bool,
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
        source: doc,
        raw: false,
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
        let (removed, kept) = removed_results(&p.doc, &p.sem);
        let m = map.clone().unwrap_or_default();
        // Org leaves the results macros whose `:exports` it does not know
        // in place and expands them, which MAE hides: their expansion
        // must still succeed, or Org's export fails, and so does this.
        if !kept.is_empty() {
            let org: HashSet<usize> = removed.difference(&kept).copied().collect();
            if let Err(d) = super::expand::expand_results(&p.doc, m.clone(), source.len(), org) {
                p.failed = Some(d);
                return p;
            }
        }
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
    if let Some(m) = &map {
        p.keep_source_nodes(sem, m);
    }
    p.map = map;
    p
}

/// `doc` with nothing expanded, its macros shown as written: what
/// `replace_body` compares when either side's macros cannot be expanded,
/// so the comparison still sees every change outside the edited body.
pub(crate) fn unexpanded<'d>(doc: &'d Document, sem: &'d Semantics) -> Prepared<'d> {
    Prepared {
        doc: Cow::Borrowed(doc),
        sem: Cow::Borrowed(sem),
        map: None,
        source: doc,
        raw: true,
        failed: None,
        notes: Vec::new(),
    }
}

/// The starts of the results macros of `doc` that babel removes: macro
/// objects whose `:exports` hides them, and in the value of a keyword the
/// export parses (where Org's second expansion finds them too) those its
/// `:exports` hides there; in an `EXPORT_` property, all (fail closed).
fn removed_results(doc: &Document, sem: &Semantics) -> (HashSet<usize>, HashSet<usize>) {
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
    let mut kept = HashSet::new();
    for n in doc.syntax().descendants() {
        if is_results(&n) {
            if b.babel_hides_object(&n) {
                out.insert(usize::from(n.text_range().start()));
                if b.org_keeps_results(&n) {
                    kept.insert(usize::from(n.text_range().start()));
                }
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
    (out, kept)
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
        let mut cx = DocCx::with_source(&self.doc, &self.sem, Some(self.source));
        if self.raw {
            Rc::get_mut(&mut cx).expect("a fresh reading").raw_macros = true;
        }
        cx
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

    /// The rendered text's KB nodes are those that come from the source's
    /// (the node of an expanded headline, item or file that stands where a
    /// source node stands, with its id); a headline or drawer an expansion
    /// made is never one, and lends nobody its id (review round 4, M6).
    fn keep_source_nodes(&mut self, source: &Semantics, map: &super::expand::Map) {
        let at: HashMap<(u32, crate::NodeOrigin), &str> = source
            .nodes
            .iter()
            .map(|n| ((n.span.start, n.origin), n.id.as_str()))
            .collect();
        let real = |n: &SemanticNode| {
            let s = map.to_source(Span::new(n.span.start as usize, n.span.start as usize));
            at.get(&(s.start, n.origin)) == Some(&n.id.as_str())
        };
        if self.sem.nodes.iter().all(real) {
            return;
        }
        // A node the expansion made can hold an id a real node has, and
        // the semantics keep only the first of an id: derive them again
        // with the made nodes' ids masked (byte for byte, so every span
        // stays), then drop the made nodes.
        let mut bytes = self.doc.syntax().to_string().into_bytes();
        for n in self.sem.nodes.iter().filter(|n| !real(n)) {
            mask_ids(
                &mut bytes,
                n.span.start as usize,
                n.body_span.start as usize,
            );
        }
        let masked = String::from_utf8(bytes).expect("whole characters masked");
        let mut sem = crate::semantics_with(&self.doc.reparse(&masked), &self.sem.options);
        sem.nodes.retain(real);
        self.sem = Cow::Owned(sem);
    }

    /// Each node of `source` (the source's semantics) that the rendered
    /// page exports, with its node in the rendered text. The semantics list
    /// only exported nodes, so a node an expansion put in a COMMENT or
    /// `:noexport:` subtree (or removed) has none.
    pub fn counterparts<'s>(
        &'s self,
        source: &'s Semantics,
    ) -> Vec<(&'s SemanticNode, &'s SemanticNode)> {
        let mut by_id: HashMap<&str, Vec<&SemanticNode>> = HashMap::new();
        for n in &self.sem.nodes {
            by_id.entry(n.id.as_str()).or_default().push(n);
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        source
            .nodes
            .iter()
            .filter_map(|n| {
                let k = seen.entry(n.id.as_str()).or_insert(0);
                let m = by_id.get(n.id.as_str()).and_then(|v| v.get(*k)).copied();
                *k += 1;
                m.map(|m| (n, m))
            })
            .collect()
    }
}
/// Masks the value of every `:ID:` line in `bytes[a..b]` (a node's header):
/// each of its bytes becomes `\x01`, which no id is made of.
fn mask_ids(bytes: &mut [u8], a: usize, b: usize) {
    let b = b.min(bytes.len());
    let mut line = a;
    while line < b {
        let end = bytes[line..b]
            .iter()
            .position(|&c| c == b'\n' || c == b'\r')
            .map_or(b, |k| line + k);
        let text = &bytes[line..end];
        let lead = text
            .iter()
            .take_while(|c| matches!(c, b' ' | b'\t'))
            .count();
        if text.len() >= lead + 4 && text[lead..lead + 4].eq_ignore_ascii_case(b":ID:") {
            for c in &mut bytes[line + lead + 4..end] {
                if !matches!(*c, b' ' | b'\t') {
                    *c = 1;
                }
            }
        }
        line = end + 1;
    }
}
