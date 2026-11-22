//! One KB, derived into pages (ADR-119 D5).
//!
//! Two renders per page, both by mae-org. The first, against an empty
//! resolver, finds where every node and `CUSTOM_ID` of the page landed. Those
//! go into ONE resolver for the KB. The second renders each page against it,
//! and its HTML passes the final sanitiser.

use std::collections::{BTreeMap, HashMap};

use mae_org::{
    parse, render, semantics, HeadlineIds, NodeOrigin, ParseOptions, RenderOptions, Rendered,
    Resolved, Resolver,
};

use crate::address::{export_collision_key, Addressing};
use crate::sanitize::sanitize_fragment;

/// One page's source: an Org text with a file-level `:ID:`.
#[derive(Debug, Clone)]
pub struct SourcePage {
    /// Where it came from (a bundle path, a store key): named in diagnostics
    /// only, never part of an address (D4).
    pub origin: String,
    pub text: String,
}

/// A derived page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPage {
    /// The file-level `:ID:`.
    pub id: String,
    pub title: String,
    pub href: String,
    /// The sanitised article body.
    pub html: String,
}

/// Why a page was withheld (P-147, P-15). A withheld page publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Withheld {
    /// No file-level `:ID:`.
    NoFileId { origin: String },
    /// The same ID on several pages: none of them publishes.
    DuplicateId { id: String, origins: Vec<String> },
    /// IDs equal under case-folding: one directory on a case-insensitive
    /// filesystem, so the export cannot hold both (P-15).
    CaseCollision { ids: Vec<String> },
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
/// the author wrote (P-147).
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
    let parsed: Vec<Parsed> = sources
        .iter()
        .map(|source| {
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
        })
        .collect();
    let publishable = publishable(&parsed, addressing, &mut site.withheld);

    // Pass 1: where each page's nodes and CUSTOM_IDs land.
    let empty = Resolver::default();
    let first: Vec<(usize, Rendered)> = publishable
        .iter()
        .map(|&i| {
            (
                i,
                render(&parsed[i].doc, &parsed[i].sem, &options(), &empty),
            )
        })
        .collect();
    let resolver = resolver(&parsed, &first, addressing);

    // Pass 2: every page against the one resolver.
    for (i, _) in &first {
        let p = &parsed[*i];
        let id = p.id.clone().expect("publishable pages have an ID");
        let r = render(&p.doc, &p.sem, &options(), &resolver);
        if let Some(d) = r.diagnostics.iter().find(|d| withholds(d.kind_name())) {
            site.withheld.push(Withheld::Unfaithful {
                id,
                kind: d.kind_name(),
            });
            continue;
        }
        let html = sanitize_fragment(&mae_org::to_html(&r.body));
        let page = DerivedPage {
            title: p.title.clone().unwrap_or_else(|| id.clone()),
            href: addressing.page_href(&id),
            html,
            id: id.clone(),
        };
        site.pages.insert(id, page);
    }
    site
}

/// The pages that may publish: each has a file ID that no other page shares,
/// and (for the export) no other ID equal to it under case-folding.
fn publishable(
    parsed: &[Parsed],
    addressing: &Addressing,
    withheld: &mut Vec<Withheld>,
) -> Vec<usize> {
    let mut by_id: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, p) in parsed.iter().enumerate() {
        match &p.id {
            Some(id) => by_id.entry(id.as_str()).or_default().push(i),
            None => withheld.push(Withheld::NoFileId {
                origin: p.source.origin.clone(),
            }),
        }
    }
    let mut ok = Vec::new();
    let mut folded: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (id, at) in &by_id {
        if at.len() > 1 {
            let origins = at
                .iter()
                .map(|&i| parsed[i].source.origin.clone())
                .collect();
            withheld.push(Withheld::DuplicateId {
                id: id.to_string(),
                origins,
            });
        } else {
            folded.entry(export_collision_key(id)).or_default().push(id);
        }
    }
    let export = matches!(addressing, Addressing::Export { .. });
    for ids in folded.values() {
        if export && ids.len() > 1 {
            withheld.push(Withheld::CaseCollision {
                ids: ids.iter().map(|s| s.to_string()).collect(),
            });
            continue;
        }
        ok.extend(ids.iter().map(|id| by_id[id][0]));
    }
    ok.sort_unstable();
    ok
}

/// The KB's one resolver: every page, every node on it at its anchor, and
/// each page's CUSTOM_IDs (S0). A heading's UUID `CUSTOM_ID` is an ID too
/// (D5), unless a node already holds that ID.
fn resolver(parsed: &[Parsed], first: &[(usize, Rendered)], addressing: &Addressing) -> Resolver {
    let mut r = Resolver::default();
    let mut uuid_cids: HashMap<String, Resolved> = HashMap::new();
    for (i, rendered) in first {
        let p = &parsed[*i];
        let Some(id) = &p.id else { continue };
        let href = addressing.page_href(id);
        let mut page =
            Resolved::new(href.clone()).with_title(p.title.clone().unwrap_or_else(|| id.clone()));
        for (c, a) in &rendered.custom_ids {
            page = page.with_custom_id(c.clone(), a.clone());
            if is_uuid(c) {
                uuid_cids.insert(c.clone(), Resolved::new(format!("{href}#{a}")));
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
        if !r.targets.contains_key(&c) {
            r = r.with_target(c, t);
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
