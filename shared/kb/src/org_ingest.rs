//! MAE's KB derivation from org text, built on `mae-org` (ADR-112 O4).
//! @ai-caution: [kb-links] Nothing calls this module yet (#941 PR1). It is
//! the one derivation the ingest, edge and node-buffer paths move onto in
//! PR2–PR4; until then `crate::org` stays authoritative, and the two are
//! held together by `tests/org_semantics_differential.rs`, which requires
//! every difference to be one `mae-org`'s divergence register lists.
//!
//! Every function here is a projection of [`mae_org::semantics_with`] under
//! [`org_options`]: nothing re-splits org text. Where `mae-org` and the old
//! parser represent the same fact differently, the adapter maps the
//! REPRESENTATION only, and says so:
//! - **properties** (#941 G2): keys lower-cased, as `crate::org` has always
//!   stored them, and `ID` dropped (it is the node's id). A file node's
//!   `KIND` and `ALIASES` are fields, not properties; a headline's or an
//!   item's stay in its properties as well, as `crate::org` kept them;
//! - **titles**: a missing title is the id (file, item) or empty (headline);
//! - **kind**: `NodeKind::from_str_lossy` of the text;
//! - **tags**: the file tags, then the node's own ([`Semantics::tags_of`]);
//! - **edges**: KB links with a target or a fragment ([`is_edge`]);
//! - **bodies** (#941 G4): a node's body is its `body_span`, the write
//!   contract's unit, through [`store_body`]'s link rewrite. It stops at the
//!   first node nested in it and holds no front matter, unlike
//!   `crate::org`'s; its bytes are the file's (a CRLF file's body keeps its
//!   CRs), so a body can be spliced back where it came from.
//!
//! **The store form keeps the ADR-030 query.** `crate::org::rewrite_links`
//! dropped `?rel=&w=&c=` from every stored link, so an ingested node's
//! stored edges were all `references` at weight 1 whatever the file said;
//! only `kb_build`, which read the raw file, kept them. [`store_body`]
//! writes `[[TARGET#FRAGMENT?QUERY|DESCRIPTION]]`, which `mae-org`'s link
//! grammar (step 3: the store form's `|` first, then the query, then the
//! fragment) reads back to the same edge, so the edges of a stored body are
//! the edges of its source ([`edges_of_body`]). The help renderer already
//! strips the query from a target, and `export::convert_links_to_org`
//! carries it into `[[id:T?Q][D]]`, so no consumer of stored bodies is
//! harmed by it.

use crate::org::{OrgParseResult, ParsedLink};
use crate::{Node, NodeKind};
use mae_org::{
    parse, semantics_with, Document, Link, LinkFormat, LinkKind, NodeOrigin, ParseOptions,
    SemanticLink, SemanticNode, Semantics, SemanticsOptions, SyntaxKind, TodoSequence,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::OnceLock;

/// The declared configuration MAE parses org with (ADR-112 D4): one value,
/// so ingest, a dry-run import and edge derivation can never read the same
/// text differently.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct OrgOptions {
    pub parse: ParseOptions,
    pub semantics: SemanticsOptions,
}

/// MAE's TODO keywords, done states after the `|`: the six `crate::org`
/// recognises (`TODO_KEYWORDS`), split as the agenda and the highlighter
/// split them (`DONE`, `CANCELLED`, `DEFERRED` are done). `mae-org`'s own
/// default is Org's `TODO | DONE` (#941 G1). An in-file `#+TODO:` replaces
/// them, as in Org.
pub const TODO_STATES: [&str; 3] = ["TODO", "NEXT", "WAIT"];
/// See [`TODO_STATES`].
pub const DONE_STATES: [&str; 3] = ["DONE", "CANCELLED", "DEFERRED"];

/// The options every derivation in this module uses, built once.
pub fn org_options() -> &'static OrgOptions {
    static OPTIONS: OnceLock<OrgOptions> = OnceLock::new();
    OPTIONS.get_or_init(|| OrgOptions {
        parse: ParseOptions::default()
            .with_todo_sequences(vec![TodoSequence::new(TODO_STATES, DONE_STATES)]),
        semantics: SemanticsOptions::default(),
    })
}

/// `text` parsed and projected under [`org_options`].
fn derive(text: &str) -> (Document, Semantics) {
    let o = org_options();
    let doc = parse(text, &o.parse);
    let sem = semantics_with(&doc, &o.semantics);
    (doc, sem)
}

/// Whether a link is a graph edge: a KB link naming a node or a fragment.
pub fn is_edge(l: &SemanticLink) -> bool {
    l.kind == LinkKind::Kb && !(l.target.is_empty() && l.fragment.is_none())
}

/// A link as `crate::org` represents an edge. `display` is the description
/// (source text) or the target.
fn parsed_link(l: &SemanticLink) -> ParsedLink {
    ParsedLink {
        target: l.target.clone(),
        display: l.description.clone().unwrap_or_else(|| l.target.clone()),
        rel_type: l.rel.clone(),
        fragment: l.fragment.clone(),
        weight: l.weight,
        confidence: l.confidence,
        attrs: l.attrs.clone(),
    }
}

/// The edges a body declares, in document order: every KB link of it,
/// whichever node of the body's own text it sits under (a body holds one
/// node's text; an old body that still holds nested nodes keeps counting
/// their links, as `crate::org::parse_typed_links` did). Raw org and
/// [`store_body`]'s form give the same edges.
pub fn edges_of_body(body: &str) -> Vec<ParsedLink> {
    let (_, sem) = derive(body);
    sem.links
        .iter()
        .filter(|l| is_edge(l))
        .map(parsed_link)
        .collect()
}

/// The `#+TRANSCLUDE:` members of a body, as `(member, role)` in document
/// order (MAE's `MEMBER [ROLE]` and org-transclusion's link form).
pub fn transclusions_of_body(body: &str) -> Vec<(String, String)> {
    let (_, sem) = derive(body);
    sem.transclusions
        .into_iter()
        .map(|t| (t.member, t.role))
        .collect()
}

/// Every node of an org file. See [`parse_file`].
pub fn parse_nodes(text: &str) -> Vec<Node> {
    parse_file(text).nodes
}

/// An org file's nodes, edges `(source, link)` and transclusions
/// `(source, member, role)`, each in document order. Links and
/// transclusions outside every node are not returned (they have no
/// source); a node's are its own, never also its ancestors'.
pub fn parse_file(text: &str) -> OrgParseResult {
    let (doc, sem) = derive(text);
    let edits = checked_edits(text, &doc, &sem);
    let nodes = sem
        .nodes
        .iter()
        .map(|n| node_of(text, &sem, n, &edits))
        .collect();
    let typed_links = sem
        .links
        .iter()
        .filter(|l| is_edge(l))
        .filter_map(|l| Some((l.from.clone()?, parsed_link(l))))
        .collect();
    let transclusions = sem
        .transclusions
        .iter()
        .filter_map(|t| Some((t.from.clone()?, t.member.clone(), t.role.clone())))
        .collect();
    OrgParseResult {
        nodes,
        typed_links,
        transclusions,
    }
}

/// One node in `crate::Node`'s representation (see the module doc).
fn node_of(text: &str, sem: &Semantics, n: &SemanticNode, edits: &[Edit]) -> Node {
    let title = n.title.clone().unwrap_or_else(|| match n.origin {
        NodeOrigin::Headline => String::new(),
        _ => n.id.clone(),
    });
    let kind = NodeKind::from_str_lossy(n.kind.as_deref().unwrap_or(""));
    let body = apply_edits(text, n.body_span.range(), edits);
    let mut node = Node::new(n.id.clone(), title, kind, body).with_tags(sem.tags_of(n));
    node.aliases = n.aliases.clone();
    node.properties = stored_properties(n);
    node.todo_state = n.todo.clone();
    node.priority = n.priority;
    node
}

/// A node's properties as the store keeps them (#941 G2): keys lower-cased,
/// without `ID`, and for the file node without the `KIND`/`ALIASES` it holds
/// as fields. `mae-org` gives each key once (canonical, first-occurrence
/// order), so lower-casing cannot merge two.
pub fn stored_properties(n: &SemanticNode) -> HashMap<String, String> {
    let file = n.origin == NodeOrigin::File;
    let is_field = |k: &str| k == "ID" || (file && (k == "KIND" || k == "ALIASES"));
    n.properties
        .iter()
        .filter(|(k, _)| !is_field(k))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect()
}

// ------------------------------------------------------------ store form

/// One link of the text rewritten: its range and its replacement.
#[derive(Debug, Clone, PartialEq)]
struct Edit {
    range: Range<usize>,
    text: String,
}

/// `text` in the form the store keeps: every bracket link `mae-org` reads
/// rewritten the way `crate::org::rewrite_links_with_types` rewrote it,
/// with the differences `tests/org_semantics_differential.rs` lists
/// (`stored_bodies_differ_from_mae_kb_only_as_listed`): the ADR-030 query
/// is kept (see the module doc); a KB link is whatever `mae-org`'s grammar
/// says is one, so a bare id or an unknown scheme stays a link where
/// mae-kb flattened anything outside its `KB_NAMESPACES` to text (while
/// still making an edge of it); and verbatim and unrendered text is left
/// alone, where mae-kb's scanner rewrote `[[ … ]]` in it.
/// - a KB link becomes `[[TARGET#FRAGMENT?QUERY|DESCRIPTION]]`: no `id:`,
///   no `::search` option (an `id:` link's search is not part of the edge,
///   and mae-kb's `x::*h` target named no node);
/// - any other bracket link (web, mail, file, another scheme, a local
///   `[[*h]]`/`[[#c]]`/`[[(r)]]` target) becomes `DESCRIPTION (PATH)`, or
///   `(PATH)` without a description, so that consumers still reading
///   `[[…]]` as a KB link (the export and help renderers, until #942/#943)
///   never see one;
/// - a link the semantics do not read (in a comment, a drawer, an excluded
///   subtree, a table, verbatim text, a src block) and every plain or angle
///   link is left as written.
///
/// A rewrite is kept only when the replacement, read alone, is what the
/// link was: the same edge for a KB link, no edge for any other; and then
/// only if the whole result still makes the same edges ([`checked_edits`]).
/// Otherwise links stay as written, so the edges of the result are always
/// the edges of `text`. Idempotent.
pub fn store_body(text: &str) -> String {
    let (doc, sem) = derive(text);
    let edits = checked_edits(text, &doc, &sem);
    apply_edits(text, 0..text.len(), &edits)
}

/// [`store_edits`], kept only if the whole rewritten text makes the same
/// edges from the same nodes and has nothing left to rewrite; otherwise
/// none. Each replacement is checked alone, but text around it can still
/// change how it reads: a flattened `[[https://e.org]]` inside `[[ ]]`
/// makes a new link, and one after `id:` makes a plain id link of its
/// description. Rewriting all or nothing keeps [`store_body`] idempotent
/// (a second pass finds no edit, or makes the same refusal) at the cost of
/// one more parse, and a body refused keeps its links as written, which
/// every reader of stored bodies also accepts.
fn checked_edits(text: &str, doc: &Document, sem: &Semantics) -> Vec<Edit> {
    let edits = store_edits(text, doc, sem);
    if edits.is_empty() {
        return edits;
    }
    let result = apply_edits(text, 0..text.len(), &edits);
    let (rdoc, rsem) = derive(&result);
    let settled =
        edge_keys(&rsem) == edge_keys(sem) && store_edits(&result, &rdoc, &rsem).is_empty();
    if settled {
        edits
    } else {
        Vec::new()
    }
}

/// An edge's source and payload, without its display text.
type EdgeKey<'a> = (
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    &'a str,
    u64,
    u64,
    &'a [(String, String)],
);

/// Every edge of `sem`, in document order, as [`EdgeKey`]s.
fn edge_keys(sem: &Semantics) -> Vec<EdgeKey<'_>> {
    sem.links
        .iter()
        .filter(|l| is_edge(l))
        .map(|l| {
            (
                l.from.as_deref(),
                l.target.as_str(),
                l.fragment.as_deref(),
                l.rel.as_str(),
                l.weight.to_bits(),
                l.confidence.to_bits(),
                l.attrs.as_slice(),
            )
        })
        .collect()
}

/// `text[range]` with the edits that fall inside it applied. Edits are
/// sorted and disjoint (one per link; links never overlap), so the ones in
/// `range` are found by binary search: a file of N nodes and M links costs
/// N·log M, not N·M.
fn apply_edits(text: &str, range: Range<usize>, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(range.len());
    let mut at = range.start;
    let first = edits.partition_point(|e| e.range.start < range.start);
    for e in edits[first..]
        .iter()
        .take_while(|e| e.range.start < range.end)
        .filter(|e| e.range.end <= range.end)
    {
        out.push_str(&text[at..e.range.start]);
        out.push_str(&e.text);
        at = e.range.end;
    }
    out.push_str(&text[at..range.end]);
    out
}

fn store_edits(text: &str, doc: &Document, sem: &Semantics) -> Vec<Edit> {
    let links = tree_links(doc);
    let candidates: Vec<(&SemanticLink, Edit)> = sem
        .links
        .iter()
        .filter_map(|l| Some((l, candidate(text, &links, l)?)))
        .collect();
    let readings = Readings::of(candidates.iter().map(|(_, e)| e.text.as_str()));
    let mut edits: Vec<Edit> = candidates
        .into_iter()
        .filter(|(l, e)| readings.confirm(&e.text, l))
        .map(|(_, e)| e)
        .collect();
    edits.sort_by_key(|e| e.range.start);
    // Defensive: links never overlap, but a pair that did must not make
    // `apply_edits` slice backwards.
    edits.dedup_by(|b, a| b.range.start < a.range.end);
    edits
}

/// The rewrite of one link, if it is a bracket link that has a store form
/// other than its text; not yet checked ([`Readings::confirm`]).
fn candidate(text: &str, links: &HashMap<usize, Link>, l: &SemanticLink) -> Option<Edit> {
    let range = l.span.range();
    let src = text.get(range.clone())?;
    let raw = RawLink::read(links.get(&range.start), src)?;
    let r = if l.kind == LinkKind::Kb {
        kb_store_form(l, &raw)
    } else {
        flattened(&raw)
    };
    (r != src).then_some(Edit { range, text: r })
}

/// What each distinct replacement reads as on its own, from ONE parse:
/// every one-line replacement on a line of its own (after a word, so a
/// replacement that starts like `#+begin_src` or `* ` opens nothing),
/// blank lines between them. A replacement over several lines could join
/// its neighbours, so it is read alone. A link per replacement parsed
/// separately cost a full parse each: 50,000 distinct links took minutes.
#[derive(Default)]
struct Readings(HashMap<String, Vec<SemanticLink>>);

/// What a one-line replacement is preceded by in the shared parse.
const READ_AFTER: &str = "x ";

impl Readings {
    fn of<'a>(replacements: impl Iterator<Item = &'a str>) -> Readings {
        let mut out = Readings::default();
        let (mut batch, mut at) = (String::new(), Vec::new());
        for r in replacements {
            if out.0.contains_key(r) {
                continue;
            }
            out.0.insert(r.to_string(), Vec::new());
            if r.contains(['\n', '\r']) {
                out.0.insert(r.to_string(), derive(r).1.links);
                continue;
            }
            batch.push_str(READ_AFTER);
            at.push((batch.len(), r));
            batch.push_str(r);
            batch.push_str("\n\n");
        }
        for mut l in derive(&batch).1.links {
            let i = at.partition_point(|(start, _)| *start <= l.span.start as usize);
            let Some(&(start, r)) = i.checked_sub(1).and_then(|i| at.get(i)) else {
                continue;
            };
            l.span.start -= start as u32;
            l.span.end -= start as u32;
            out.0.entry(r.to_string()).or_default().push(l);
        }
        out
    }

    /// Whether `replacement` is what `l` was: the same edge for a KB link,
    /// no edge for any other.
    fn confirm(&self, replacement: &str, l: &SemanticLink) -> bool {
        let links = self.0.get(replacement).map_or(&[][..], Vec::as_slice);
        if l.kind == LinkKind::Kb {
            reads_as_edge(links, replacement.len(), l)
        } else {
            reads_as_no_edge(links)
        }
    }
}

/// A bracket link's text as written: its path and description.
struct RawLink<'a> {
    path: &'a str,
    description: Option<&'a str>,
}

impl<'a> RawLink<'a> {
    /// The bracket link that `src` is (`link`: the tree's link starting
    /// where it does), unless it is not one or sits in a table (where the
    /// store form's `|` would split a cell). A link in a keyword or property
    /// value is not in the tree; its text alone is read.
    fn read(link: Option<&Link>, src: &'a str) -> Option<RawLink<'a>> {
        let inner = src.strip_prefix("[[")?.strip_suffix("]]")?;
        let link = match link {
            Some(link) => link.clone(),
            None => {
                let alone = parse(src, &org_options().parse);
                tree_links(&alone).remove(&0)?
            }
        };
        let in_table = link
            .syntax()
            .ancestors()
            .any(|a| matches!(a.kind(), SyntaxKind::TABLE | SyntaxKind::TABLE_ROW));
        if link.format() != LinkFormat::Bracket || in_table {
            return None;
        }
        let description = link.description_text();
        let path = match &description {
            // `[[PATH][DESCRIPTION]]`: the description is the end of `inner`.
            Some(d) => inner.strip_suffix(d.as_str())?.strip_suffix("][")?,
            None => inner,
        };
        let description = description.map(|d| &inner[inner.len() - d.len()..]);
        Some(RawLink { path, description })
    }
}

/// Every `LINK` node of `doc` by its start offset (the outermost, where
/// two start together), collected in one walk. Looking each link up from
/// the root instead (`token_at_offset`) scans a node's children at every
/// level, so a paragraph of N links costs N² (1 MB of links: minutes).
fn tree_links(doc: &Document) -> HashMap<usize, Link> {
    let mut out = HashMap::new();
    for l in doc.syntax().descendants().filter_map(Link::cast) {
        let start = usize::from(l.syntax().text_range().start());
        out.entry(start).or_insert(l);
    }
    out
}

/// `[[TARGET#FRAGMENT?QUERY|DESCRIPTION]]` for a KB link. The query and the
/// description are the source's text, so a second rewrite changes nothing.
fn kb_store_form(l: &SemanticLink, raw: &RawLink) -> String {
    // The store form's own description (`[[T|D]]`) when there is no
    // bracket one; the query is in the part before it.
    let (before_bar, bar_description) = match raw.path.split_once('|') {
        Some((p, d)) => (p, Some(d)),
        None => (raw.path, None),
    };
    let query = before_bar.split_once('?').map(|(_, q)| q);
    let description = raw.description.or(bar_description);
    let mut out = format!("[[{}", l.target);
    if let Some(f) = &l.fragment {
        out.push('#');
        out.push_str(f);
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(d) = description {
        out.push('|');
        out.push_str(d);
    }
    out.push_str("]]");
    out
}

/// `DESCRIPTION (PATH)`, or `(PATH)`: a link that is not an edge, as plain
/// text.
fn flattened(raw: &RawLink) -> String {
    match raw.description {
        Some(d) => format!("{d} ({})", raw.path),
        None => format!("({})", raw.path),
    }
}

/// Whether a replacement of `len` bytes whose links, read alone, are
/// `links` is exactly one link spanning all of it, making the same edge as
/// `l`.
fn reads_as_edge(links: &[SemanticLink], len: usize, l: &SemanticLink) -> bool {
    match links {
        [r] => {
            r.span.range() == (0..len)
                && is_edge(r) == is_edge(l)
                && r.kind == l.kind
                && r.target == l.target
                && r.fragment == l.fragment
                && r.rel == l.rel
                && r.weight.to_bits() == l.weight.to_bits()
                && r.confidence.to_bits() == l.confidence.to_bits()
                && r.attrs == l.attrs
        }
        _ => false,
    }
}

/// Whether a replacement whose links, read alone, are `links` makes no
/// edge.
fn reads_as_no_edge(links: &[SemanticLink]) -> bool {
    !links.iter().any(is_edge)
}
