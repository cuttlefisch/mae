//! KB node metadata: titles, aliases and properties, rendered inline-only
//! through the same builder as the body.

use super::build::{trim_text, Builder, Mode, Table};
use super::notes::node_at;
use super::tree::{push, text, Node};
use super::{NodeMeta, RenderOptions, Resolver};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::semantics::file_title_source;
use crate::semantics::items::{item_drawer, item_title_parts};
use crate::{NodeOrigin, RenderDiagnostic, SemanticNode, Semantics, Span};

/// The metadata of every node of `source` (the source's semantics) that
/// the page exports, in its order, read from its counterpart in the
/// rendered document `p` (its macros expanded): a node the expansion
/// removed, or in a subtree the export drops (COMMENT, `:noexport:`), has
/// no entry. What to publish is these nodes, not the semantics'.
pub(crate) fn nodes<'a>(
    p: &'a super::prepared::Prepared,
    cx: std::rc::Rc<super::build::DocCx<'a>>,
    source: &Semantics,
    opts: &RenderOptions,
    resolver: &Resolver,
    page: &Table,
) -> (Vec<NodeMeta>, Vec<RenderDiagnostic>) {
    let root = p.doc.syntax();
    let mut b = Builder::with_cx(cx, opts, resolver, Mode::Meta, Table::default());
    let nodes = p
        .counterparts(source)
        .into_iter()
        .map(|(_, n)| b.node_meta(&root, n, page))
        .collect();
    (nodes, b.diagnostics)
}

impl Builder<'_> {
    fn node_meta(&mut self, root: &SyntaxNode, n: &SemanticNode, page: &Table) -> NodeMeta {
        let b = self;
        let mut title = b.node_title(root, n);
        trim_text(&mut title);
        // As inside a link's description: links show their text, no
        // `<a>` (nor a footnote marker) is made.
        b.in_link = true;
        let mut title_unlinked = b.node_title(root, n);
        b.in_link = false;
        trim_text(&mut title_unlinked);
        unwrap_plain(&mut title_unlinked);
        // Aliases and property values are the semantics' strings, read
        // from the node's drawer: no place of their own, but they sit in
        // the node, so their `:exports` scope is the node's (review
        // round 2: an alias read the file's and showed hidden code).
        let at = Some(n.span.start);
        // The aliases are the ALIASES value split at its commas, and a
        // split can cut a construct the whole value hides (a macro's
        // arguments hold commas too): when the whole value hides
        // anything, no alias is shown (fail closed).
        let whole = alias_source(root, n);
        let cut = whole.is_some_and(|v| b.placeless(at, |b| b.value_hides(&v)));
        let aliases = n
            .aliases
            .iter()
            .filter(|_| !cut)
            .map(|a| b.placeless(at, |b| b.inline_text(a)))
            .collect();
        // What `{{{property}}}` would read is built only when a value has a
        // macro to expand with it: it stringifies the entry, which costs more
        // than every plain value of a node together.
        let has_macro = n.properties.iter().any(|(_, v)| v.contains("{{{"));
        let (entry, drawer) = if has_macro {
            node_entry(root, n)
        } else {
            (String::new(), n.span)
        };
        let properties = n
            .properties
            .iter()
            .map(|(k, v)| {
                let v = b.placeless(at, |b| b.macro_value(&entry, v, drawer));
                (k.clone(), v)
            })
            .collect();
        NodeMeta {
            id: n.id.clone(),
            anchor: page.nodes.get(&n.id).cloned(),
            title,
            title_unlinked,
            aliases,
            properties,
        }
    }
}

/// The whole value node `n`'s aliases were split from: for the file node
/// its last `#+aliases:` keyword (which wins over the drawer), else the
/// `ALIASES` property (its `+` lines appended).
fn alias_source(root: &SyntaxNode, n: &SemanticNode) -> Option<String> {
    if n.origin == NodeOrigin::File {
        let section = root.children().next().filter(|s| s.kind() == SECTION);
        let keyword = section.and_then(|s| {
            s.children()
                .filter_map(crate::Keyword::cast)
                .filter(|k| k.key().eq_ignore_ascii_case("aliases"))
                .last()
        });
        if let Some(k) = keyword {
            return Some(k.value());
        }
    }
    let p = n.properties.iter().find(|(k, _)| k == "ALIASES");
    p.map(|(_, v)| v.clone())
}

/// `text` as Org objects, inline-only (no document around it).
pub(crate) fn inline(text: &str, opts: &RenderOptions, resolver: &Resolver) -> Vec<Node> {
    let doc = crate::parse("", &crate::ParseOptions::default());
    let sem = Semantics::default();
    let mut b = Builder::new(&doc, &sem, opts, resolver, Mode::Meta, Table::default());
    b.placeless(None, |b| b.inline_text(text))
}

impl Builder<'_> {
    pub fn node_title(&mut self, root: &SyntaxNode, n: &SemanticNode) -> Vec<Node> {
        let mut out = Vec::new();
        match n.origin {
            NodeOrigin::File => {
                if let Some((value, at)) = file_title_source(root) {
                    let tree = crate::green::objects(&value, PARAGRAPH);
                    let was = std::mem::replace(&mut self.in_doc_title, true);
                    self.with_base(at, |b| b.objects(&tree, &mut out));
                    self.in_doc_title = was;
                }
            }
            NodeOrigin::Headline => {
                let title = node_at(root, n.span, HEADLINE)
                    .and_then(|h| h.children().find(|c| c.kind() == HEADLINE_TITLE));
                if let Some(t) = title {
                    self.objects(&t, &mut out);
                }
            }
            NodeOrigin::Item => self.item_title(root, n, &mut out),
        }
        out
    }

    fn item_title(&mut self, root: &SyntaxNode, n: &SemanticNode, out: &mut Vec<Node>) {
        if let Some(item) = node_at(root, n.span, ITEM) {
            self.item_node_title(&item, out);
        }
    }

    /// A KB item's title: its tag and its first paragraph, rendered.
    pub fn item_node_title(&mut self, item: &SyntaxNode, out: &mut Vec<Node>) {
        let Some((drawer, _)) = item_drawer(item) else {
            return;
        };
        let (tag, first) = item_title_parts(item, &drawer);
        let mut tag_nodes = Vec::new();
        if let Some(t) = tag {
            self.objects(&t, &mut tag_nodes);
        }
        trim_text(&mut tag_nodes);
        let mut para = Vec::new();
        if let Some(p) = first {
            self.objects(&p, &mut para);
        }
        trim_text(&mut para);
        let both = !tag_nodes.is_empty() && !para.is_empty();
        tag_nodes.into_iter().for_each(|t| push(out, t));
        if both {
            push(out, text(" :: "));
        }
        para.into_iter().for_each(|t| push(out, t));
    }

    /// `s` parsed as objects and rendered.
    pub fn inline_text(&mut self, s: &str) -> Vec<Node> {
        let tree = crate::green::objects(s, PARAGRAPH);
        let mut out = Vec::new();
        self.objects(&tree, &mut out);
        trim_text(&mut out);
        out
    }

    /// A property value: plain text in which only bracket links are links.
    ///
    /// Org reads a value as text and shows it as written. MAE reads it as
    /// objects and renders them inline-only through the one object path
    /// (so whatever the body hides is hidden at any depth: an inline src
    /// block or babel call, an export snippet, a macro that renders
    /// nothing, a citation's hidden parts), every inline src block hidden
    /// whatever its `:exports`; then keeps only its text, bracket links
    /// (and images) as rendered (registered: `property-value-babel`).
    pub fn property_value(&mut self, value: &str) -> Vec<Node> {
        if let Some(plain) = plain_value(value) {
            return plain;
        }
        self.property_value_parsed(value)
    }

    /// [`Builder::property_value`] by the object parser (always correct;
    /// the oracle [`plain_value`] is tested against).
    fn property_value_parsed(&mut self, value: &str) -> Vec<Node> {
        let tree = crate::green::objects(value, PARAGRAPH);
        let mode = std::mem::replace(&mut self.mode, Mode::Meta);
        let in_value = std::mem::replace(&mut self.in_value, true);
        let mut rendered = Vec::new();
        self.objects(&tree, &mut rendered);
        self.mode = mode;
        self.in_value = in_value;
        let mut out = Vec::new();
        flatten_value(rendered, &mut out);
        trim_text(&mut out);
        out
    }
}

/// The entry text `{{{property}}}` reads at node `n` (see [`entry_text`]),
/// and where its property drawer is (else the node).
fn node_entry(root: &SyntaxNode, n: &SemanticNode) -> (String, Span) {
    let at = match n.origin {
        NodeOrigin::File => None,
        NodeOrigin::Headline => node_at(root, n.span, HEADLINE),
        NodeOrigin::Item => node_at(root, n.span, ITEM),
    };
    let inside = at.unwrap_or_else(|| root.clone());
    let drawer = entry_drawer(&inside).map_or(n.span, |d| Span::of(&d));
    (entry_text(&inside), drawer)
}

/// The property drawer of the entry holding `inside` (its headline's, or
/// the file's before the first).
fn entry_drawer(inside: &SyntaxNode) -> Option<SyntaxNode> {
    let headline = inside.ancestors().find(|a| a.kind() == HEADLINE);
    match headline.and_then(crate::Headline::cast) {
        Some(h) => h.property_drawer().map(|d| d.syntax().clone()),
        None => {
            let root = inside.ancestors().last()?;
            let zeroth = root.children().next().filter(|s| s.kind() == SECTION)?;
            zeroth.children().find(|c| c.kind() == PROPERTY_DRAWER)
        }
    }
}

/// What `{{{property(KEY)}}}` reads at the entry holding `inside`: its
/// headline's first line (for `ITEM` and `TODO`) and its property drawer,
/// as text a value's paragraph can follow, line endings `\n`.
pub(crate) fn entry_text(inside: &SyntaxNode) -> String {
    let mut out = String::new();
    if let Some(h) = inside.ancestors().find(|a| a.kind() == HEADLINE) {
        let t = h.to_string();
        out.push_str(t.split(['\n', '\r']).next().unwrap_or(""));
        out.push('\n');
    }
    if let Some(d) = entry_drawer(inside) {
        let t = d.to_string().replace("\r\n", "\n").replace('\r', "\n");
        out.push_str(t.trim_end_matches('\n'));
        out.push('\n');
    }
    out
}

impl Builder<'_> {
    /// A property value as `{{{property(KEY)}}}` renders it at its entry
    /// (`entry`, see [`entry_text`]): Org shows a property value only
    /// through that macro, whose expansion is scanned again where it
    /// stands, so the value's macros expand with the document's templates
    /// (then babel's `results`), and what they open or close hides as it
    /// would there. A value whose macros cannot be expanded is withheld
    /// (nothing), with a [`RenderDiagnostic::PropertyWithheld`] at
    /// `drawer`. A value with no macro, or a document shown as written,
    /// is rendered as it is.
    pub fn macro_value(&mut self, entry: &str, value: &str, drawer: Span) -> Vec<Node> {
        // The context is built on the first value with a macro, if any.
        if !value.contains("{{{") {
            return self.property_value(value);
        }
        let expanded = match self.value_cx() {
            Some(vcx) => {
                let text = format!("{entry}x {value}\n");
                match self.expand_value(vcx, &text) {
                    Ok(t) => t,
                    Err(_) => {
                        self.diagnostics
                            .push(RenderDiagnostic::PropertyWithheld(drawer));
                        return Vec::new();
                    }
                }
            }
            _ => return self.property_value(value),
        };
        let line = expanded.lines().last().unwrap_or("");
        self.property_value(line.strip_prefix("x ").unwrap_or(line))
    }

    /// `text` with its macros expanded twice, as the export does: the
    /// templates, then the `results` babel keeps (in a value, a results
    /// macro an inline src block or call owns is removed).
    fn expand_value(
        &self,
        vcx: &super::expand::ValueCx,
        text: &str,
    ) -> Result<String, crate::MacroFailure> {
        let (text, map) = match vcx.expand(text)? {
            Some(e) => (e.text, e.map),
            None => (text.to_string(), Default::default()),
        };
        if !text.contains("{{{") {
            return Ok(text);
        }
        let d = crate::parse(&text, vcx.options());
        let removed = d
            .syntax()
            .descendants()
            .filter(|n| n.kind() == MACRO && super::babel::results_macro_owner(n).is_some())
            .map(|n| usize::from(n.text_range().start()))
            .collect();
        Ok(match vcx.results(&d, map, removed)? {
            Some(e) => e.text,
            None => text,
        })
    }

    /// Whether `value`, read as objects at this builder's place, holds
    /// anything the renderer hides: an inline src block or results macro
    /// its `:exports` hides, a babel call, an export snippet, or a macro
    /// that renders nothing.
    pub fn value_hides(&self, value: &str) -> bool {
        let tree = crate::green::objects(value, PARAGRAPH);
        let mut objects = tree.descendants().skip(1);
        objects.any(|d| {
            matches!(d.kind(), INLINE_BABEL_CALL | EXPORT_SNIPPET | MACRO)
                || self.babel_hides_object(&d)
        })
    }
}

/// A property value that cannot hold any Org object, as
/// [`Builder::property_value`] renders it (its trimmed text); `None` when it
/// might hold one. Letters and digits of any script, spaces and a few
/// punctuation marks start no object (no emphasis, link, entity, snippet,
/// macro, timestamp, footnote or babel marker); a single `-` or `.` is safe,
/// but `--` and `...` are special strings. Most property values (ids, names,
/// dates written as words) take this path, which skips the object parser.
fn plain_value(value: &str) -> Option<Vec<Node>> {
    let safe = |c: char| c.is_alphanumeric() || " ,;!?'\"()#%&-.".contains(c);
    if !value.chars().all(safe) || value.contains("--") || value.contains("...") {
        return None;
    }
    let t = value.trim_matches(' ');
    Some(if t.is_empty() {
        Vec::new()
    } else {
        vec![super::tree::text(t)]
    })
}

/// A rendered property value as its text, but for the links: an `<a>`, an
/// `<img>` and an unresolved link's span stay as rendered; every other
/// element is replaced by its children.
fn flatten_value(nodes: Vec<Node>, out: &mut Vec<Node>) {
    use super::tree::Tag;
    for n in nodes {
        match n {
            Node::Element(e)
                if matches!(e.tag, Tag::A | Tag::Img)
                    || (e.tag == Tag::Span
                        && e.get(super::tree::Attr::Class) == Some("org-unresolved")) =>
            {
                push(out, Node::Element(e))
            }
            Node::Element(e) => flatten_value(e.children, out),
            t => push(out, t),
        }
    }
}

/// Replaces each class-less `<span>` (a link shown as plain text) by its
/// children, recursively, so an unlinked title is the link text itself.
fn unwrap_plain(nodes: &mut Vec<Node>) {
    let old = std::mem::take(nodes);
    for n in old {
        match n {
            Node::Element(mut e) => {
                unwrap_plain(&mut e.children);
                if e.tag == super::tree::Tag::Span && e.attrs.is_empty() {
                    e.children.into_iter().for_each(|c| push(nodes, c));
                } else {
                    push(nodes, Node::Element(e));
                }
            }
            t => push(nodes, t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn both(value: &str) -> (Option<Vec<Node>>, Vec<Node>) {
        let doc = crate::parse("", &crate::ParseOptions::default());
        let sem = Semantics::default();
        let (opts, res) = (RenderOptions::default(), Resolver::default());
        let mut b = Builder::new(&doc, &sem, &opts, &res, Mode::Meta, Table::default());
        let parsed = b.placeless(None, |b| b.property_value_parsed(value));
        (plain_value(value), parsed)
    }

    #[test]
    fn plain_values_render_as_the_object_parser_renders_them() {
        for v in [
            "n12",
            " a b ",
            "3f9c-1a2b",
            "Ünïcødé 中文",
            "a.b",
            "x, y; z!",
            "",
        ] {
            let (fast, parsed) = both(v);
            assert_eq!(fast, Some(parsed), "{v:?}");
        }
        for v in [
            "a--b",
            "a...",
            "*b*",
            "x_y",
            "[[l]]",
            "https:x",
            "\\alpha",
            "src_sh{x}",
            "a/b",
        ] {
            assert!(plain_value(v).is_none(), "{v:?} took the plain path");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4000))]
        /// Over the safe alphabet the plain path answers almost always; it
        /// must answer exactly what the parser does.
        #[test]
        fn safe_values_render_the_same_both_ways(
            v in "[a-zA-Z0-9 ,;!?'\"()#%&.\\-éß中]{0,24}"
        ) {
            let (fast, parsed) = both(&v);
            if let Some(fast) = fast {
                prop_assert_eq!(fast, parsed, "{:?}", v);
            } else {
                prop_assert!(v.contains("--") || v.contains("..."), "{:?}", v);
            }
        }

        /// Whenever the plain path answers, it answers what the parser does.
        #[test]
        fn the_plain_path_never_disagrees_with_the_parser(
            v in "[a-zA-Z0-9 ,;!?'\"()#%&.\\-éß中_*/=~+^<>\\[\\]{}@:$|]{0,24}"
        ) {
            let (fast, parsed) = both(&v);
            if let Some(fast) = fast {
                prop_assert_eq!(fast, parsed, "{:?}", v);
            }
        }
    }
}
