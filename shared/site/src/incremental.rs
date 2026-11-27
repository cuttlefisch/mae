//! Incremental derivation (ADR-119 D6): a [`Deriver`] keeps, between
//! derivations of one KB, what each page's derivation read and produced, and
//! re-derives only what changed. A fresh `Deriver` is the full derivation:
//! there is one code path, so the two can only differ by what is reused.
//!
//! **What is kept per page.**
//! - Its *summary*: what the first render and the semantics say about it (its
//!   ID, title, nodes and their anchors, `CUSTOM_ID`s, the targets it links,
//!   whether it renders faithfully). The summary is reused while the page's
//!   text and the hidden namespaces are unchanged. The KB-wide lists (which
//!   pages publish, the resolver, the backlinks) are rebuilt from summaries
//!   on every derivation: they are cheap.
//! - Its *output* (the page, its index entry, its plain blocks), reused while
//!   its fingerprint is unchanged.
//!
//! **Correct by construction.** A page's second render is given a resolver
//! that holds only the entries for the targets its links and transclusions
//! name: the page-local resolver. Its fingerprint hashes exactly that
//! resolver, the page's attachments, its backlinks, its address, its text
//! and the context. The render cannot read anything the fingerprint does not
//! cover, because nothing else is handed to it.
//!
//! @ai-caution: [architecture] Anything new a page's output reads must be
//! added to [`page_fingerprint`], or reused output goes stale. The property
//! test (`incremental_tests`) compares a warm `Deriver` with a fresh one after
//! every random edit; extend its edit set with the new input.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use mae_org::{
    parse, render, semantics, AttachmentState, NodeOrigin, ParseOptions, Resolved, Resolver,
};

use crate::address::{export_collision_key, is_reserved, valid_id, Addressing};
use crate::derive::{
    dates_of, file_properties, finish_lists, options, page_tags, scrub, scrub_nodes, toc,
    withholds, Backlink, DerivedPage, IndexEntry, PageRef, Site, SiteOptions, SourcePage, Withheld,
};
use crate::highlight::{highlight_blocks, PlainBlock};
use crate::sanitize::sanitize_fragment;

/// What the first render and the semantics say about one page.
struct Summary {
    /// The key: the summary holds while these are unchanged.
    text: String,
    hidden: Vec<String>,
    origin: String,
    id: Option<String>,
    title: Option<String>,
    /// Every node of the page (`sem.nodes`), with its title.
    nodes: Vec<(String, Option<String>)>,
    /// The targets of its `id:` links, for backlinks.
    kb_links: Vec<String>,
    /// Every target its second render may look up: its links' targets and
    /// its transclusions' members. The page-local resolver holds these.
    reads: BTreeSet<String>,
    /// The render diagnostic that withholds it, if any.
    unfaithful: Option<&'static str>,
    custom_ids: Vec<(String, String)>,
    /// Each node's anchor on the page, from the first render.
    anchors: Vec<(String, Option<String>)>,
}

fn summarize(source: &SourcePage, opts: &SiteOptions) -> Summary {
    let doc = parse(&source.text, &ParseOptions::default());
    let sem = semantics(&doc);
    let file = sem.nodes.iter().find(|n| n.origin == NodeOrigin::File);
    let id = file.map(|n| n.id.clone());
    let title = file
        .and_then(|n| n.title.clone())
        .map(|t| scrub(&t, &opts.hidden_namespaces).unwrap_or(t));
    let first = render(&doc, &sem, &options(), &Resolver::default());
    let unfaithful = first
        .diagnostics
        .iter()
        .find(|d| withholds(d.kind_name()))
        .map(|d| d.kind_name());
    let reads = sem
        .links
        .iter()
        .map(|l| l.target.clone())
        .chain(sem.transclusions.iter().map(|t| t.member.clone()))
        .collect();
    Summary {
        text: source.text.clone(),
        hidden: opts.hidden_namespaces.clone(),
        origin: source.origin.clone(),
        id,
        title,
        nodes: sem
            .nodes
            .iter()
            .map(|n| (n.id.clone(), n.title.clone()))
            .collect(),
        kb_links: sem
            .links
            .iter()
            .filter(|l| l.kind == mae_org::LinkKind::Kb)
            .map(|l| l.target.clone())
            .collect(),
        reads,
        unfaithful,
        custom_ids: first.custom_ids,
        anchors: first.nodes.into_iter().map(|n| (n.id, n.anchor)).collect(),
    }
}

/// One page's derived output.
struct PageOut {
    page: DerivedPage,
    index: IndexEntry,
    plain: Vec<PlainBlock>,
}

/// What one KB's derivations keep between runs. [`Default`] is empty: the
/// next derivation is a full one.
#[derive(Default)]
pub struct Deriver {
    /// By the source's origin.
    summaries: HashMap<String, Arc<Summary>>,
    /// By page ID: the fingerprint it was derived under, and its output.
    outputs: HashMap<String, (u128, Arc<PageOut>)>,
    /// How many pages the last derivation rendered (not reused).
    rendered: usize,
}

impl Deriver {
    /// How many pages the last [`Deriver::derive`] rendered rather than
    /// reused.
    pub fn last_rendered(&self) -> usize {
        self.rendered
    }

    /// Derive every page of one KB, reusing what is unchanged since the last
    /// call. Output is byte-identical to a fresh `Deriver`'s.
    pub fn derive(
        &mut self,
        sources: &[SourcePage],
        addressing: &Addressing,
        opts: &SiteOptions,
    ) -> Site {
        let summaries = self.summaries_for(sources, opts);
        let mut site = Site::default();
        let candidates = addressable(&summaries, addressing, &mut site.withheld);
        let faithful: Vec<usize> = candidates
            .into_iter()
            .filter(|&i| match summaries[i].unfaithful {
                Some(kind) => {
                    site.withheld.push(Withheld::Unfaithful {
                        id: summaries[i].id.clone().unwrap_or_default(),
                        kind,
                    });
                    false
                }
                None => true,
            })
            .collect();
        let resolver = resolver(&summaries, &faithful, addressing);
        let backlinks = backlinks(&summaries, &faithful, addressing);
        let context = context_hash(opts);
        let mut outputs = HashMap::with_capacity(faithful.len());
        // In the pages' order: the tag lists' sort is stable, so ties keep it.
        let mut ordered: Vec<Arc<PageOut>> = Vec::with_capacity(faithful.len());
        self.rendered = 0;
        for &i in &faithful {
            let s = &summaries[i];
            let id = s.id.clone().unwrap_or_default();
            let Some(href) = addressing.page_href(&id) else {
                continue;
            };
            let local = local_resolver(s, &sources[i], &resolver);
            let links = backlinks.get(&id).cloned().unwrap_or_default();
            let fp = page_fingerprint(context, &sources[i], &href, &local, &links);
            let out = match self.outputs.get(&id) {
                Some((f, out)) if *f == fp => Arc::clone(out),
                _ => {
                    self.rendered += 1;
                    Arc::new(render_page(
                        s,
                        &sources[i],
                        (&id, href),
                        &local,
                        links,
                        opts,
                    ))
                }
            };
            ordered.push(Arc::clone(&out));
            outputs.insert(id, (fp, out));
        }
        for out in &ordered {
            let page = out.page.clone();
            for t in &page.tags {
                site.tags.entry(t.clone()).or_default().push(PageRef {
                    title: page.title.clone(),
                    id: page.id.clone(),
                    href: page.href.clone(),
                });
            }
            site.index.push(out.index.clone());
            site.plain_blocks.extend(out.plain.iter().cloned());
            site.pages.insert(page.id.clone(), page);
        }
        finish_lists(&mut site);
        // Only what the KB still holds is kept.
        self.outputs = outputs;
        site
    }

    /// Each source's summary, reused when its text and the hidden namespaces
    /// are unchanged; the kept set becomes exactly these sources'.
    fn summaries_for(&mut self, sources: &[SourcePage], opts: &SiteOptions) -> Vec<Arc<Summary>> {
        let mut kept = HashMap::with_capacity(sources.len());
        let out = sources
            .iter()
            .map(|src| {
                let reuse = self
                    .summaries
                    .get(&src.origin)
                    .filter(|s| s.text == src.text && s.hidden == opts.hidden_namespaces);
                let s = match reuse {
                    Some(s) => Arc::clone(s),
                    None => Arc::new(summarize(src, opts)),
                };
                kept.insert(src.origin.clone(), Arc::clone(&s));
                s
            })
            .collect();
        self.summaries = kept;
        out
    }
}

/// Test only: every faithful page whose output differs when rendered against
/// the whole KB's resolver (as before D6) rather than its page-local one.
/// Empty means the page-local resolver changes nothing.
#[cfg(test)]
pub(crate) fn local_vs_global(
    sources: &[SourcePage],
    addressing: &Addressing,
    opts: &SiteOptions,
) -> Vec<String> {
    let summaries: Vec<Arc<Summary>> = sources
        .iter()
        .map(|s| Arc::new(summarize(s, opts)))
        .collect();
    let mut withheld = Vec::new();
    let faithful: Vec<usize> = addressable(&summaries, addressing, &mut withheld)
        .into_iter()
        .filter(|&i| summaries[i].unfaithful.is_none())
        .collect();
    let kb = resolver(&summaries, &faithful, addressing);
    let links = backlinks(&summaries, &faithful, addressing);
    let mut differ = Vec::new();
    for &i in &faithful {
        let s = &summaries[i];
        let id = s.id.clone().unwrap_or_default();
        let Some(href) = addressing.page_href(&id) else {
            continue;
        };
        let bl = links.get(&id).cloned().unwrap_or_default();
        let local = local_resolver(s, &sources[i], &kb);
        let mut global = kb.clone();
        global.attachments = local.attachments.clone();
        let a = render_page(
            s,
            &sources[i],
            (&id, href.clone()),
            &local,
            bl.clone(),
            opts,
        );
        let b = render_page(s, &sources[i], (&id, href), &global, bl, opts);
        if a.page != b.page || a.index != b.index {
            differ.push(id);
        }
    }
    differ
}

/// The page's second render, against its page-local resolver.
fn render_page(
    s: &Summary,
    source: &SourcePage,
    (id, href): (&str, String),
    local: &Resolver,
    backlinks: Vec<Backlink>,
    opts: &SiteOptions,
) -> PageOut {
    let doc = parse(&source.text, &ParseOptions::default());
    let sem = semantics(&doc);
    let mut r = render(&doc, &sem, &options(), local);
    scrub_nodes(&mut r.body, &opts.hidden_namespaces);
    // After scrubbing: a block is highlighted as it is shown.
    let mut plain = BTreeSet::new();
    if let Some(h) = &opts.highlighter {
        highlight_blocks(&mut r.body, id, h.as_ref(), &mut plain);
    }
    let title = s.title.clone().unwrap_or_else(|| id.to_string());
    let properties = file_properties(&sem, &opts.hidden_namespaces);
    let index = IndexEntry {
        id: id.to_string(),
        title: title.clone(),
        href: href.clone(),
        tags: page_tags(&sem),
        links: Vec::new(),
        text: mae_org::text_of(&r.body),
    };
    let page = DerivedPage {
        title,
        href,
        html: sanitize_fragment(&mae_org::to_html(&r.body)),
        backlinks,
        tags: page_tags(&sem),
        toc: toc(&r.body),
        dates: dates_of(&properties),
        properties,
        id: id.to_string(),
    };
    PageOut {
        page,
        index,
        plain: plain.into_iter().collect(),
    }
}

/// The KB resolver's entries for the targets `s` reads, and its own
/// attachments: everything its second render may look up.
fn local_resolver(s: &Summary, source: &SourcePage, kb: &Resolver) -> Resolver {
    let mut r = Resolver::default();
    for t in &s.reads {
        if let Some(e) = kb.targets.get(t) {
            r.targets.insert(t.clone(), e.clone());
        }
        // A page's attachments are its own: NAMEs are per node.
        if let Some(name) = t.strip_prefix("attachment:") {
            let state = source
                .attachments
                .get(name)
                .cloned()
                .unwrap_or(AttachmentState::Missing);
            r.attachments.insert(t.clone(), state);
        }
    }
    r
}

/// The global inputs: the core's version and the operator's options.
fn context_hash(opts: &SiteOptions) -> u128 {
    hash128(|h| {
        env!("CARGO_PKG_VERSION").hash(h);
        opts.hidden_namespaces.hash(h);
        opts.highlighter.is_some().hash(h);
    })
}

/// A hash of everything a page's second render and its output read.
fn page_fingerprint(
    context: u128,
    source: &SourcePage,
    href: &str,
    local: &Resolver,
    backlinks: &[Backlink],
) -> u128 {
    hash128(|h| {
        context.hash(h);
        source.text.hash(h);
        href.hash(h);
        let mut targets: Vec<(&String, &Resolved)> = local.targets.iter().collect();
        targets.sort_by(|a, b| a.0.cmp(b.0));
        for (k, e) in targets {
            (k, &e.href, &e.title, &e.custom_ids).hash(h);
        }
        let mut attachments: Vec<(&String, String)> = local
            .attachments
            .iter()
            .map(|(k, v)| (k, format!("{v:?}")))
            .collect();
        attachments.sort();
        attachments.hash(h);
        local.placeholder.hash(h);
        for b in backlinks {
            (&b.title, &b.id, &b.href).hash(h);
        }
    })
}

/// Two independent 64-bit SipHashes of the same input: a 128-bit key, so a
/// collision serving another page's output is not a practical risk.
fn hash128(feed: impl Fn(&mut DefaultHasher)) -> u128 {
    let mut a = DefaultHasher::new();
    0u8.hash(&mut a);
    feed(&mut a);
    let mut b = DefaultHasher::new();
    1u8.hash(&mut b);
    feed(&mut b);
    (u128::from(a.finish()) << 64) | u128::from(b.finish())
}

/// The pages that may publish, before rendering: a valid file ID, no node ID
/// shared with any other node of the KB, and (in the export) no file ID in
/// the same directory as another's.
fn addressable(
    summaries: &[Arc<Summary>],
    addressing: &Addressing,
    withheld: &mut Vec<Withheld>,
) -> Vec<usize> {
    let mut ok: Vec<usize> = Vec::new();
    for (i, s) in summaries.iter().enumerate() {
        match &s.id {
            None => withheld.push(Withheld::NoFileId {
                origin: s.origin.clone(),
            }),
            Some(id) if !valid_id(id) => withheld.push(Withheld::InvalidId {
                origin: s.origin.clone(),
                id: id.clone(),
            }),
            Some(id) if is_reserved(id) => withheld.push(Withheld::Reserved {
                origin: s.origin.clone(),
                id: id.clone(),
            }),
            Some(_) => ok.push(i),
        }
    }
    // Every node ID, file-level or not, once in the KB.
    let mut holders: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &i in &ok {
        for (n, _) in &summaries[i].nodes {
            holders.entry(n.as_str()).or_default().push(i);
        }
    }
    let mut out_dup = vec![false; summaries.len()];
    for (id, at) in &holders {
        if at.len() > 1 {
            let origins = at.iter().map(|&i| summaries[i].origin.clone()).collect();
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
                summaries[i].id.as_deref().unwrap_or_default(),
            ))
            .or_default()
            .push(i);
        }
        let mut out = vec![false; summaries.len()];
        for at in dirs.values().filter(|at| at.len() > 1) {
            let ids = at
                .iter()
                .map(|&i| summaries[i].id.clone().unwrap_or_default())
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

/// Who links to whom, over the pages that publish only (P-36): a link to any
/// node of page Q is a link to Q, a page never backlinks itself, and a link
/// from a withheld page counts for nothing. mae-org lists only the links in
/// rendered regions, so a `:noexport:` subtree's links do not count either.
fn backlinks(
    summaries: &[Arc<Summary>],
    faithful: &[usize],
    addressing: &Addressing,
) -> BTreeMap<String, Vec<Backlink>> {
    let mut page_of: HashMap<&str, &str> = HashMap::new();
    for &i in faithful {
        let s = &summaries[i];
        let Some(id) = s.id.as_deref() else { continue };
        for (n, _) in &s.nodes {
            page_of.insert(n.as_str(), id);
        }
    }
    let mut out: BTreeMap<String, BTreeSet<Backlink>> = BTreeMap::new();
    for &i in faithful {
        let s = &summaries[i];
        let Some(from) = s.id.as_deref() else {
            continue;
        };
        let Some(href) = addressing.page_href(from) else {
            continue;
        };
        let me = Backlink {
            title: s.title.clone().unwrap_or_else(|| from.to_string()),
            id: from.to_string(),
            href,
        };
        for target in &s.kb_links {
            if let Some(&to) = page_of.get(target.as_str()) {
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

/// The KB's one resolver, over the pages that will publish: every page, every
/// node on it at its anchor, and each page's CUSTOM_IDs (S0). A heading's
/// UUID `CUSTOM_ID` is an ID too (D5), unless a node holds that ID or two
/// pages share the UUID: then it resolves nowhere rather than by order.
fn resolver(summaries: &[Arc<Summary>], faithful: &[usize], addressing: &Addressing) -> Resolver {
    let mut r = Resolver::default();
    let mut uuid_cids: HashMap<String, Option<Resolved>> = HashMap::new();
    for &i in faithful {
        let s = &summaries[i];
        let Some(id) = &s.id else { continue };
        let Some(href) = addressing.page_href(id) else {
            continue;
        };
        let mut page =
            Resolved::new(href.clone()).with_title(s.title.clone().unwrap_or_else(|| id.clone()));
        for (c, a) in &s.custom_ids {
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
        for (n, anchor) in &s.anchors {
            if n == id {
                continue;
            }
            let at = match anchor {
                Some(a) => format!("{href}#{a}"),
                None => href.clone(),
            };
            let title = s
                .nodes
                .iter()
                .find(|(x, _)| x == n)
                .and_then(|(_, t)| t.clone());
            let mut t = Resolved::new(at);
            if let Some(title) = title {
                t = t.with_title(title);
            }
            r = r.with_target(n.clone(), t);
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

#[cfg(test)]
#[path = "incremental_tests.rs"]
mod tests;
