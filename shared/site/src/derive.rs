//! One KB, derived into pages (ADR-119 D5).
//!
//! Two renders per page, both by mae-org. The first, against an empty
//! resolver, finds where every node and `CUSTOM_ID` of the page landed, and
//! whether the page can be rendered faithfully at all. The faithful pages'
//! nodes go into ONE resolver for the KB, so a link never points at a page
//! that does not publish. The second render is each such page against it,
//! and its HTML passes the final sanitiser.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use mae_org::{
    parse, render, semantics, HeadlineIds, NodeOrigin, ParseOptions, RenderOptions, Rendered,
    Resolved, Resolver,
};

use crate::address::{export_collision_key, is_reserved, valid_id, valid_tag, Addressing};
use crate::highlight::{highlight_blocks, Highlighter, PlainBlock};
use crate::sanitize::sanitize_fragment;

/// One page's source: an Org text with a file-level `:ID:`.
#[derive(Debug, Clone)]
pub struct SourcePage {
    /// Where it came from (a bundle path, a store key): named in diagnostics
    /// only, never part of an address (D4).
    pub origin: String,
    pub text: String,
}

/// A derived page. `title` is raw text: the chrome escapes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPage {
    /// The file-level `:ID:`.
    pub id: String,
    pub title: String,
    pub href: String,
    /// The sanitised article body.
    pub html: String,
    /// Every published page that links to this one (P-36), by title then ID.
    pub backlinks: Vec<Backlink>,
    /// Its valid Org file tags (P-73), each once, in written order.
    pub tags: Vec<String>,
    /// Its headings in order, each with the anchor the page gives it (P-79).
    pub toc: Vec<TocEntry>,
    /// Its file-level properties (P-154), `ID` left out, for its slot
    /// components. Keys are mae-org's canonical form: Emacs's case folding,
    /// then ASCII upper case, so `:state:`, `:State:` and `:STATE:` are all
    /// `STATE`. Values are as Org reads them (`org-entry-get`), with hidden
    /// references removed (P-13).
    pub properties: BTreeMap<String, String>,
    /// Its dates by name, lower case, each as its source wrote it (a
    /// component reads one with `iso_date`): `created` and `updated`, from
    /// the `CREATED` and `UPDATED` properties, when present.
    pub dates: BTreeMap<String, String>,
}

/// The properties whose values are a page's dates, and the names
/// [`DerivedPage::dates`] gives them.
pub const DATE_PROPERTIES: &[(&str, &str)] = &[("CREATED", "created"), ("UPDATED", "updated")];

/// One heading of a page's table of contents. `text` is raw text: the
/// chrome escapes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// 1 to 6, as rendered.
    pub level: u8,
    pub text: String,
    /// The heading's own id on the page: every heading mae-org renders has one.
    pub anchor: String,
}

/// A page another page refers to: a backlink, a tag page's entry. `title` is
/// raw text: the chrome escapes it.
pub type Backlink = PageRef;

/// A reference to a published page.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageRef {
    pub title: String,
    pub id: String,
    pub href: String,
}

/// Why a page was withheld (P-147, P-15). A withheld page publishes nothing,
/// and no other page links to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Withheld {
    /// No file-level `:ID:`.
    NoFileId { origin: String },
    /// A file ID that cannot be an address: `.` or `..` (see
    /// [`crate::address::valid_id`]).
    InvalidId { origin: String, id: String },
    /// A file ID that names an address the engine generates (P-166): see
    /// [`crate::address::RESERVED_IDS`].
    Reserved { origin: String, id: String },
    /// One node ID on several pages, or twice on one, file-level or not:
    /// every page holding it is withheld, so no link is decided by order.
    DuplicateId { id: String, origins: Vec<String> },
    /// File IDs that are one directory in the export (case, trailing dots):
    /// see [`crate::address::export_collision_key`] (P-15).
    FsCollision { ids: Vec<String> },
    /// mae-org could not render it faithfully (`kind`: its diagnostic).
    Unfaithful { id: String, kind: &'static str },
}

/// A derived KB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Site {
    /// By file-level ID.
    pub pages: BTreeMap<String, DerivedPage>,
    pub withheld: Vec<Withheld>,
    /// Every valid tag of a published page, with its pages A to Z by title,
    /// then by ID ([`crate::order`]; P-70, P-71). Keyed by the tag's bytes:
    /// [`Site::tags_in_order`] is the order a reader sees.
    pub tags: BTreeMap<String, Vec<PageRef>>,
    /// The content index search and the graph read (P-77): one entry per
    /// published page, by ID.
    pub index: Vec<IndexEntry>,
    /// The published pages' source blocks left plain, and why (P-108): one
    /// per page, language and reason. Empty without a highlighter.
    pub plain_blocks: BTreeSet<PlainBlock>,
}

impl Site {
    /// Every tag with its pages, A to Z by tag ([`crate::order`]): the order
    /// of the tag index (P-71) and of the explorer's groups.
    pub fn tags_in_order(&self) -> Vec<(&str, &[PageRef])> {
        let mut v: Vec<(&str, &[PageRef])> = self
            .tags
            .iter()
            .map(|(t, p)| (t.as_str(), p.as_slice()))
            .collect();
        v.sort_by(|a, b| crate::order::cmp_text(a.0, b.0));
        v
    }
}

/// One page in the content index (P-77). It names published pages only and
/// holds no hidden-namespace reference (P-78).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub id: String,
    pub title: String,
    pub href: String,
    pub tags: Vec<String>,
    /// The published pages it links to, by ID, each once, sorted.
    pub links: Vec<String>,
    /// Its rendered text.
    pub text: String,
}

/// What the site's operator configures (D3): never content.
#[derive(Clone, Default)]
pub struct SiteOptions {
    /// Namespaces whose references (`ns:rest`) are removed from every output
    /// (P-13, P-78).
    pub hidden_namespaces: Vec<String>,
    /// What highlights source blocks (D12); `None`, every block is plain.
    pub highlighter: Option<Arc<dyn Highlighter>>,
}

impl std::fmt::Debug for SiteOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteOptions")
            .field("hidden_namespaces", &self.hidden_namespaces)
            .field("highlighter", &self.highlighter.is_some())
            .finish()
    }
}

/// The render diagnostics that withhold a page: its output would not be what
/// the author wrote (P-147). None depends on the resolver, so the first
/// render decides them.
fn withholds(kind: &str) -> bool {
    kind.starts_with("macro_") && kind != "fragment_macro" || kind == "footnote_undefined"
}

/// How every page renders. A top-level headline is an `h2`: the page title
/// is the page's one `h1`, written by the chrome (P-17), as ox-html's
/// `org-html-toplevel-hlevel` 2 does.
fn options() -> RenderOptions {
    let below_the_title = mae_org::ToplevelHlevel::new(2).expect("2 is a heading level");
    RenderOptions::default()
        .with_headline_ids(HeadlineIds::Explicit)
        .with_toplevel_hlevel(below_the_title)
}

struct Parsed<'a> {
    source: &'a SourcePage,
    doc: mae_org::Document,
    sem: mae_org::Semantics,
    id: Option<String>,
    title: Option<String>,
}

/// Derive every page of one KB, with no operator options.
pub fn derive(sources: &[SourcePage], addressing: &Addressing) -> Site {
    derive_with(sources, addressing, &SiteOptions::default())
}

/// Derive every page of one KB under the operator's options.
pub fn derive_with(sources: &[SourcePage], addressing: &Addressing, opts: &SiteOptions) -> Site {
    let mut site = Site::default();
    let parsed: Vec<Parsed> = sources.iter().map(|s| parse_page(s, opts)).collect();
    let candidates = addressable(&parsed, addressing, &mut site.withheld);

    // Pass 1: where each page's nodes and CUSTOM_IDs land, and whether it
    // renders faithfully.
    let empty = Resolver::default();
    let mut first: Vec<(usize, Rendered)> = Vec::new();
    for i in candidates {
        let p = &parsed[i];
        let r = render(&p.doc, &p.sem, &options(), &empty);
        match r.diagnostics.iter().find(|d| withholds(d.kind_name())) {
            Some(d) => site.withheld.push(Withheld::Unfaithful {
                id: p.id.clone().unwrap_or_default(),
                kind: d.kind_name(),
            }),
            None => first.push((i, r)),
        }
    }
    let resolver = resolver(&parsed, &first, addressing);
    let backlinks = backlinks(&parsed, &first, addressing);

    // Pass 2: every faithful page against the one resolver.
    for (i, _) in &first {
        let p = &parsed[*i];
        let id = p.id.clone().unwrap_or_default();
        let Some(href) = addressing.page_href(&id) else {
            continue;
        };
        let mut r = render(&p.doc, &p.sem, &options(), &resolver);
        scrub_nodes(&mut r.body, &opts.hidden_namespaces);
        // After scrubbing: a block is highlighted as it is shown.
        if let Some(h) = &opts.highlighter {
            highlight_blocks(&mut r.body, &id, h.as_ref(), &mut site.plain_blocks);
        }
        site.index.push(IndexEntry {
            id: id.clone(),
            title: p.title.clone().unwrap_or_else(|| id.clone()),
            href: href.clone(),
            tags: page_tags(&p.sem),
            links: Vec::new(),
            text: mae_org::text_of(&r.body),
        });
        let properties = file_properties(&p.sem, &opts.hidden_namespaces);
        let page = DerivedPage {
            title: p.title.clone().unwrap_or_else(|| id.clone()),
            href,
            html: sanitize_fragment(&mae_org::to_html(&r.body)),
            backlinks: backlinks.get(&id).cloned().unwrap_or_default(),
            tags: page_tags(&p.sem),
            toc: toc(&r.body),
            dates: dates_of(&properties),
            properties,
            id: id.clone(),
        };
        list_under_tags(&mut site.tags, &page);
        site.pages.insert(id, page);
    }
    finish_lists(&mut site);
    site
}

/// The lists derived from every page at once: each tag's pages A to Z, and
/// each index entry's outgoing links to published pages (the backlinks,
/// inverted), with the index in ID order.
fn finish_lists(site: &mut Site) {
    for pages in site.tags.values_mut() {
        pages.sort_by(crate::explorer::by_title);
    }
    let mut out: BTreeMap<&str, std::collections::BTreeSet<String>> = BTreeMap::new();
    for page in site.pages.values() {
        for b in &page.backlinks {
            out.entry(b.id.as_str())
                .or_default()
                .insert(page.id.clone());
        }
    }
    for e in &mut site.index {
        e.links = out
            .remove(e.id.as_str())
            .unwrap_or_default()
            .into_iter()
            .collect();
    }
    site.index.sort_by(|a, b| a.id.cmp(&b.id));
}

/// `s` with every reference in a hidden namespace removed: a word that starts
/// `ns:` for a hidden `ns`, up to whitespace, keeping trailing punctuation so
/// the sentence still reads (P-13). `None` when nothing was hidden.
pub fn scrub(s: &str, hidden: &[String]) -> Option<String> {
    if hidden.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut rest = s;
    while !rest.is_empty() {
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (word, tail) = rest.split_at(word_end);
        let lead = word.len() - word.trim_start_matches(|c: char| "([\"'".contains(c)).len();
        let core = &word[lead..];
        let trail_at = core
            .trim_end_matches(|c: char| ".,;:!?)]\"'".contains(c))
            .len();
        let token = &core[..trail_at];
        let is_hidden = token
            .split_once(':')
            .is_some_and(|(ns, r)| !r.is_empty() && hidden.iter().any(|h| h == ns));
        if is_hidden {
            out.push_str(&word[..lead]);
            out.push_str(&core[trail_at..]);
            changed = true;
        } else {
            out.push_str(word);
        }
        let ws_end = tail
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(tail.len());
        out.push_str(&tail[..ws_end]);
        rest = &tail[ws_end..];
    }
    changed.then_some(out)
}

fn scrub_nodes(nodes: &mut [mae_org::Node], hidden: &[String]) {
    if hidden.is_empty() {
        return;
    }
    for n in nodes {
        match n {
            mae_org::Node::Text(t) => {
                if let Some(s) = scrub(t, hidden) {
                    *t = s;
                }
            }
            mae_org::Node::Element(e) => scrub_nodes(&mut e.children, hidden),
            _ => {}
        }
    }
}

/// Who links to whom, over the pages that publish only (P-36): a link to any
/// node of page Q is a link to Q, a page never backlinks itself, and a link
/// from a withheld page counts for nothing. mae-org lists only the links in
/// rendered regions, so a `:noexport:` subtree's links do not count either.
fn backlinks(
    parsed: &[Parsed],
    first: &[(usize, Rendered)],
    addressing: &Addressing,
) -> BTreeMap<String, Vec<Backlink>> {
    let mut page_of: HashMap<&str, &str> = HashMap::new();
    for (i, _) in first {
        let p = &parsed[*i];
        let Some(id) = p.id.as_deref() else { continue };
        for n in &p.sem.nodes {
            page_of.insert(n.id.as_str(), id);
        }
    }
    let mut out: BTreeMap<String, std::collections::BTreeSet<Backlink>> = BTreeMap::new();
    for (i, _) in first {
        let p = &parsed[*i];
        let Some(from) = p.id.as_deref() else {
            continue;
        };
        let Some(href) = addressing.page_href(from) else {
            continue;
        };
        let me = Backlink {
            title: p.title.clone().unwrap_or_else(|| from.to_string()),
            id: from.to_string(),
            href,
        };
        for l in p
            .sem
            .links
            .iter()
            .filter(|l| l.kind == mae_org::LinkKind::Kb)
        {
            if let Some(&to) = page_of.get(l.target.as_str()) {
                if to != from {
                    out.entry(to.to_string()).or_default().insert(me.clone());
                }
            }
        }
    }
    out.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

/// A page's headings in document order, from the rendered tree, so every
/// anchor is an id the page really has (P-79). A heading inside an unrendered
/// subtree is not in the tree, so not in the TOC.
fn toc(body: &[mae_org::Node]) -> Vec<TocEntry> {
    fn walk(nodes: &[mae_org::Node], out: &mut Vec<TocEntry>) {
        for n in nodes {
            let mae_org::Node::Element(e) = n else {
                continue;
            };
            let level = match e.tag {
                mae_org::Tag::H1 => 1,
                mae_org::Tag::H2 => 2,
                mae_org::Tag::H3 => 3,
                mae_org::Tag::H4 => 4,
                mae_org::Tag::H5 => 5,
                mae_org::Tag::H6 => 6,
                _ => {
                    walk(&e.children, out);
                    continue;
                }
            };
            if let Some(anchor) = e.get(mae_org::Attr::Id) {
                out.push(TocEntry {
                    level,
                    text: mae_org::text_of(&e.children),
                    anchor: anchor.to_string(),
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

/// A page's valid file tags (P-73), each once, in written order.
fn page_tags(sem: &mae_org::Semantics) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in &sem.file_tags {
        if valid_tag(t) && !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

/// `page` under each of its tags, for the tag pages (P-70).
fn list_under_tags(tags: &mut BTreeMap<String, Vec<PageRef>>, page: &DerivedPage) {
    for t in &page.tags {
        tags.entry(t.clone()).or_default().push(PageRef {
            title: page.title.clone(),
            id: page.id.clone(),
            href: page.href.clone(),
        });
    }
}

/// The file node's properties but its `ID` (see [`DerivedPage::properties`]).
fn file_properties(sem: &mae_org::Semantics, hidden: &[String]) -> BTreeMap<String, String> {
    let Some(file) = sem.nodes.iter().find(|n| n.origin == NodeOrigin::File) else {
        return BTreeMap::new();
    };
    file.properties
        .iter()
        .filter(|(k, _)| k != "ID")
        .map(|(k, v)| (k.clone(), scrub(v, hidden).unwrap_or_else(|| v.clone())))
        .collect()
}

/// [`DATE_PROPERTIES`] read from `properties`.
fn dates_of(properties: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    DATE_PROPERTIES
        .iter()
        .filter_map(|(key, name)| properties.get(*key).map(|v| (name.to_string(), v.clone())))
        .collect()
}

fn parse_page<'a>(source: &'a SourcePage, opts: &SiteOptions) -> Parsed<'a> {
    let doc = parse(&source.text, &ParseOptions::default());
    let sem = semantics(&doc);
    let file = sem.nodes.iter().find(|n| n.origin == NodeOrigin::File);
    let id = file.map(|n| n.id.clone());
    let title = file
        .and_then(|n| n.title.clone())
        .map(|t| scrub(&t, &opts.hidden_namespaces).unwrap_or(t));
    Parsed {
        source,
        doc,
        sem,
        id,
        title,
    }
}

/// The pages that may publish, before rendering: a valid file ID, no node ID
/// shared with any other node of the KB, and (in the export) no file ID in
/// the same directory as another's.
fn addressable(
    parsed: &[Parsed],
    addressing: &Addressing,
    withheld: &mut Vec<Withheld>,
) -> Vec<usize> {
    let mut ok: Vec<usize> = Vec::new();
    for (i, p) in parsed.iter().enumerate() {
        match &p.id {
            None => withheld.push(Withheld::NoFileId {
                origin: p.source.origin.clone(),
            }),
            Some(id) if !valid_id(id) => withheld.push(Withheld::InvalidId {
                origin: p.source.origin.clone(),
                id: id.clone(),
            }),
            Some(id) if is_reserved(id) => withheld.push(Withheld::Reserved {
                origin: p.source.origin.clone(),
                id: id.clone(),
            }),
            Some(_) => ok.push(i),
        }
    }
    // Every node ID, file-level or not, once in the KB.
    let mut holders: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &i in &ok {
        for n in &parsed[i].sem.nodes {
            holders.entry(n.id.as_str()).or_default().push(i);
        }
    }
    let mut out_dup = vec![false; parsed.len()];
    for (id, at) in &holders {
        if at.len() > 1 {
            let origins = at
                .iter()
                .map(|&i| parsed[i].source.origin.clone())
                .collect();
            withheld.push(Withheld::DuplicateId {
                id: id.to_string(),
                origins,
            });
            for &i in at {
                out_dup[i] = true;
            }
        }
    }
    ok.retain(|&i| !out_dup[i]);
    if addressing.is_export() {
        let mut dirs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &i in &ok {
            dirs.entry(export_collision_key(
                parsed[i].id.as_deref().unwrap_or_default(),
            ))
            .or_default()
            .push(i);
        }
        let mut out = vec![false; parsed.len()];
        for at in dirs.values().filter(|at| at.len() > 1) {
            let ids = at
                .iter()
                .map(|&i| parsed[i].id.clone().unwrap_or_default())
                .collect();
            withheld.push(Withheld::FsCollision { ids });
            for &i in at {
                out[i] = true;
            }
        }
        ok.retain(|&i| !out[i]);
    }
    ok
}

/// The KB's one resolver, over the pages that will publish: every page, every
/// node on it at its anchor, and each page's CUSTOM_IDs (S0). A heading's
/// UUID `CUSTOM_ID` is an ID too (D5), unless a node holds that ID or two
/// pages share the UUID: then it resolves nowhere rather than by order.
fn resolver(parsed: &[Parsed], first: &[(usize, Rendered)], addressing: &Addressing) -> Resolver {
    let mut r = Resolver::default();
    let mut uuid_cids: HashMap<String, Option<Resolved>> = HashMap::new();
    for (i, rendered) in first {
        let p = &parsed[*i];
        let Some(id) = &p.id else { continue };
        let Some(href) = addressing.page_href(id) else {
            continue;
        };
        let mut page =
            Resolved::new(href.clone()).with_title(p.title.clone().unwrap_or_else(|| id.clone()));
        for (c, a) in &rendered.custom_ids {
            page = page.with_custom_id(c.clone(), a.clone());
            if is_uuid(c) {
                let at = Resolved::new(format!("{href}#{a}"));
                uuid_cids
                    .entry(c.clone())
                    .and_modify(|e| *e = None)
                    .or_insert(Some(at));
            }
        }
        r = r.with_target(id.clone(), page);
        for n in &rendered.nodes {
            if &n.id == id {
                continue;
            }
            let at = match &n.anchor {
                Some(a) => format!("{href}#{a}"),
                None => href.clone(),
            };
            let title = p
                .sem
                .nodes
                .iter()
                .find(|s| s.id == n.id)
                .and_then(|s| s.title.clone());
            let mut t = Resolved::new(at);
            if let Some(title) = title {
                t = t.with_title(title);
            }
            r = r.with_target(n.id.clone(), t);
        }
    }
    for (c, t) in uuid_cids {
        if let Some(t) = t {
            if !r.targets.contains_key(&c) {
                r = r.with_target(c, t);
            }
        }
    }
    r
}

/// 8-4-4-4-12 hex digits.
fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}
