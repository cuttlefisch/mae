//! One KB, derived into pages (ADR-119 D5).
//!
//! Two renders per page, both by mae-org. The first, against an empty
//! resolver, finds where every node and `CUSTOM_ID` of the page landed, and
//! whether the page can be rendered faithfully at all. The faithful pages'
//! nodes go into ONE resolver for the KB, so a link never points at a page
//! that does not publish. The second render is each such page against it,
//! and its HTML passes the final sanitiser.

use std::collections::{BTreeMap, HashMap};

use mae_org::{
    parse, render, semantics, HeadlineIds, NodeOrigin, ParseOptions, RenderOptions, Rendered,
    Resolved, Resolver,
};

use crate::address::{export_collision_key, valid_id, Addressing};
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
}

/// A page that links to another. `title` is raw text: the chrome escapes it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Backlink {
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
}

/// The render diagnostics that withhold a page: its output would not be what
/// the author wrote (P-147). None depends on the resolver, so the first
/// render decides them.
fn withholds(kind: &str) -> bool {
    kind.starts_with("macro_") && kind != "fragment_macro" || kind == "footnote_undefined"
}

fn options() -> RenderOptions {
    RenderOptions::default().with_headline_ids(HeadlineIds::Explicit)
}

struct Parsed<'a> {
    source: &'a SourcePage,
    doc: mae_org::Document,
    sem: mae_org::Semantics,
    id: Option<String>,
    title: Option<String>,
}

/// Derive every page of one KB.
pub fn derive(sources: &[SourcePage], addressing: &Addressing) -> Site {
    let mut site = Site::default();
    let parsed: Vec<Parsed> = sources.iter().map(parse_page).collect();
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
        let r = render(&p.doc, &p.sem, &options(), &resolver);
        let page = DerivedPage {
            title: p.title.clone().unwrap_or_else(|| id.clone()),
            href,
            html: sanitize_fragment(&mae_org::to_html(&r.body)),
            backlinks: backlinks.get(&id).cloned().unwrap_or_default(),
            id: id.clone(),
        };
        site.pages.insert(id, page);
    }
    site
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

fn parse_page(source: &SourcePage) -> Parsed<'_> {
    let doc = parse(&source.text, &ParseOptions::default());
    let sem = semantics(&doc);
    let file = sem.nodes.iter().find(|n| n.origin == NodeOrigin::File);
    let id = file.map(|n| n.id.clone());
    let title = file.and_then(|n| n.title.clone());
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
