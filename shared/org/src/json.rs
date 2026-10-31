//! JSON of [`Semantics`]: the shape a WASM, MCP or Scheme consumer reads
//! (ADR-112 D6: one surface). Hand-written, so mae-org needs no serde
//! (ADR-112 D1: rowan is its only dependency); the WASM facade's tests parse
//! it back with a real JSON parser.
//!
//! Spans are `{"start", "end"}` BYTE offsets into the UTF-8 text (not
//! UTF-16 code units). Keys are always present, absent values are `null`,
//! and every list is in the order [`Semantics`] documents.

use crate::render::json_str;
use crate::{
    LinkClass, NodeOrigin, SemanticDiagnostic, SemanticLink, SemanticNode, Semantics, Span,
    Transclusion,
};

/// `s` as a JSON string literal, escaped as every JSON this crate writes
/// is (control characters, U+2028 and U+2029 as `\u` escapes).
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    json_str(s, &mut out);
    out
}

struct W(String);

impl W {
    fn raw(&mut self, s: &str) -> &mut Self {
        self.0.push_str(s);
        self
    }

    fn key(&mut self, k: &str) -> &mut Self {
        if !matches!(self.0.chars().last(), Some('{' | '[') | None) {
            self.0.push(',');
        }
        json_str(k, &mut self.0);
        self.0.push(':');
        self
    }

    fn str(&mut self, s: &str) -> &mut Self {
        json_str(s, &mut self.0);
        self
    }

    fn opt(&mut self, s: Option<&str>) -> &mut Self {
        match s {
            Some(s) => self.str(s),
            None => self.raw("null"),
        }
    }

    fn span(&mut self, s: Span) -> &mut Self {
        let t = format!("{{\"start\":{},\"end\":{}}}", s.start, s.end);
        self.raw(&t)
    }

    fn strs(&mut self, v: &[String]) -> &mut Self {
        self.list(v, |w, s| {
            w.str(s);
        })
    }

    fn pairs(&mut self, v: &[(String, String)]) -> &mut Self {
        self.list(v, |w, (k, val)| {
            w.raw("{").key("key").str(k).key("value").str(val).raw("}");
        })
    }

    fn list<T>(&mut self, v: &[T], f: impl Fn(&mut W, &T)) -> &mut Self {
        self.0.push('[');
        for (i, x) in v.iter().enumerate() {
            if i > 0 {
                self.0.push(',');
            }
            f(self, x);
        }
        self.0.push(']');
        self
    }
}

fn origin(o: NodeOrigin) -> &'static str {
    match o {
        NodeOrigin::File => "file",
        NodeOrigin::Headline => "headline",
        NodeOrigin::Item => "item",
    }
}

fn node(w: &mut W, sem: &Semantics, n: &SemanticNode) {
    let priority = n.priority.map(|c| c.to_string());
    w.raw("{").key("id").str(&n.id);
    w.key("origin").str(origin(n.origin));
    w.key("level").raw(&n.level.to_string());
    w.key("kind").opt(n.kind.as_deref());
    w.key("title").opt(n.title.as_deref());
    w.key("aliases").strs(&n.aliases);
    w.key("own_tags").strs(&n.own_tags);
    w.key("tags").strs(&sem.tags_of(n));
    w.key("properties").pairs(&n.properties);
    w.key("todo").opt(n.todo.as_deref());
    w.key("priority").opt(priority.as_deref());
    w.key("span").span(n.span);
    w.key("body_span").span(n.body_span).raw("}");
}

fn link(w: &mut W, l: &SemanticLink) {
    w.raw("{").key("from").opt(l.from.as_deref());
    w.key("kind").str(l.kind.as_str());
    w.key("path").str(&l.path);
    w.key("target").str(&l.target);
    w.key("fragment").opt(l.fragment.as_deref());
    w.key("search").opt(l.search.as_deref());
    w.key("rel").str(&l.rel);
    w.key("weight").raw(&l.weight.to_string());
    w.key("confidence").raw(&l.confidence.to_string());
    w.key("attrs").pairs(&l.attrs);
    w.key("description").opt(l.description.as_deref());
    w.key("span").span(l.span).raw("}");
}

fn transclusion(w: &mut W, t: &Transclusion) {
    w.raw("{").key("from").opt(t.from.as_deref());
    w.key("member").str(&t.member);
    w.key("role").str(&t.role);
    w.key("args").opt(t.args.as_deref());
    w.key("span").span(t.span).raw("}");
}

fn diagnostic(w: &mut W, d: &SemanticDiagnostic) {
    w.raw("{").key("kind").str(d.kind_name());
    w.key("span").span(d.span());
    match d {
        SemanticDiagnostic::IdInUnrenderedRegion { id, region, .. } => {
            w.key("id").str(id).key("region").str(region.as_str());
        }
        SemanticDiagnostic::LinkInUnrenderedRegion { path, region, .. } => {
            w.key("path").str(path).key("region").str(region.as_str());
        }
        SemanticDiagnostic::TransclusionInUnrenderedRegion { member, region, .. } => {
            w.key("member")
                .str(member)
                .key("region")
                .str(region.as_str());
        }
        SemanticDiagnostic::DuplicateId { id, first, .. } => {
            w.key("id").str(id).key("first").span(*first);
        }
        SemanticDiagnostic::SetupFileNotRead { file, .. } => {
            w.key("file").str(file);
        }
    }
    w.raw("}");
}

impl LinkClass {
    /// The classification as JSON: `{"kind", "target", "fragment",
    /// "search", "rel", "weight", "confidence", "attrs", "description"}`.
    pub fn to_json(&self) -> String {
        let mut w = W(String::new());
        w.raw("{").key("kind").str(self.kind.as_str());
        w.key("target").str(&self.target);
        w.key("fragment").opt(self.fragment.as_deref());
        w.key("search").opt(self.search.as_deref());
        w.key("rel").str(&self.rel);
        w.key("weight").raw(&self.weight.to_string());
        w.key("confidence").raw(&self.confidence.to_string());
        w.key("attrs").pairs(&self.attrs);
        w.key("description").opt(self.description.as_deref());
        w.raw("}");
        w.0
    }
}

impl Semantics {
    /// The semantics as JSON: `{"file_tags", "nodes", "links",
    /// "transclusions", "diagnostics", "unrendered", "footnotes"}` (see the
    /// module doc; a node's `tags` is [`Semantics::tags_of`]).
    pub fn to_json(&self) -> String {
        let mut w = W(String::new());
        w.raw("{").key("file_tags").strs(&self.file_tags);
        w.key("nodes").list(&self.nodes, |w, n| node(w, self, n));
        w.key("links").list(&self.links, link);
        w.key("transclusions")
            .list(&self.transclusions, transclusion);
        w.key("diagnostics").list(&self.diagnostics, diagnostic);
        w.key("unrendered").list(&self.unrendered, |w, u| {
            let element = u.element.org_element_name().unwrap_or("unknown");
            w.raw("{").key("span").span(u.span);
            w.key("element").str(element);
            w.key("region").str(u.region.as_str()).raw("}");
        });
        w.key("footnotes").list(&self.footnotes, |w, f| {
            w.raw("{").key("label").str(&f.label);
            w.key("span").span(f.span).raw("}");
        });
        w.raw("}");
        w.0
    }
}
