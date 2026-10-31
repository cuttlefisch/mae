//! MAE's canonical semantics: KB nodes, typed links and transclusions, as a
//! projection of the tree (ADR-112 D2, D6 layer 1).
//!
//! [`semantics`] is a pure function of the parsed text and declared
//! configuration: no user code, no I/O, so every peer and the daemon derive
//! the same nodes and edges from the same text. It reads ONLY rendered
//! regions (see `regions`); an id or link that occurs only in an unrendered
//! region yields nothing but a diagnostic.
//!
//! MAE's extensions to Org, each a documented delta:
//! - `:KIND:` and `:ALIASES:` (comma-separated) properties;
//! - file keywords `#+kind`, `#+aliases`, `#+todo_state`, `#+priority`,
//!   alongside Org's `#+title` and `#+filetags`; all are read from the
//!   zeroth section only (Org reads TITLE and FILETAGS buffer-wide), and a
//!   later keyword replaces an earlier one, except FILETAGS, which add up;
//! - list items with their own property drawer are nodes (see `items`);
//! - `#+TRANSCLUDE: MEMBER [ROLE]`;
//! - the link grammar of [`crate::classify_link`].
//!
//! What a node's fields mean, pinned by `tests/semantics.rs`:
//! - `span` is the whole element: the file, the headline's subtree, the item;
//! - `body_span` is the body alone, the write contract's unit: for a
//!   headline, its section after the planning line and property drawer, up to
//!   its first child headline (trailing blank lines included, as the section
//!   holds them); for an item, from after its property drawer to the item's
//!   end; for the file, the zeroth section after its leading comments,
//!   property drawer and keywords. An empty body is an empty span where the
//!   body would start;
//! - `tags` are the file tags then the node's own, each once, in order
//!   (inheritance from a parent headline is not applied: Org makes it
//!   configurable, so it would be declared data, not done here);
//! - `properties` are the node's property drawer with canonical keys
//!   ([`crate::Property::key`]), `ID` included, in first-occurrence order; a
//!   repeated key keeps its last value and `KEY+` appends to `KEY` with a
//!   space, as Org does;
//! - links belong to the innermost node around them (`from`), never also to
//!   that node's ancestors; `None` outside every node.

mod items;
mod plain;
mod regions;

use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::link_grammar::{classify_link, LinkKind};
use crate::span::Span;
use crate::views::{Headline, Keyword, PropertyDrawer};
use crate::Document;
use regions::Rules;
use rowan::WalkEvent;
use std::collections::HashMap;

/// Declared configuration the projection reads (ADR-112 D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticsOptions {
    /// Subtrees tagged with one of these are not rendered. An in-file
    /// `#+EXCLUDE_TAGS:` replaces them, as in Org. Default: `noexport`
    /// (`org-export-exclude-tags`).
    pub exclude_tags: Vec<String>,
}

impl Default for SemanticsOptions {
    fn default() -> Self {
        SemanticsOptions {
            exclude_tags: vec!["noexport".to_string()],
        }
    }
}

/// Which element a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeOrigin {
    File,
    Headline,
    Item,
}

/// A KB node: the file, a headline or a list item with an `:ID:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticNode {
    pub id: String,
    pub origin: NodeOrigin,
    /// 0 for the file; a headline's star count; for an item, its headline's
    /// level plus the number of items it is nested in, itself included.
    pub level: usize,
    /// `:KIND:` (or the file's `#+kind`), trimmed; `None` if absent or empty.
    pub kind: Option<String>,
    /// Plain text (see `plain`); `None` if empty or absent.
    pub title: Option<String>,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub properties: Vec<(String, String)>,
    pub todo: Option<String>,
    pub priority: Option<char>,
    pub span: Span,
    pub body_span: Span,
}

/// A link in a rendered region.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticLink {
    /// The innermost node around the link.
    pub from: Option<String>,
    pub kind: LinkKind,
    /// The path as written (`LINK_PATH`).
    pub path: String,
    pub target: String,
    pub fragment: Option<String>,
    pub rel: String,
    pub weight: f64,
    pub confidence: f64,
    pub attrs: Vec<(String, String)>,
    /// Plain text of the link's description, or the store form's.
    pub description: Option<String>,
    /// The link, without the blanks Org counts after it.
    pub span: Span,
}

/// A `#+TRANSCLUDE: MEMBER [ROLE]` line in a rendered region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transclusion {
    pub from: Option<String>,
    pub member: String,
    /// The rest of the line, trimmed; `content` when empty.
    pub role: String,
    pub span: Span,
}

/// A region the semantics do not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Region {
    CommentLine,
    CommentBlock,
    ExportBlock,
    Drawer,
    CommentSubtree,
    ExcludedSubtree,
    UnreferencedFootnote,
}

impl Region {
    /// The name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Region::CommentLine => "comment_line",
            Region::CommentBlock => "comment_block",
            Region::ExportBlock => "export_block",
            Region::Drawer => "drawer",
            Region::CommentSubtree => "comment_subtree",
            Region::ExcludedSubtree => "excluded_subtree",
            Region::UnreferencedFootnote => "unreferenced_footnote",
        }
    }
}

/// What the projection reports instead of deriving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticDiagnostic {
    /// An `:ID: value` line (possibly commented out with `#`) in an
    /// unrendered region: no node comes from it. `span` is the line.
    IdInUnrenderedRegion {
        id: String,
        span: Span,
        region: Region,
    },
    /// A link in an unrendered region: no edge comes from it.
    LinkInUnrenderedRegion {
        path: String,
        span: Span,
        region: Region,
    },
    /// A second element with an id already used in this document: it is not
    /// a node (its content belongs to the node around it). `span` is the
    /// repeated `:ID:` property, `first` the first node's span.
    DuplicateId { id: String, span: Span, first: Span },
}

impl SemanticDiagnostic {
    /// The name used in JSON.
    pub fn kind_name(&self) -> &'static str {
        match self {
            SemanticDiagnostic::IdInUnrenderedRegion { .. } => "id_in_unrendered_region",
            SemanticDiagnostic::LinkInUnrenderedRegion { .. } => "link_in_unrendered_region",
            SemanticDiagnostic::DuplicateId { .. } => "duplicate_id",
        }
    }

    pub fn span(&self) -> Span {
        match self {
            SemanticDiagnostic::IdInUnrenderedRegion { span, .. }
            | SemanticDiagnostic::LinkInUnrenderedRegion { span, .. }
            | SemanticDiagnostic::DuplicateId { span, .. } => *span,
        }
    }
}

/// Everything the projection derives, each list in document order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Semantics {
    pub nodes: Vec<SemanticNode>,
    pub links: Vec<SemanticLink>,
    pub transclusions: Vec<Transclusion>,
    /// Sorted by span, then kind.
    pub diagnostics: Vec<SemanticDiagnostic>,
}

/// [`semantics_with`] under the default options.
pub fn semantics(doc: &Document) -> Semantics {
    semantics_with(doc, &SemanticsOptions::default())
}

/// The KB nodes, links and transclusions of `doc`.
pub fn semantics_with(doc: &Document, options: &SemanticsOptions) -> Semantics {
    let root = doc.syntax();
    let exclude_tags = exclude_tags(doc, options);
    let referenced = regions::referenced_footnotes(&root, &exclude_tags);
    let header = FileHeader::read(&root);
    let mut w = Walk {
        rules: Rules {
            exclude_tags,
            referenced: Some(referenced),
        },
        file_tags: header.tags.clone(),
        file_id: None,
        owners: Vec::new(),
        seen: HashMap::new(),
        out: Semantics::default(),
    };
    w.file_node(&root, header);
    w.walk(&root);
    w.out.diagnostics.sort_by_key(|d| (d.span(), d.kind_name()));
    w.out
}

/// In-file `#+EXCLUDE_TAGS:` (all of them, words split on blanks) replace
/// the configured exclude tags.
fn exclude_tags(doc: &Document, options: &SemanticsOptions) -> Vec<String> {
    let declared: Vec<Keyword> = doc
        .keywords()
        .filter(|k| k.key().eq_ignore_ascii_case("EXCLUDE_TAGS"))
        .collect();
    if declared.is_empty() {
        return options.exclude_tags.clone();
    }
    declared
        .iter()
        .flat_map(|k| {
            k.value()
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The file node's fields, from the zeroth section.
#[derive(Default)]
struct FileHeader {
    properties: Vec<(String, String)>,
    title: Option<String>,
    tags: Vec<String>,
    kind: Option<String>,
    aliases: Option<Vec<String>>,
    todo: Option<String>,
    priority: Option<char>,
}

impl FileHeader {
    fn read(root: &SyntaxNode) -> FileHeader {
        let mut h = FileHeader::default();
        let Some(section) = root.children().next().filter(|n| n.kind() == SECTION) else {
            return h;
        };
        if let Some(d) = section.children().find_map(PropertyDrawer::cast) {
            h.properties = resolve_properties(property_pairs(&d));
        }
        for k in section.children().filter_map(Keyword::cast) {
            h.keyword(&k.key().to_ascii_lowercase(), &k.value());
        }
        h
    }

    fn keyword(&mut self, key: &str, value: &str) {
        let value = value.trim();
        let some = || Some(value.to_string()).filter(|v| !v.is_empty());
        match key {
            "title" => {
                let t = plain::plain_text(&crate::green::objects(value, PARAGRAPH));
                self.title = Some(t).filter(|t| !t.is_empty());
            }
            "filetags" => self.tags.extend(split_filetags(value)),
            "kind" => self.kind = some(),
            "aliases" => self.aliases = Some(split_commas(value)),
            "todo_state" => self.todo = some(),
            "priority" => self.priority = value.chars().next(),
            _ => {}
        }
    }
}

/// Org's FILETAGS reading: words split on blanks, each split on `:`.
fn split_filetags(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .flat_map(|w| w.split(':'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_commas(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn property_pairs(d: &PropertyDrawer) -> Vec<(String, String)> {
    d.properties()
        .into_iter()
        .map(|p| (p.key, p.value))
        .collect()
}

/// Org's reading of a drawer's lines: keys canonical, the last value of a
/// repeated key wins (in the first one's position), `KEY+` appends.
fn resolve_properties(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (key, value) in pairs {
        let key = crate::views::canonical_key(&key);
        let (base, append) = match key.strip_suffix('+') {
            Some(b) if !b.is_empty() => (b.to_string(), true),
            _ => (key, false),
        };
        match out.iter_mut().find(|(k, _)| *k == base) {
            Some((_, v)) if append => {
                v.push(' ');
                v.push_str(&value);
            }
            Some((_, v)) => *v = value,
            None => out.push((base, value)),
        }
    }
    out
}

fn property<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// Own tags after the file tags, each once.
fn merge_tags(file: &[String], own: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in file.iter().cloned().chain(own) {
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

struct Walk {
    rules: Rules,
    file_tags: Vec<String>,
    file_id: Option<String>,
    /// Enclosing headline/item nodes: (element, id).
    owners: Vec<(SyntaxNode, String)>,
    /// First span of every id given to a node.
    seen: HashMap<String, Span>,
    out: Semantics,
}

/// What every node has, before its origin-specific fields.
struct NodeCore {
    id: String,
    origin: NodeOrigin,
    level: usize,
    props: Vec<(String, String)>,
    span: Span,
    body_span: Span,
}

impl Walk {
    fn owner(&self) -> Option<String> {
        self.owners
            .last()
            .map(|o| o.1.clone())
            .or_else(|| self.file_id.clone())
    }

    fn walk(&mut self, root: &SyntaxNode) {
        let mut walk = root.preorder();
        while let Some(ev) = walk.next() {
            let n = match ev {
                WalkEvent::Enter(n) => n,
                WalkEvent::Leave(n) => {
                    if self.owners.last().is_some_and(|o| o.0 == n) {
                        self.owners.pop();
                    }
                    continue;
                }
            };
            if let Some(region) = self.rules.region(&n) {
                self.unrendered(&n, region);
                walk.skip_subtree();
                continue;
            }
            match n.kind() {
                HEADLINE => self.headline(&n),
                ITEM => self.item(&n),
                LINK => self.link(&n),
                KEYWORD => self.keyword(&n),
                _ => {}
            }
            if matches!(n.kind(), LINK | PROPERTY_DRAWER | DRAWER) {
                // A link's parts are not links; a property drawer (or an
                // item's, the only drawer that reaches here) holds values.
                walk.skip_subtree();
            }
        }
    }

    /// Claims `id` for a node at `span`; false (and a diagnostic) if taken.
    fn claim(&mut self, id: &str, id_span: Span, span: Span) -> bool {
        if let Some(first) = self.seen.get(id) {
            self.out.diagnostics.push(SemanticDiagnostic::DuplicateId {
                id: id.to_string(),
                span: id_span,
                first: *first,
            });
            return false;
        }
        self.seen.insert(id.to_string(), span);
        true
    }

    fn file_node(&mut self, root: &SyntaxNode, h: FileHeader) {
        let Some(id) = property(&h.properties, "ID").map(str::to_string) else {
            return;
        };
        let span = Span::of(root);
        self.seen.insert(id.clone(), span);
        self.file_id = Some(id.clone());
        let kind = h
            .kind
            .or_else(|| property(&h.properties, "KIND").map(str::to_string));
        let aliases = h.aliases.unwrap_or_else(|| {
            property(&h.properties, "ALIASES").map_or_else(Vec::new, split_commas)
        });
        self.out.nodes.push(SemanticNode {
            id,
            origin: NodeOrigin::File,
            level: 0,
            kind,
            title: h.title,
            aliases,
            tags: merge_tags(&h.tags, []),
            properties: h.properties,
            todo: h.todo,
            priority: h.priority,
            span,
            body_span: file_body(root),
        });
    }

    fn headline(&mut self, n: &SyntaxNode) {
        let Some(h) = Headline::cast(n.clone()) else {
            return;
        };
        let Some(drawer) = h.property_drawer() else {
            return;
        };
        let props = resolve_properties(property_pairs(&drawer));
        let Some(id) = property(&props, "ID").map(str::to_string) else {
            return;
        };
        let id_span = id_property_span(drawer.syntax()).unwrap_or(Span::of(drawer.syntax()));
        if !self.claim(&id, id_span, Span::of(n)) {
            return;
        }
        let title = n
            .children()
            .find(|c| c.kind() == HEADLINE_TITLE)
            .map(|t| plain::plain_text(&t))
            .filter(|t| !t.is_empty());
        let tags = merge_tags(&self.file_tags, h.tags());
        let core = NodeCore {
            id,
            origin: NodeOrigin::Headline,
            level: h.level(),
            props,
            span: Span::of(n),
            body_span: headline_body(n),
        };
        self.push_node(n, core, title, tags, (h.todo(), h.priority()));
    }

    fn item(&mut self, n: &SyntaxNode) {
        let Some((drawer, raw)) = n
            .children()
            .find_map(|c| items::item_property_drawer(&c).map(|p| (c, p)))
        else {
            return;
        };
        let props = resolve_properties(raw);
        let Some(id) = property(&props, "ID").map(str::to_string) else {
            return;
        };
        let id_span = id_line_span(&drawer).unwrap_or(Span::of(&drawer));
        if !self.claim(&id, id_span, Span::of(n)) {
            return;
        }
        let title = items::item_title(n, &drawer);
        let tags = merge_tags(&self.file_tags, []);
        let core = NodeCore {
            id,
            origin: NodeOrigin::Item,
            level: items::item_level(n),
            props,
            span: Span::of(n),
            body_span: items::item_body(n, &drawer),
        };
        self.push_node(n, core, title, tags, (None, None));
    }

    fn push_node(
        &mut self,
        element: &SyntaxNode,
        core: NodeCore,
        title: Option<String>,
        tags: Vec<String>,
        (todo, priority): (Option<String>, Option<char>),
    ) {
        let kind = property(&core.props, "KIND").map(str::to_string);
        let aliases = property(&core.props, "ALIASES").map_or_else(Vec::new, split_commas);
        self.owners.push((element.clone(), core.id.clone()));
        self.out.nodes.push(SemanticNode {
            id: core.id,
            origin: core.origin,
            level: core.level,
            kind,
            title,
            aliases,
            tags,
            properties: core.props,
            todo,
            priority,
            span: core.span,
            body_span: core.body_span,
        });
    }

    fn link(&mut self, n: &SyntaxNode) {
        let (Some(path), Some(buffer)) = (plain::link_path(n), plain::link_path_buffer(n)) else {
            return;
        };
        let c = classify_link(&buffer);
        let description = plain::description(n).or(c.description);
        self.out.links.push(SemanticLink {
            from: self.owner(),
            kind: c.kind,
            path,
            target: c.target,
            fragment: c.fragment,
            rel: c.rel,
            weight: c.weight,
            confidence: c.confidence,
            attrs: c.attrs,
            description,
            span: without_trailing_blanks(n),
        });
    }

    fn keyword(&mut self, n: &SyntaxNode) {
        let Some(k) = Keyword::cast(n.clone()) else {
            return;
        };
        if !k.key().eq_ignore_ascii_case("TRANSCLUDE") {
            return;
        }
        let value = k.value();
        let value = value.trim();
        let (member, role) = match value.split_once([' ', '\t']) {
            Some((m, r)) => (m, r.trim()),
            None => (value, ""),
        };
        if member.is_empty() {
            return;
        }
        let role = if role.is_empty() { "content" } else { role };
        self.out.transclusions.push(Transclusion {
            from: self.owner(),
            member: member.to_string(),
            role: role.to_string(),
            span: without_trailing_blanks(n),
        });
    }

    fn unrendered(&mut self, n: &SyntaxNode, region: Region) {
        let base = usize::from(n.text_range().start());
        for (id, span) in regions::id_lines(&n.to_string(), base) {
            let d = SemanticDiagnostic::IdInUnrenderedRegion { id, span, region };
            self.out.diagnostics.push(d);
        }
        for (path, span) in regions::region_links(n) {
            let d = SemanticDiagnostic::LinkInUnrenderedRegion { path, span, region };
            self.out.diagnostics.push(d);
        }
    }
}

/// The span of a property drawer's (last) `ID` property line.
fn id_property_span(drawer: &SyntaxNode) -> Option<Span> {
    PropertyDrawer::cast(drawer.clone())?
        .properties()
        .into_iter()
        .rev()
        .find(|p| p.key == "ID")
        .map(|p| without_trailing_blanks(&p.node))
}

/// The span of an item drawer's last `:ID:` line.
fn id_line_span(drawer: &SyntaxNode) -> Option<Span> {
    let base = usize::from(drawer.text_range().start());
    regions::id_lines(&drawer.to_string(), base)
        .pop()
        .map(|(_, s)| s)
}

/// `node`'s span without trailing blanks and line endings.
fn without_trailing_blanks(node: &SyntaxNode) -> Span {
    let s = Span::of(node);
    let text = node.to_string();
    let kept = text.trim_end_matches([' ', '\t', '\n', '\r']).len();
    Span::new(s.start as usize, s.start as usize + kept)
}

/// A headline's body: its section after planning and property drawer.
fn headline_body(n: &SyntaxNode) -> Span {
    let Some(section) = n.children().find(|c| c.kind() == SECTION) else {
        let at = n
            .children()
            .find(|c| c.kind() == HEADLINE)
            .map_or(n.text_range().end(), |c| c.text_range().start());
        return Span::new(usize::from(at), usize::from(at));
    };
    let head = section
        .children()
        .take_while(|c| matches!(c.kind(), PLANNING | PROPERTY_DRAWER))
        .last();
    after(&section, head.as_ref())
}

/// The file's body: the zeroth section after its leading comments,
/// property drawer and keywords.
fn file_body(root: &SyntaxNode) -> Span {
    let Some(first) = root.children().next() else {
        let end = usize::from(root.text_range().end());
        return Span::new(end, end);
    };
    if first.kind() != SECTION {
        let at = usize::from(first.text_range().start());
        return Span::new(at, at);
    }
    let head = first
        .children()
        .take_while(|c| matches!(c.kind(), COMMENT | PROPERTY_DRAWER | KEYWORD))
        .last();
    after(&first, head.as_ref())
}

/// From the end of `head` (or the start of `container`) to its end.
fn after(container: &SyntaxNode, head: Option<&SyntaxNode>) -> Span {
    let start = head.map_or(container.text_range().start(), |h| h.text_range().end());
    Span::new(
        usize::from(start),
        usize::from(container.text_range().end()),
    )
}
