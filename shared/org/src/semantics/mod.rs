//! MAE's canonical semantics: KB nodes, typed links and transclusions, as a
//! projection of the tree (ADR-112 D2, D6 layer 1).
//!
//! [`semantics`] is a pure function of the parsed text and declared
//! configuration ([`SemanticsOptions`], ADR-112 D4): no user code, no I/O,
//! so every peer and the daemon derive the same nodes and edges from the
//! same text. It reads ONLY rendered regions (see `regions`); an id, link or
//! transclusion that occurs only in an unrendered region yields nothing but
//! a diagnostic. A `#+SETUPFILE` is not read (no I/O): it is diagnosed,
//! since the file it names could declare TODO keywords and tags.
//!
//! MAE's extensions to Org, each a documented delta:
//! - `:KIND:` and `:ALIASES:` (comma-separated) properties;
//! - file keywords `#+kind`, `#+aliases`, `#+todo_state`, `#+priority`.
//!   Org has no reading of them, so their rule is MAE's: the zeroth section
//!   only, document order, the last wins, and an empty last value clears
//!   the field (`#+todo_state: TODO` then `#+todo_state:` gives no state).
//!   The file's property drawer always precedes its keywords (Org reads a
//!   drawer only at the top), so `#+kind` and `#+aliases` win over `:KIND:`
//!   and `:ALIASES:`. Org's own keywords follow Org (see `keywords`): the
//!   title is `org-get-title`'s, every `#+title` line joined; FILETAGS add
//!   up, read from the zeroth section only (Org reads them buffer-wide);
//! - list items with their own property drawer are nodes (see `items`);
//! - `#+TRANSCLUDE: MEMBER [ROLE]`, and org-transclusion's
//!   `#+TRANSCLUDE: [[LINK][DESC]] ARGS…`, whose member is the link's target;
//! - the link grammar of [`crate::classify_link_with`].
//!
//! What a node's fields mean, pinned by `tests/semantics.rs`:
//! - `span` is the whole element: the file, the headline's subtree, the item;
//! - `body_span` is the body alone, the write contract's unit (ADR-092). It
//!   starts after the node's front matter (a headline's line, planning line
//!   and property drawer; an item's first line and drawer; the file's
//!   leading comments, property drawer and keywords) and runs to the start
//!   of the FIRST NODE NESTED in the element, or to the element's end when
//!   none is. So the bodies of two nodes never overlap and a body never
//!   holds another node, its drawer or its `:ID:`: writing one node's body
//!   can never rewrite another node. A headline without an id, a duplicate
//!   id or an unrendered subtree is not a node, so its text is part of the
//!   body around it, exactly as its links are (the same rule for headlines
//!   and items). Text AFTER a nested node (a paragraph after a sub-item
//!   node, a heading after a child headline node) is in no node's body: the
//!   write path never touches it, so it is kept byte for byte, but it is not
//!   editable through any node. Its links and transclusions still belong to
//!   the innermost node around them. One contiguous span was chosen over a
//!   body with holes because a write is then one splice of one range; a
//!   body with holes would need the writer to decide which hole new text
//!   falls into. An empty body is an empty span where the body would start;
//! - a node's tags are the file tags then its own, each once, in order
//!   ([`Semantics::tags_of`]); inheritance from a parent headline is not
//!   applied (Org makes it configurable, so it would be declared data, not
//!   done here). The file tags are stored ONCE, in [`Semantics::file_tags`],
//!   and each node holds only its own tags not among them
//!   ([`SemanticNode::own_tags`]): N file tags and N nodes would otherwise
//!   be N² strings, a memory denial of service from a few hundred KB of
//!   text (`tests/scaling.rs`);
//! - `properties` are the node's property drawer with canonical keys
//!   ([`crate::Property::key`]), `ID` included, in first-occurrence order,
//!   each valued as `org-entry-get` reads it: the last `KEY` line, then
//!   every `KEY+` value in order, joined with a space, wherever they are; a
//!   value that is exactly `nil` is none, and a key with no value left is
//!   not listed. Values are trimmed as Org trims them (space, tab, LF, CR,
//!   never a no-break space), so the `ID` field and `properties` always
//!   agree;
//! - links belong to the innermost node around them (`from`), never also to
//!   that node's ancestors; `None` outside every node. A link in a property
//!   value belongs to the node whose drawer it is, and a link in a keyword
//!   value (`#+title`, `#+caption`, any other) to the node around the
//!   keyword: org-roam's defaults (`org-roam-db-extra-links-elements` is
//!   `(node-property keyword)`), except the values it excludes
//!   (`ROAM_REFS`, `TRANSCLUDE`) and the raw export keywords (`#+HTML:` and
//!   the like, whose value is output text, never Org), all declared in
//!   [`SemanticsOptions`]. A link inside an export snippet `@@html:…@@` is
//!   not an Org link at all;
//! - a link or transclusion inside a rendered footnote definition belongs
//!   to the node of the text that references the footnote (where Org's
//!   export puts the note), not to the node the definition happens to sit
//!   under: the first reference in rendered text, in document order, and
//!   for a definition referenced only from another definition, that
//!   definition's node. A node nested inside the definition still owns its
//!   own links. A label has ONE definition, as Org's export uses one: the
//!   last of those in rendered regions, standalone or inline
//!   (`[fn:n:text]`); failing that, the first in a pruned region. The
//!   others are unrendered ([`Region::OverriddenFootnote`]).

pub(crate) mod items;
mod keywords;
pub(crate) mod plain;
pub(crate) mod regions;

use crate::kind::SyntaxKind::{self, *};
use crate::kind::SyntaxNode;
use crate::link_grammar::{classify_link_with, classify_normalized, org_trim, LinkKind, LinkTypes};
use crate::span::Span;
use crate::views::{Headline, Keyword, LinkAbbrevs, LinkFormat, PropertyDrawer};
use crate::Document;
use regions::Rules;
use rowan::WalkEvent;
use std::collections::{HashMap, HashSet};

/// How an archived headline (one tagged with [`SemanticsOptions::archive_tag`])
/// is rendered: `org-export-with-archived-trees`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchivedTrees {
    /// The headline is rendered, its contents are not (Org's default,
    /// `'headline`).
    Headline,
    /// The whole subtree is rendered (`t`).
    All,
    /// Nothing of it is rendered (`nil`).
    None,
}

/// Declared configuration the projection reads (ADR-112 D4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SemanticsOptions {
    /// Subtrees tagged with one of these (or every subtree, when the file
    /// tags carry one) are not rendered. In-file `#+EXCLUDE_TAGS:` lines
    /// replace them, as in Org. Default: `noexport`
    /// (`org-export-exclude-tags`).
    pub exclude_tags: Vec<String>,
    /// When a headline (or the file tags) carries one of these, only the
    /// selected subtrees and their ancestors are rendered, and the zeroth
    /// section's content is not. In-file `#+SELECT_TAGS:` lines replace
    /// them.
    ///
    /// Default: **none**, deliberately unlike Org's `org-export-select-tags`
    /// (`export`). Selection is an export-time choice made by whoever runs
    /// the export; in a KB, a note tagged `export` because it is *about*
    /// exporting would otherwise vanish from the graph, zeroth section and
    /// all (a bundled DevPractices note did). A KB or file that wants Org's
    /// selection declares it: here, or in-file with `#+SELECT_TAGS:`.
    /// [`ORG_EXPORT_SELECT_TAGS`] is Org's default, for an exporter.
    pub select_tags: Vec<String>,
    /// The tag that archives a headline, matched exactly
    /// (`org-archive-tag`). Default: `ARCHIVE`.
    pub archive_tag: String,
    /// Default: [`ArchivedTrees::Headline`].
    pub archived_trees: ArchivedTrees,
    /// Which link schemes are not KB links. Default: [`LinkTypes::default`].
    pub link_types: LinkTypes,
    /// Keywords (any case) whose values are not read for links: org-roam's
    /// `TRANSCLUDE` and the raw export keywords. Default:
    /// [`LINK_EXCLUDE_KEYWORDS`].
    pub link_exclude_keywords: Vec<String>,
    /// Properties (any case) whose values are not read for links. Default:
    /// org-roam's `ROAM_REFS`.
    pub link_exclude_properties: Vec<String>,
}

/// Org's default `org-export-select-tags`, for a caller that wants export
/// selection exactly as `emacs -Q` does it (see
/// [`SemanticsOptions::select_tags`] for why it is not the default).
pub const ORG_EXPORT_SELECT_TAGS: &[&str] = &["export"];

/// Keywords whose values are not link sources: `TRANSCLUDE` (org-roam's
/// `org-roam-db-extra-links-exclude-keys`), `LINK` (an abbreviation's
/// template), and the keywords whose value Org passes to an exporter as raw
/// output, which is never parsed as Org.
pub const LINK_EXCLUDE_KEYWORDS: &[&str] = &[
    "TRANSCLUDE",
    // A `#+LINK:` value is an abbreviation's template, not a link.
    "LINK",
    "ASCII",
    "BEAMER",
    "HTML",
    "HTML_HEAD",
    "HTML_HEAD_EXTRA",
    "LATEX",
    "LATEX_HEADER",
    "LATEX_HEADER_EXTRA",
    "ODT",
    "TEXINFO",
];

impl Default for SemanticsOptions {
    fn default() -> Self {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        SemanticsOptions {
            exclude_tags: own(&["noexport"]),
            select_tags: Vec::new(),
            archive_tag: "ARCHIVE".to_string(),
            archived_trees: ArchivedTrees::Headline,
            link_types: LinkTypes::default(),
            link_exclude_keywords: own(LINK_EXCLUDE_KEYWORDS),
            link_exclude_properties: own(&["ROAM_REFS"]),
        }
    }
}

/// Which element a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum NodeOrigin {
    File,
    Headline,
    Item,
}

/// A KB node: the file, a headline or a list item with an `:ID:`.
///
/// **Not display text.** `title`, `aliases` and `properties` are the
/// node's SOURCE read as plain text: an inline src block, a babel call or
/// an export snippet the export hides is spelled out in them, as written
/// (`Notes src_sh[:exports none]{cat key}`). That is what a KB index needs;
/// a page, a list or a search result must show [`crate::NodeMeta`]'s
/// `title`, `title_unlinked`, `aliases` and `properties` ([`crate::render`])
/// or [`crate::node_text`] instead, which hide what the export hides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The node's own tags (a headline's), each once, without any of the
    /// file's tags: [`Semantics::tags_of`] gives the full list.
    pub own_tags: Vec<String>,
    pub properties: Vec<(String, String)>,
    pub todo: Option<String>,
    pub priority: Option<char>,
    pub span: Span,
    pub body_span: Span,
}

/// A link in a rendered region.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SemanticLink {
    /// The node the link belongs to (see the module doc).
    pub from: Option<String>,
    pub kind: LinkKind,
    /// The path as written (`LINK_PATH`).
    pub path: String,
    pub target: String,
    pub fragment: Option<String>,
    /// The `::search` option of an `id:` or file link.
    pub search: Option<String>,
    pub rel: String,
    pub weight: f64,
    pub confidence: f64,
    pub attrs: Vec<(String, String)>,
    /// Plain text of the link's description, or the store form's: SOURCE,
    /// not display text (a hidden construct in it is spelled out; see
    /// [`SemanticNode`]). A renderer shows the description as rendered.
    pub description: Option<String>,
    /// The link, without the blanks Org counts after it.
    pub span: Span,
}

/// A `#+TRANSCLUDE:` line in a rendered region.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Transclusion {
    pub from: Option<String>,
    /// MAE's `MEMBER`, or the target of org-transclusion's link (the id of
    /// `[[id:x][X]]`).
    pub member: String,
    /// MAE's `ROLE`: the rest of the line, trimmed; `content` when empty, and
    /// always for org-transclusion's link form.
    pub role: String,
    /// org-transclusion's properties after the link (`:level 2`), trimmed;
    /// `None` for MAE's form or when there are none.
    pub args: Option<String>,
    pub span: Span,
}

/// A region the semantics do not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Region {
    CommentLine,
    CommentBlock,
    ExportBlock,
    Drawer,
    CommentSubtree,
    ExcludedSubtree,
    UnreferencedFootnote,
    /// A footnote definition Org's export does not use: it uses ONE per
    /// label (the last of those rendered, standalone or inline; failing
    /// that, the first in a pruned region), so the others are not rendered.
    OverriddenFootnote,
    /// Outside every selected tree, or the zeroth section's content, while a
    /// select tag is in force.
    Unselected,
    /// The contents of an archived headline (or, under
    /// [`ArchivedTrees::None`], the whole subtree).
    Archived,
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
            Region::OverriddenFootnote => "overridden_footnote",
            Region::Unselected => "unselected",
            Region::Archived => "archived",
        }
    }
}

/// What the projection reports instead of deriving.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// A `#+TRANSCLUDE:` in an unrendered region: no transclusion comes from
    /// it. `member` is what the member would have been.
    TransclusionInUnrenderedRegion {
        member: String,
        span: Span,
        region: Region,
    },
    /// A second element with an id already used in this document: it is not
    /// a node (its content belongs to the node around it). `span` is the
    /// repeated `:ID:` property, `first` the first node's span.
    DuplicateId { id: String, span: Span, first: Span },
    /// A `#+SETUPFILE:` keyword. The file it names can declare TODO keywords,
    /// tags and options that would change this document's semantics; it was
    /// not read (no I/O), so the semantics are those of this text alone.
    SetupFileNotRead { file: String, span: Span },
}

impl SemanticDiagnostic {
    /// The name used in JSON.
    pub fn kind_name(&self) -> &'static str {
        match self {
            SemanticDiagnostic::IdInUnrenderedRegion { .. } => "id_in_unrendered_region",
            SemanticDiagnostic::LinkInUnrenderedRegion { .. } => "link_in_unrendered_region",
            SemanticDiagnostic::TransclusionInUnrenderedRegion { .. } => {
                "transclusion_in_unrendered_region"
            }
            SemanticDiagnostic::DuplicateId { .. } => "duplicate_id",
            SemanticDiagnostic::SetupFileNotRead { .. } => "setup_file_not_read",
        }
    }

    pub fn span(&self) -> Span {
        match self {
            SemanticDiagnostic::IdInUnrenderedRegion { span, .. }
            | SemanticDiagnostic::LinkInUnrenderedRegion { span, .. }
            | SemanticDiagnostic::TransclusionInUnrenderedRegion { span, .. }
            | SemanticDiagnostic::DuplicateId { span, .. }
            | SemanticDiagnostic::SetupFileNotRead { span, .. } => *span,
        }
    }
}

/// An element the projection does not read, so no renderer may draw: the
/// decision renderers reuse (ADR-112 D2/D3) rather than re-derive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Unrendered {
    /// The element's span; with `element` it identifies one node of the tree.
    pub span: Span,
    /// The element's kind (a `HEADLINE`, `COMMENT_BLOCK`, …).
    pub element: SyntaxKind,
    pub region: Region,
}

/// A footnote definition element in a rendered region: one that rendered
/// text references (Org renders it in the footnotes section, wherever it
/// sits, unless a region Org never parses holds it).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RenderedFootnote {
    pub label: String,
    /// The `FOOTNOTE_DEFINITION` element.
    pub span: Span,
}

/// Everything the projection derives, each list in document order.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Semantics {
    /// The file's tags (`#+filetags`), each once, in order: every node's.
    pub file_tags: Vec<String>,
    pub nodes: Vec<SemanticNode>,
    pub links: Vec<SemanticLink>,
    pub transclusions: Vec<Transclusion>,
    /// Sorted by span, then kind.
    pub diagnostics: Vec<SemanticDiagnostic>,
    /// Every element the walk found unrendered, outermost first: what it
    /// skipped, so a renderer skips exactly the same (sorted by span start,
    /// then by the longer span first). Nested entries occur only inside a
    /// footnote definition Org re-adds from a pruned region.
    pub unrendered: Vec<Unrendered>,
    /// Every rendered footnote definition element, in document order.
    pub footnotes: Vec<RenderedFootnote>,
}

impl Semantics {
    /// A node's tags: the file tags, then its own.
    pub fn tags_of(&self, node: &SemanticNode) -> Vec<String> {
        let mut out = self.file_tags.clone();
        out.extend(node.own_tags.iter().cloned());
        out
    }
}

/// [`semantics_with`] under the default options.
pub fn semantics(doc: &Document) -> Semantics {
    semantics_with(doc, &SemanticsOptions::default())
}

/// The KB nodes, links and transclusions of `doc`.
pub fn semantics_with(doc: &Document, options: &SemanticsOptions) -> Semantics {
    let root = doc.syntax();
    // Settings as Org's export reads them: not from COMMENT subtrees.
    let settings = keywords::outside_comment_trees(&root);
    let groups = keywords::TagGroups::new(&settings);
    let declared = |key: &str, configured: &[String]| {
        groups.expand(&keywords::declared_tags(&settings, key, configured))
    };
    let exclude = declared("EXCLUDE_TAGS", &options.exclude_tags);
    let select = declared("SELECT_TAGS", &options.select_tags);
    let mut header = FileHeader::read(&root);
    header.title = keywords::title(doc)
        .map(|t| plain::plain_text(&crate::green::objects(&t, PARAGRAPH)))
        .filter(|t| !t.is_empty());
    let mut rules = Rules::new(&root, options, &exclude, &select, &header.tags);
    rules.chosen = regions::choose_definitions(&root, &rules);
    let (referenced, order) = regions::referenced_footnotes(&root, &rules);
    rules.referenced = Some(referenced);
    let mut w = Walk {
        rules,
        link_types: &options.link_types,
        abbrevs: keywords::export_link_abbrevs(&root, &settings),
        file_tags: merge_tags(&header.tags, &HashSet::new()),
        file_tag_set: HashSet::new(),
        file_id: None,
        owners: Vec::new(),
        seen: HashMap::new(),
        note_ctx: HashMap::new(),
        note_place: HashMap::new(),
        pending_links: Vec::new(),
        pending_transclusions: Vec::new(),
        out: Semantics::default(),
    };
    w.out.file_tags = w.file_tags.clone();
    w.file_tag_set = w.file_tags.iter().cloned().collect();
    w.file_node(&root, header);
    w.walk(&root);
    w.setup_files(doc);
    w.attribute_notes(&order);
    let mut out = w.out;
    clip_bodies(&mut out.nodes);
    out.diagnostics.sort_by_key(|d| (d.span(), d.kind_name()));
    out.unrendered
        .sort_by_key(|u| (u.span.start, std::cmp::Reverse(u.span.end), u.element));
    out
}

/// The file node's fields, from the zeroth section.
#[derive(Default)]
struct FileHeader {
    properties: Vec<(String, String)>,
    title: Option<String>,
    /// The last `#+title` value and its offset (`None` when it is empty):
    /// the source a renderer draws the title from.
    title_source: Option<(String, usize)>,
    tags: Vec<String>,
    /// `Some` once a `#+kind` keyword is read, even an empty one.
    kind: Option<Option<String>>,
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
            let key = k.key().to_ascii_lowercase();
            h.keyword(&key, &k.value());
            if key == "title" {
                h.title_source = value_token(k.syntax()).filter(|(v, _)| !org_trim(v).is_empty());
            }
        }
        h
    }

    /// One keyword, in document order: MAE's own keywords follow MAE's rule
    /// (the last wins, an empty value clears); the title is not read here
    /// (`keywords::title`, Org's rule).
    fn keyword(&mut self, key: &str, value: &str) {
        let value = org_trim(value);
        let some = || Some(value.to_string()).filter(|v| !v.is_empty());
        match key {
            "filetags" => self.tags.extend(split_filetags(value)),
            "kind" => self.kind = Some(some()),
            "aliases" => self.aliases = Some(split_commas(value)),
            "todo_state" => self.todo = some(),
            "priority" => self.priority = value.chars().next(),
            _ => {}
        }
    }
}

/// The file title's source: the last `#+title` value of the zeroth
/// section and its offset, `None` if absent or empty. The same reading as
/// the file node's title (one implementation, principle #8).
pub(crate) fn file_title_source(root: &SyntaxNode) -> Option<(String, usize)> {
    FileHeader::read(root).title_source
}

/// Org's FILETAGS reading: words split on blanks, each split on `:`.
fn split_filetags(value: &str) -> Vec<String> {
    keywords::split_words(value)
        .iter()
        .flat_map(|w| w.split(':'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn split_commas(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(org_trim)
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

/// `org-entry-get`'s reading of a drawer's lines, for every key: keys
/// canonical, in first-occurrence order; the value is the LAST `KEY` line
/// (`plist-put` replaces), then every `KEY+` value in order wherever it
/// is (so `:K+: b` before `:K: a` gives `a b`), joined with a space; a
/// value that is exactly `nil` is none (`org-not-nil`), and a key left with
/// none is dropped. Linear: each key is found through a map.
fn resolve_properties(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut keys: Vec<(String, Option<String>, Vec<String>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (key, value) in pairs {
        let key = crate::views::canonical_key(&key);
        let (base, append) = match key.strip_suffix('+') {
            Some(b) if !b.is_empty() => (b.to_string(), true),
            _ => (key, false),
        };
        let i = *at.entry(base.clone()).or_insert_with(|| {
            keys.push((base, None, Vec::new()));
            keys.len() - 1
        });
        if append {
            keys[i].2.push(value);
        } else {
            keys[i].1 = Some(value);
        }
    }
    keys.into_iter()
        .filter_map(|(key, base, extra)| {
            let values: Vec<String> = base
                .into_iter()
                .chain(extra)
                .filter(|v| v != "nil")
                .collect();
            (!values.is_empty()).then(|| (key, values.join(" ")))
        })
        .collect()
}

fn property<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| org_trim(v))
        .filter(|v| !v.is_empty())
}

/// `tags` without empty ones, repeats or any in `skip`, in order. Linear in
/// `tags` alone: `skip` is a set built once, so N file tags and N headlines
/// cost O(N), not one set of N per headline.
fn merge_tags(tags: &[String], skip: &HashSet<String>) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        if !t.is_empty() && !skip.contains(t) && seen.insert(t.as_str()) {
            out.push(t.clone());
        }
    }
    out
}

/// Whom text belongs to while it is walked.
#[derive(Debug, Clone)]
enum Owner {
    /// A node.
    Node(String),
    /// The node of the text that references footnote `LABEL`, known once
    /// the walk is done.
    Note(String),
}

struct Walk<'o> {
    rules: Rules,
    link_types: &'o LinkTypes,
    /// The `#+LINK:` abbreviations Org's export applies.
    abbrevs: LinkAbbrevs,
    file_tags: Vec<String>,
    /// The same, as a set, built once.
    file_tag_set: HashSet<String>,
    file_id: Option<String>,
    /// Enclosing headline/item nodes and rendered footnote definitions.
    owners: Vec<(SyntaxNode, Owner)>,
    /// First span of every id given to a node.
    seen: HashMap<String, Span>,
    /// For each referenced footnote label, the owner at the reference that
    /// made it referenced.
    note_ctx: HashMap<String, Option<Owner>>,
    /// For each label, the owner around its (first) definition: used only
    /// if its reference was never walked.
    note_place: HashMap<String, Option<Owner>>,
    /// Links and transclusions whose owner is a footnote's, by index.
    pending_links: Vec<(usize, String)>,
    pending_transclusions: Vec<(usize, String)>,
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

impl Walk<'_> {
    fn owner(&self) -> Option<Owner> {
        self.owners
            .last()
            .map(|o| o.1.clone())
            .or_else(|| self.file_id.clone().map(Owner::Node))
    }

    /// The owner as a node id, or the footnote label to resolve later.
    fn from(&self) -> (Option<String>, Option<String>) {
        match self.owner() {
            Some(Owner::Node(id)) => (Some(id), None),
            Some(Owner::Note(label)) => (None, Some(label)),
            None => (None, None),
        }
    }

    fn walk(&mut self, start: &SyntaxNode) {
        let mut walk = start.preorder();
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
            if n.kind() == FOOTNOTE_REFERENCE {
                // A reference, even one whose inline definition Org drops.
                self.note_reference(&n);
            }
            if let Some(region) = self.rules.region(&n) {
                self.out.unrendered.push(Unrendered {
                    span: Span::of(&n),
                    element: n.kind(),
                    region,
                });
                self.unrendered(&n, region);
                walk.skip_subtree();
                continue;
            }
            match n.kind() {
                HEADLINE => self.headline(&n),
                ITEM => self.item(&n),
                LINK => self.link(&n, 0),
                KEYWORD => self.keyword(&n),
                PROPERTY_DRAWER => self.property_drawer_links(&n),
                DRAWER => self.item_drawer(&n),
                FOOTNOTE_DEFINITION => {
                    if let Some(label) = regions::definition_label(&n) {
                        self.enter_note(&n, label);
                    }
                }
                // An inline definition Org uses: its text is the note's.
                FOOTNOTE_REFERENCE => {
                    if let Some(label) = regions::inline_label(&n) {
                        self.enter_note(&n, label);
                    }
                }
                _ => {}
            }
            if matches!(n.kind(), LINK | PROPERTY_DRAWER | DRAWER) {
                // A link's parts are not links; a drawer's values were read
                // as values above (an item's drawer is the only drawer that
                // reaches here).
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
        // A keyword follows the drawer, so it wins, even when empty.
        let kind = h
            .kind
            .unwrap_or_else(|| property(&h.properties, "KIND").map(str::to_string));
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
            own_tags: Vec::new(),
            properties: h.properties,
            todo: h.todo,
            priority: h.priority,
            span,
            body_span: file_body(root),
        });
    }

    fn headline(&mut self, n: &SyntaxNode) {
        // An archived headline is rendered, its drawer is not: no node.
        if self.rules.contents_hidden(n) {
            return;
        }
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
        let tags = merge_tags(&h.tags(), &self.file_tag_set);
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
        let Some((drawer, raw)) = items::item_drawer(n) else {
            return;
        };
        let props = resolve_properties(raw.into_iter().map(|p| (p.key, p.value)).collect());
        let Some(id) = property(&props, "ID").map(str::to_string) else {
            return;
        };
        let id_span = id_line_span(&drawer).unwrap_or(Span::of(&drawer));
        if !self.claim(&id, id_span, Span::of(n)) {
            return;
        }
        let title = items::item_title(n, &drawer);
        let tags = Vec::new();
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
        self.owners
            .push((element.clone(), Owner::Node(core.id.clone())));
        self.out.nodes.push(SemanticNode {
            id: core.id,
            origin: core.origin,
            level: core.level,
            kind,
            title,
            aliases,
            own_tags: tags,
            properties: core.props,
            todo,
            priority,
            span: core.span,
            body_span: core.body_span,
        });
    }

    /// A LINK object; `base` is the offset of the tree it is in (non-zero
    /// for a link in a keyword or property value, parsed on its own).
    fn link(&mut self, n: &SyntaxNode, base: usize) {
        let (Some(path), Some(buffer)) = (plain::link_path(n), plain::link_path_buffer(n)) else {
            return;
        };
        let bracket =
            crate::views::Link::cast(n.clone()).is_some_and(|l| l.format() == LinkFormat::Bracket);
        let c = if bracket {
            // As Org makes a bracket link's raw link (the view's function).
            classify_normalized(&self.abbrevs.bracket_path(&buffer), self.link_types)
        } else {
            classify_link_with(&buffer, self.link_types)
        };
        let description = plain::description(n).or(c.description);
        let span = object_span(n);
        let (from, note) = self.from();
        if let Some(label) = note {
            self.pending_links.push((self.out.links.len(), label));
        }
        self.out.links.push(SemanticLink {
            from,
            kind: c.kind,
            path,
            target: c.target,
            fragment: c.fragment,
            search: c.search,
            rel: c.rel,
            weight: c.weight,
            confidence: c.confidence,
            attrs: c.attrs,
            description,
            span: Span::new(base + span.start as usize, base + span.end as usize),
        });
    }

    /// The links of a keyword or property value, read as Org reads a
    /// secondary string (its objects), at `at` in the document.
    fn value_links(&mut self, value: &str, at: usize) {
        let tree = crate::green::objects(value, PARAGRAPH);
        for l in tree.descendants().filter(|n| n.kind() == LINK) {
            self.link(&l, at);
        }
    }

    fn keyword(&mut self, n: &SyntaxNode) {
        if let Some((text, at)) = keyword_link_value(n, &self.rules) {
            self.value_links(&text, at);
        }
        let Some(k) = Keyword::cast(n.clone()) else {
            return;
        };
        if !k.key().eq_ignore_ascii_case("TRANSCLUDE") {
            return;
        }
        let Some((member, role, args)) = transclusion_parts(&k.value(), self.link_types) else {
            return;
        };
        let (from, note) = self.from();
        if let Some(label) = note {
            self.pending_transclusions
                .push((self.out.transclusions.len(), label));
        }
        self.out.transclusions.push(Transclusion {
            from,
            member,
            role,
            args,
            span: without_trailing_blanks(n),
        });
    }

    /// Links in the values of a headline's or the file's property drawer.
    fn property_drawer_links(&mut self, drawer: &SyntaxNode) {
        for p in drawer.children().filter(|c| c.kind() == NODE_PROPERTY) {
            if let Some((text, at)) = property_link_value(&p, &self.rules) {
                self.value_links(&text, at);
            }
        }
    }

    /// An item's property drawer: the links in its values, and the
    /// footnotes it references (Org renders that drawer, so its references
    /// count, and a note it references first belongs to the item).
    fn item_drawer(&mut self, drawer: &SyntaxNode) {
        self.item_drawer_links(drawer);
        for r in drawer
            .descendants()
            .filter(|d| d.kind() == FOOTNOTE_REFERENCE)
        {
            self.note_reference(&r);
        }
    }

    /// Links in the values of an item's property drawer.
    fn item_drawer_links(&mut self, drawer: &SyntaxNode) {
        for p in items::item_property_drawer(drawer).unwrap_or_default() {
            let key = crate::views::canonical_key(&p.key);
            let key = key.strip_suffix('+').unwrap_or(&key);
            if !self.rules.link_exclude_properties.contains(key) {
                self.value_links(&p.value, p.value_start);
            }
        }
    }

    /// A rendered footnote definition (standalone, or an inline one Org
    /// uses): what it holds belongs to the node of the text referencing it,
    /// resolved once the walk is done.
    fn enter_note(&mut self, n: &SyntaxNode, label: String) {
        let place = self.owner();
        self.out.footnotes.push(RenderedFootnote {
            label: label.clone(),
            span: Span::of(n),
        });
        self.note_place.entry(label.clone()).or_insert(place);
        self.owners.push((n.clone(), Owner::Note(label)));
    }

    /// The reference that made a footnote referenced records its owner.
    fn note_reference(&mut self, n: &SyntaxNode) {
        let Some(label) = regions::reference_label(n) else {
            return;
        };
        let first = self
            .rules
            .referenced
            .as_ref()
            .and_then(|m| m.get(&label))
            .copied();
        if first == Some(u32::from(n.text_range().start())) {
            let owner = self.owner();
            self.note_ctx.entry(label).or_insert(owner);
        }
    }

    /// Resolves the owners of footnote contents. `order` is the order the
    /// labels became referenced, so a label referenced from inside another
    /// definition comes after that definition's label: one pass, no chains.
    fn attribute_notes(&mut self, order: &[String]) {
        let mut resolved: HashMap<&str, Option<String>> = HashMap::new();
        for label in order {
            let ctx = self
                .note_ctx
                .get(label)
                .or_else(|| self.note_place.get(label))
                .cloned()
                .flatten();
            let id = match ctx {
                Some(Owner::Node(id)) => Some(id),
                Some(Owner::Note(l)) => resolved.get(l.as_str()).cloned().flatten(),
                None => None,
            };
            resolved.insert(label, id);
        }
        for (i, label) in std::mem::take(&mut self.pending_links) {
            self.out.links[i].from = resolved.get(label.as_str()).cloned().flatten();
        }
        for (i, label) in std::mem::take(&mut self.pending_transclusions) {
            self.out.transclusions[i].from = resolved.get(label.as_str()).cloned().flatten();
        }
    }

    fn unrendered(&mut self, n: &SyntaxNode, region: Region) {
        // Footnote definitions Org re-adds from this region are rendered:
        // walk them, and report only what lies outside them.
        let kept = if region.parses_contents() {
            regions::rendered_definitions(n, &self.rules)
        } else {
            Vec::new()
        };
        // Whether a position is in no kept definition: the kept ranges
        // sorted by start, with the greatest end so far, so each check is a
        // binary search, not a scan of every kept definition.
        let mut ranges: Vec<(usize, usize)> = kept
            .iter()
            .map(|d| {
                (
                    usize::from(d.text_range().start()),
                    usize::from(d.text_range().end()),
                )
            })
            .collect();
        ranges.sort_unstable();
        let mut reach = Vec::with_capacity(ranges.len());
        for &(_, b) in &ranges {
            reach.push(reach.last().map_or(b, |&r: &usize| r.max(b)));
        }
        let outside = |s: Span| {
            let at = s.start as usize;
            let i = ranges.partition_point(|&(a, _)| a <= at);
            i == 0 || reach[i - 1] <= at
        };
        let base = usize::from(n.text_range().start());
        for (id, span) in regions::id_lines(&n.to_string(), base) {
            if outside(span) {
                let d = SemanticDiagnostic::IdInUnrenderedRegion { id, span, region };
                self.out.diagnostics.push(d);
            }
        }
        let hidden = regions::region_contents(n, &self.rules);
        for (path, span) in hidden.links {
            if outside(span) {
                let d = SemanticDiagnostic::LinkInUnrenderedRegion { path, span, region };
                self.out.diagnostics.push(d);
            }
        }
        for (member, span) in hidden.transclusions {
            if outside(span) {
                let d = SemanticDiagnostic::TransclusionInUnrenderedRegion {
                    member,
                    span,
                    region,
                };
                self.out.diagnostics.push(d);
            }
        }
        for def in kept {
            self.walk(&def);
        }
    }

    /// Every `#+SETUPFILE:` keyword, wherever it is (Org reads them
    /// buffer-wide).
    fn setup_files(&mut self, doc: &Document) {
        for k in doc.keywords() {
            if k.key().eq_ignore_ascii_case("SETUPFILE") {
                self.out
                    .diagnostics
                    .push(SemanticDiagnostic::SetupFileNotRead {
                        file: org_trim(&k.value()).to_string(),
                        span: without_trailing_blanks(k.syntax()),
                    });
            }
        }
    }
}

/// A `#+TRANSCLUDE:` keyword's member and span, for a diagnostic.
pub(crate) fn transclude_member(n: &SyntaxNode) -> Option<(String, Span)> {
    let k = Keyword::cast(n.clone())?;
    if !k.key().eq_ignore_ascii_case("TRANSCLUDE") {
        return None;
    }
    let (member, _, _) = transclusion_parts(&k.value(), LinkTypes::defaults())?;
    Some((member, without_trailing_blanks(n)))
}

/// `(member, role, args)` of a `#+TRANSCLUDE:` value: org-transclusion's
/// `[[LINK][DESC]] ARGS…` (member: the link's target), else MAE's
/// `MEMBER [ROLE]`. `None` when there is no member.
fn transclusion_parts(value: &str, types: &LinkTypes) -> Option<(String, String, Option<String>)> {
    let value = org_trim(value);
    if value.starts_with("[[") {
        let tree = crate::green::objects(value, PARAGRAPH);
        let link = tree
            .children()
            .find(|c| c.kind() == LINK && u32::from(c.text_range().start()) == 0);
        if let Some(link) = link {
            let path = plain::link_path_buffer(&link)?;
            let member = classify_link_with(&path, types).target;
            let end = usize::from(link.text_range().end());
            let args = Some(org_trim(&value[end..]).to_string()).filter(|a| !a.is_empty());
            return Some((member, "content".to_string(), args)).filter(|m| !m.0.is_empty());
        }
    }
    let (member, role) = match value.split_once([' ', '\t']) {
        Some((m, r)) => (m, org_trim(r)),
        None => (value, ""),
    };
    if member.is_empty() {
        return None;
    }
    let role = if role.is_empty() { "content" } else { role };
    Some((member.to_string(), role.to_string(), None))
}

/// The value of a keyword the semantics read links from, and its offset:
/// `None` for an excluded key or an empty value.
pub(crate) fn keyword_link_value(n: &SyntaxNode, rules: &Rules) -> Option<(String, usize)> {
    let k = Keyword::cast(n.clone())?;
    if rules
        .link_exclude_keywords
        .contains(&k.key().to_ascii_uppercase())
    {
        return None;
    }
    value_token(n)
}

/// The value of a property line the semantics read links from, and its
/// offset: `None` for an excluded key or an empty value.
pub(crate) fn property_link_value(n: &SyntaxNode, rules: &Rules) -> Option<(String, usize)> {
    let key = n
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == NAME)?;
    let key = crate::views::canonical_key(key.text());
    let key = key.strip_suffix('+').unwrap_or(&key);
    if rules.link_exclude_properties.contains(key) {
        return None;
    }
    value_token(n)
}

pub(crate) fn value_token(n: &SyntaxNode) -> Option<(String, usize)> {
    let t = n
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == VALUE)?;
    Some((t.text().to_string(), usize::from(t.text_range().start())))
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

/// An object's span without its post-blank: the spaces and tabs
/// org-element counts as `:post-blank`. Not line endings: a plain link's
/// path may end in a content CR (`org-link-plain-re` takes it in an LF
/// file), and an object never owns the newline after it.
fn object_span(node: &SyntaxNode) -> Span {
    let s = Span::of(node);
    let kept = node.to_string().trim_end_matches([' ', '\t']).len();
    Span::new(s.start as usize, s.start as usize + kept)
}

/// An element's (a line's) span without trailing blanks and its line ending.
fn without_trailing_blanks(node: &SyntaxNode) -> Span {
    let s = Span::of(node);
    let text = node.to_string();
    let kept = text.trim_end_matches([' ', '\t', '\n', '\r']).len();
    Span::new(s.start as usize, s.start as usize + kept)
}

/// A headline's body before clipping: after its planning line and property
/// drawer, to the end of its subtree.
fn headline_body(n: &SyntaxNode) -> Span {
    let end = usize::from(n.text_range().end());
    let Some(section) = n.children().find(|c| c.kind() == SECTION) else {
        let at = n
            .children()
            .find(|c| c.kind() == HEADLINE)
            .map_or(n.text_range().end(), |c| c.text_range().start());
        return Span::new(usize::from(at), end);
    };
    let head = section
        .children()
        .take_while(|c| matches!(c.kind(), PLANNING | PROPERTY_DRAWER))
        .last();
    let start = head.map_or(section.text_range().start(), |h| h.text_range().end());
    Span::new(usize::from(start), end)
}

/// The file's body before clipping: after the zeroth section's leading
/// comments, property drawer and keywords, to the end of the document.
fn file_body(root: &SyntaxNode) -> Span {
    let end = usize::from(root.text_range().end());
    let Some(first) = root.children().next() else {
        return Span::new(end, end);
    };
    if first.kind() != SECTION {
        return Span::new(usize::from(first.text_range().start()), end);
    }
    let head = first
        .children()
        .take_while(|c| matches!(c.kind(), COMMENT | PROPERTY_DRAWER | KEYWORD))
        .last();
    let start = head.map_or(first.text_range().start(), |h| h.text_range().end());
    Span::new(usize::from(start), end)
}

/// Ends each body at the first node nested in its element (see the module
/// doc). Nodes are in document order and their spans nest, so the first
/// node nested in node `i`, if any, is node `i + 1`.
fn clip_bodies(nodes: &mut [SemanticNode]) {
    for i in 0..nodes.len() {
        let Some(next) = nodes.get(i + 1).map(|n| n.span.start) else {
            break;
        };
        let n = &mut nodes[i];
        if next < n.span.end && next < n.body_span.end {
            n.body_span.end = next.max(n.body_span.start);
        }
    }
}
