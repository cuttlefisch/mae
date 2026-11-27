//! One KB, derived into pages (ADR-119 D5).
//!
//! Two renders per page, both by mae-org. The first, against an empty
//! resolver, finds where every node and `CUSTOM_ID` of the page landed, and
//! whether the page can be rendered faithfully at all. The faithful pages'
//! nodes go into ONE resolver for the KB, so a link never points at a page
//! that does not publish. The second render is each such page against it,
//! and its HTML passes the final sanitiser.
//!
//! The passes themselves are [`crate::incremental`]'s: a full derivation is a
//! fresh [`crate::incremental::Deriver`], and a kept one re-derives only what
//! changed (D6).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mae_org::{AttachmentState, HeadlineIds, NodeOrigin, RenderOptions};

use crate::address::{valid_tag, Addressing};
use crate::highlight::{Highlighter, PlainBlock};

/// One page's source: an Org text with a file-level `:ID:`.
#[derive(Debug, Clone)]
pub struct SourcePage {
    /// Where it came from (a bundle path, a store key): named in diagnostics
    /// only, never part of an address (D4).
    pub origin: String,
    pub text: String,
    /// The node's attachments by NAME, as the host found each (ADR-115 E3):
    /// [`AttachmentState::Resolved`] with the blob's URL when it holds the
    /// bytes, [`AttachmentState::Missing`] when it does not. An
    /// `[[attachment:NAME]]` link of the page whose NAME is not here renders
    /// as missing: a page never shows a file its node does not bind.
    pub attachments: BTreeMap<String, AttachmentState>,
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
pub(crate) fn withholds(kind: &str) -> bool {
    kind.starts_with("macro_") && kind != "fragment_macro" || kind == "footnote_undefined"
}

/// How every page renders. A top-level headline is an `h2`: the page title
/// is the page's one `h1`, written by the chrome (P-17), as ox-html's
/// `org-html-toplevel-hlevel` 2 does.
pub(crate) fn options() -> RenderOptions {
    let below_the_title = mae_org::ToplevelHlevel::new(2).expect("2 is a heading level");
    RenderOptions::default()
        .with_headline_ids(HeadlineIds::Explicit)
        .with_toplevel_hlevel(below_the_title)
}

/// Derive every page of one KB, with no operator options.
pub fn derive(sources: &[SourcePage], addressing: &Addressing) -> Site {
    derive_with(sources, addressing, &SiteOptions::default())
}

/// Derive every page of one KB under the operator's options: a full
/// derivation, a fresh [`crate::incremental::Deriver`]'s.
pub fn derive_with(sources: &[SourcePage], addressing: &Addressing, opts: &SiteOptions) -> Site {
    crate::incremental::Deriver::default().derive(sources, addressing, opts)
}

/// The lists derived from every page at once: each tag's pages A to Z, and
/// each index entry's outgoing links to published pages (the backlinks,
/// inverted), with the index in ID order.
pub(crate) fn finish_lists(site: &mut Site) {
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

pub(crate) fn scrub_nodes(nodes: &mut [mae_org::Node], hidden: &[String]) {
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

/// A page's headings in document order, from the rendered tree, so every
/// anchor is an id the page really has (P-79). A heading inside an unrendered
/// subtree is not in the tree, so not in the TOC.
pub(crate) fn toc(body: &[mae_org::Node]) -> Vec<TocEntry> {
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
pub(crate) fn page_tags(sem: &mae_org::Semantics) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in &sem.file_tags {
        if valid_tag(t) && !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

/// The file node's properties but its `ID` (see [`DerivedPage::properties`]).
pub(crate) fn file_properties(
    sem: &mae_org::Semantics,
    hidden: &[String],
) -> BTreeMap<String, String> {
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
pub(crate) fn dates_of(properties: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    DATE_PROPERTIES
        .iter()
        .filter_map(|(key, name)| properties.get(*key).map(|v| (name.to_string(), v.clone())))
        .collect()
}
