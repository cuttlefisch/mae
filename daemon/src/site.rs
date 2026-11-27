//! The live KB site (ADR-119 L1a): one page of a KB, read-only, at
//! `/kb/<kb-id>/<id>` on the OAuth listener.
//!
//! Every answer that is not a page is the same [`SitePage::NotFound`]: a KB
//! that does not exist, one the caller may not read, an end-to-end-encrypted
//! KB (L1 serves plaintext KBs only), a node that does not exist, and a page
//! the derivation withheld. So the route is no oracle for which KBs or nodes
//! exist (D7).
//!
//! The read gate is `kb_query::load_gated`, the one `kb/query.*` uses, so the
//! site and the query surface can never disagree about who may read a KB
//! (principle #8). The page itself comes from `mae_site::derive`, the core the
//! static export shares.
//!
//! One timing difference remains by design: a member asking for a missing
//! node waits for a derivation, while a non-member is refused at the gate. It
//! tells a caller only whether THEY may read the KB, which they know anyway.
//!
//! A KB is derived whole, bounded (D21) by [`SiteLimits`] and off the async
//! workers (`spawn_blocking`), once per change: later requests are answered
//! from the derivation cache (`site_cache`, D6's first step). The read gate
//! still runs on every request, before the cache is asked. Incremental
//! re-derivation per node is the rest of D6.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::doc_store::DocStore;
use crate::kb_query;
use crate::site_cache::{DerivationKey, Derived};
use crate::site_components::{KbComponents, Slots};
use mae_org::{AttachmentState, ResolvedAttachment};
use mae_site::components::{PageData, Slot};
use mae_site::{Addressing, SourcePage};
use mae_sync::kb::{AttachmentRef, Encryption, KbCollectionDoc, KbNodeDoc};

/// What the route answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitePage {
    /// A page: its title (raw text), its sanitised article HTML, its
    /// backlinks, its tags as `(tag, href)`, and where its explorer loads.
    Found {
        title: String,
        html: String,
        backlinks: Vec<mae_site::Backlink>,
        tags: Vec<(String, String)>,
        toc: Vec<mae_site::TocEntry>,
        explorer: Option<ExplorerRef>,
        /// The operator's components rendered for this page; `None` when
        /// the KB has none.
        slots: Option<Slots>,
    },
    /// A page a component emits (H2): its title (raw text) and its
    /// sanitised HTML.
    Component {
        title: String,
        html: String,
        explorer: Option<ExplorerRef>,
        slots: Option<Slots>,
    },
    /// The content index (P-77): one entry per published page.
    Index(Vec<mae_site::IndexEntry>),
    /// A listing the engine generates: the tag index or one tag's pages, as
    /// `(text, href)` entries.
    Listing {
        kind: Listing,
        items: Vec<(String, String)>,
        explorer: Option<ExplorerRef>,
        slots: Option<Slots>,
    },
    /// The explorer's data (P-39, Amendment 3), as the JSON the browser
    /// runtime reads.
    Explorer(String),
    /// Every refusal and every absence, alike.
    NotFound,
    /// The KB is larger than this listener derives per request.
    TooLarge,
}

/// Where a page's explorer loads its data from, and which version of it this
/// page was derived with: the browser runtime fetches it once and again only
/// when a page names another version (D6: chrome is not embedded in pages).
/// It lists titles only, which every reader of the KB may have (ADR-119
/// Amendment 3 G2; `kb/query.titles` gives the same).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerRef {
    pub src: String,
    /// The first 16 hex digits of the SHA-256 of [`explorer_json`]'s output.
    pub version: String,
}

/// The explorer as the browser runtime reads it:
/// `{"groups":[{"tag","pages":[{"title","href"}]}],"untagged":[{"title","href"}]}`,
/// in display order. Titles are raw text; the runtime writes them with
/// `textContent`, never as markup.
pub fn explorer_json(ex: &mae_site::explorer::Explorer) -> String {
    let pages = |ps: &[mae_site::PageRef]| -> Vec<serde_json::Value> {
        ps.iter()
            .map(|p| serde_json::json!({ "title": p.title, "href": p.href }))
            .collect()
    };
    let groups: Vec<serde_json::Value> = ex
        .groups
        .iter()
        .map(|g| serde_json::json!({ "tag": g.tag, "pages": pages(&g.pages) }))
        .collect();
    serde_json::json!({ "groups": groups, "untagged": pages(&ex.untagged) }).to_string()
}

fn explorer_ref(json: &str, addressing: &Addressing) -> ExplorerRef {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(json.as_bytes());
    ExplorerRef {
        src: addressing.explorer_href(),
        version: hex::encode(&digest[..8]),
    }
}

/// Which generated listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Every tag (P-71).
    Tags,
    /// The pages of one tag (P-70).
    Tag(String),
}

/// What a request asks for under `/kb/<kb-id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// `/kb/<kb-id>/<id>`.
    Page(String),
    /// `/kb/<kb-id>/tags`.
    Tags,
    /// `/kb/<kb-id>/tags/<tag>`.
    Tag(String),
    /// `/kb/<kb-id>/index.json`.
    Index,
    /// `/kb/<kb-id>/explorer.json`.
    Explorer,
    /// `/kb/<kb-id>/`: the KB's home page, the node the operator declared
    /// for it (ADR-119 Amendment 2 B3).
    Home(Option<String>),
    /// `/kb/<kb-id>/site/<name>`: a page a component emits (Amendment 4 H2).
    Site(String),
}

/// The operator's components for one KB, and the locale they render in.
#[derive(Debug, Clone)]
pub struct ComponentsFor {
    pub components: Arc<KbComponents>,
    pub locale: String,
}

/// What one page request may read (D21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteLimits {
    /// Nodes in the KB: more, and the KB is [`SitePage::TooLarge`].
    pub max_scan_nodes: usize,
    /// Bytes of one node's title, of its body, or of its properties (keys and
    /// values together, which now reach the page source): more, and the KB is
    /// too large.
    pub max_body_bytes: usize,
}

/// The page `id` of `kb_id`, as `principal` may read it.
pub async fn page(
    doc_store: &DocStore,
    kb_id: &str,
    id: &str,
    principal: Option<&str>,
    limits: SiteLimits,
) -> SitePage {
    view(
        doc_store,
        kb_id,
        &View::Page(id.to_string()),
        principal,
        limits,
        &mae_site::SiteOptions::default(),
        None,
    )
    .await
}

/// One view of `kb_id`, as `principal` may read it, with the KB's
/// `components` in its slots. A component page of a KB with no components is
/// the same not-found as every other refusal, answered before any read.
pub async fn view(
    doc_store: &DocStore,
    kb_id: &str,
    view: &View,
    principal: Option<&str>,
    limits: SiteLimits,
    opts: &mae_site::SiteOptions,
    components: Option<ComponentsFor>,
) -> SitePage {
    if matches!(view, View::Site(_)) && components.is_none() {
        return SitePage::NotFound;
    }
    let Ok(addressing) = Addressing::live(kb_id) else {
        return SitePage::NotFound;
    };
    // Before any read: what is read below is at least this new (`site_cache`).
    let key = derivation_key(doc_store, kb_id, limits, opts);
    let (coll, may_copy) = match gated(doc_store, kb_id, principal).await {
        Ok(v) => v,
        Err(refused) => return refused.into(),
    };
    let derived = doc_store
        .site_cache()
        .get_or_derive(kb_id, key, |deriver| {
            derive_kb(
                doc_store,
                kb_id,
                coll,
                (addressing.clone(), limits),
                (opts, deriver),
            )
        })
        .await;
    match *derived {
        Derived::Site(_) => {}
        Derived::TooLarge => return SitePage::TooLarge,
        Derived::Failed => return SitePage::NotFound,
    }
    // Rendering is CPU work: off the async workers.
    let view = view.clone();
    let answered = tokio::task::spawn_blocking(move || match &*derived {
        Derived::Site(site) => answer(site, &addressing, view, may_copy, components.as_ref()),
        Derived::TooLarge => SitePage::TooLarge,
        Derived::Failed => SitePage::NotFound,
    })
    .await;
    answered.unwrap_or(SitePage::NotFound)
}

/// What a derivation of `kb_id` reads, besides its content (`site_cache`).
fn derivation_key(
    doc_store: &DocStore,
    kb_id: &str,
    limits: SiteLimits,
    opts: &mae_site::SiteOptions,
) -> DerivationKey {
    DerivationKey {
        generation: doc_store.kb_generation(kb_id),
        blob_changes: doc_store.blob_store().map_or(0, |b| b.changes()),
        hidden_namespaces: opts.hidden_namespaces.clone(),
        highlighted: opts.highlighter.is_some(),
        max_scan_nodes: limits.max_scan_nodes,
        max_body_bytes: limits.max_body_bytes,
    }
}

/// Read and derive the whole of `kb_id` from `coll`: the cache's one way in.
async fn derive_kb(
    doc_store: &DocStore,
    kb_id: &str,
    coll: KbCollectionDoc,
    (addressing, limits): (Addressing, SiteLimits),
    (opts, deriver): (&mae_site::SiteOptions, crate::site_cache::SharedDeriver),
) -> Derived {
    let read = std::time::Instant::now();
    let sources = match readable_sources(doc_store, kb_id, &coll, limits).await {
        Ok(sources) => sources,
        Err(Refused::Unreadable) => return Derived::Failed,
        Err(_) => return Derived::TooLarge,
    };
    let read_ms = read.elapsed().as_millis();
    let (opts, kb_id) = (opts.clone(), kb_id.to_string());
    let started = std::time::Instant::now();
    let derived = tokio::task::spawn_blocking(move || {
        // A panic in an earlier derivation leaves nothing worth reusing.
        let mut d = deriver.lock().unwrap_or_else(|poisoned| {
            let mut d = poisoned.into_inner();
            *d = Default::default();
            d
        });
        let site = d.derive(&sources, &addressing, &opts);
        // P-108: never fatal, but said, once per derivation.
        for line in crate::site_highlight::plain_block_lines(&kb_id, &site.plain_blocks) {
            tracing::info!("{line}");
        }
        (site, kb_id, sources.len(), d.last_rendered())
    })
    .await;
    match derived {
        Ok((site, kb_id, pages, rendered)) => {
            let ms = started.elapsed().as_millis();
            doc_store.site_cache().note_rendered(rendered);
            tracing::info!(kb_id = %kb_id, pages, rendered, read_ms, ms, "site: derived");
            Derived::Site(site)
        }
        Err(e) => {
            tracing::warn!(error = %e, "site: derivation stopped");
            Derived::Failed
        }
    }
}

/// Why [`readable_sources`] gave no sources: the answer it stands for.
enum Refused {
    NotFound,
    TooLarge,
    /// A listed node's document could not be read for a reason other than
    /// its absence: the derivation fails, and is not held (`site_cache`).
    Unreadable,
}

impl From<Refused> for SitePage {
    fn from(r: Refused) -> Self {
        match r {
            Refused::NotFound => SitePage::NotFound,
            Refused::TooLarge => SitePage::TooLarge,
            Refused::Unreadable => SitePage::NotFound,
        }
    }
}

/// The KB's collection, once the read gate admits `principal` to a plaintext
/// KB, and whether `principal` may have the whole KB's content at once (D7,
/// G2); otherwise the answer to give. Every request, cached or not.
async fn gated(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
) -> Result<(KbCollectionDoc, bool), Refused> {
    let Ok((coll, encryption)) = kb_query::load_gated(doc_store, kb_id, principal).await else {
        return Err(Refused::NotFound);
    };
    if !matches!(encryption, Encryption::None) {
        return Err(Refused::NotFound);
    }
    if !review_off(doc_store, kb_id).await {
        return Err(Refused::NotFound);
    }
    let may_copy = kb_query::may_copy_whole_kb(doc_store, kb_id, &coll, principal).await;
    Ok((coll, may_copy))
}

/// ADR-119 D8: the live app serves a KB with publication review off only.
/// A review-enabled KB's pages are its working copy, drafts included, so it
/// is the same not-found as every refusal until the published view (L3)
/// derives it from in-force revisions. Checked on every request, before the
/// cache, so switching review on hides a KB at once. A policy that cannot be
/// read hides it too.
async fn review_off(doc_store: &DocStore, kb_id: &str) -> bool {
    let kb = kb_id.to_string();
    let enabled = doc_store
        .review_read(move |c| crate::review::store::policy(c, &kb).map(|p| p.review_enabled))
        .await;
    match enabled {
        Ok(on) => !on,
        Err(e) => {
            tracing::warn!(kb_id, error = %e, "site: review policy unreadable; KB not served");
            false
        }
    }
}

/// The pages of `coll`, KB `kb_id`'s collection, as sources, when the KB is
/// within `limits`.
async fn readable_sources(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    limits: SiteLimits,
) -> Result<Vec<SourcePage>, Refused> {
    let nodes = coll.list_nodes();
    if nodes.len() > limits.max_scan_nodes {
        return Err(Refused::TooLarge);
    }
    let mut sources = Vec::with_capacity(nodes.len());
    let mut entries = Vec::with_capacity(nodes.len());
    for (node_id, _) in nodes {
        let name = mae_sync::kb_node_doc_name(kb_id, &node_id);
        // Absent is the KB's state, held like any other; any other read error
        // may pass, so this derivation is not held.
        let state = match doc_store.encode_state_and_sv(&name).await {
            Ok((state, _sv)) => state,
            Err(crate::storage::StorageError::DurableDocMissing(_)) => continue,
            Err(e) => {
                tracing::warn!(kb_id = %kb_id, node_id = %node_id, error = %e, "site: a node could not be read");
                return Err(Refused::Unreadable);
            }
        };
        let Ok(doc) = KbNodeDoc::from_bytes(&state) else {
            continue;
        };
        let (title, body) = (doc.title(), doc.body());
        let props: usize = doc
            .properties()
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        if [title.len(), body.len(), props]
            .iter()
            .any(|n| *n > limits.max_body_bytes)
        {
            return Err(Refused::TooLarge);
        }
        if let Some(text) = page_source(&node_id, &doc) {
            entries.push(doc.attachments());
            sources.push(SourcePage {
                attachments: Default::default(),
                origin: node_id,
                text,
            });
        }
    }
    let states = attachment_states(doc_store, kb_id, entries).await;
    for (source, states) in sources.iter_mut().zip(states) {
        source.attachments = states;
    }
    Ok(sources)
}

/// Each page's attachments as the page shows them (ADR-115 E3): a NAME whose
/// blob the KB's store holds resolves to its route, `/kb/<kb_id>/blobs/
/// <sha256>`, which authorizes the reader again; any other is missing (no
/// attachment store, a blob gone or never stored). One blocking task asks the
/// store about every distinct blob, off the async workers.
async fn attachment_states(
    doc_store: &DocStore,
    kb_id: &str,
    entries: Vec<BTreeMap<String, AttachmentRef>>,
) -> Vec<BTreeMap<String, AttachmentState>> {
    let shas: BTreeSet<String> = entries
        .iter()
        .flat_map(|m| m.values().map(|a| a.sha256.clone()))
        .collect();
    let held: HashMap<String, crate::blob_store::BlobMeta> = match doc_store.blob_store() {
        Some(blobs) if !shas.is_empty() => {
            let (blobs, kb) = (Arc::clone(blobs), kb_id.to_string());
            tokio::task::spawn_blocking(move || {
                shas.into_iter()
                    .filter_map(|sha| match blobs.get(&kb, &sha) {
                        Ok(meta) => meta.map(|m| (sha, m)),
                        // Shown as missing, and said here: never silent.
                        Err(e) => {
                            tracing::warn!(kb_id = %kb, sha256 = %sha, error = %e, "site: attachment store unreadable");
                            None
                        }
                    })
                    .collect()
            })
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "site: attachment lookup stopped");
                HashMap::new()
            })
        }
        _ => HashMap::new(),
    };
    entries
        .into_iter()
        .map(|m| {
            m.into_iter()
                .map(|(name, a)| {
                    let state = match held.get(&a.sha256) {
                        Some(meta) => {
                            let mut file = ResolvedAttachment::new(
                                kb_query::blob_url(kb_id, &a.sha256),
                                meta.media_type.as_str(),
                            );
                            if let (Some(w), Some(h)) = (meta.width, meta.height) {
                                file = file.with_width(w).with_height(h);
                            }
                            AttachmentState::Resolved(file)
                        }
                        None => AttachmentState::Missing,
                    };
                    (name, state)
                })
                .collect()
        })
        .collect()
}

/// The answer to `view` from a derived KB. A reader who may not have the
/// whole KB's content at once (`may_copy`, a query-only member, D7 and G2)
/// is refused the content index, which holds every page's text, as the one
/// not-found. Everything else is theirs: pages one at a time, and the
/// explorer and tag listings, which hold titles only. Every page and listing
/// carries `components` rendered in its slots.
fn answer(
    site: &mae_site::Site,
    addressing: &Addressing,
    view: View,
    may_copy: bool,
    components: Option<&ComponentsFor>,
) -> SitePage {
    let explorer = || {
        let json = explorer_json(&mae_site::explorer::explorer(site));
        Some(explorer_ref(&json, addressing))
    };
    let slots = |page: PageData| slots_for(components, site, addressing, page);
    match view {
        View::Index if !may_copy => SitePage::NotFound,
        // The declared node when this reader has it as a page; otherwise the
        // tag index. Unpublished, withheld and undeclared all answer alike, so
        // the home address says nothing about a node the reader cannot see.
        View::Home(declared) => match declared.filter(|id| site.pages.contains_key(id)) {
            Some(id) => answer(site, addressing, View::Page(id), may_copy, components),
            None => answer(site, addressing, View::Tags, may_copy, components),
        },
        View::Page(id) => match site.pages.get(&id) {
            Some(p) => SitePage::Found {
                title: p.title.clone(),
                html: p.html.clone(),
                backlinks: p.backlinks.clone(),
                tags: p
                    .tags
                    .iter()
                    .map(|t| (t.clone(), addressing.tag_href(t)))
                    .collect(),
                toc: p.toc.clone(),
                explorer: explorer(),
                slots: slots(page_data(p)),
            },
            None => SitePage::NotFound,
        },
        View::Tags => SitePage::Listing {
            kind: Listing::Tags,
            items: site
                .tags_in_order()
                .into_iter()
                .map(|(t, _)| (t.to_string(), addressing.tag_href(t)))
                .collect(),
            explorer: explorer(),
            slots: slots(generated_page("tags", addressing.tags_href())),
        },
        View::Index => SitePage::Index(site.index.clone()),
        View::Explorer => SitePage::Explorer(explorer_json(&mae_site::explorer::explorer(site))),
        View::Tag(tag) => match site.tags.get(&tag) {
            Some(pages) => SitePage::Listing {
                slots: slots(generated_page(&tag, addressing.tag_href(&tag))),
                kind: Listing::Tag(tag),
                items: pages
                    .iter()
                    .map(|p| (p.title.clone(), p.href.clone()))
                    .collect(),
                explorer: explorer(),
            },
            None => SitePage::NotFound,
        },
        View::Site(name) => component_page(site, addressing, &name, components).map_or(
            SitePage::NotFound,
            |(title, html, slots)| SitePage::Component {
                title,
                html,
                explorer: explorer(),
                slots,
            },
        ),
    }
}

/// The page `name` a component emits, as `(title, html, slots)`; `None` when
/// no component emits it or its render failed (logged).
fn component_page(
    site: &mae_site::Site,
    addressing: &Addressing,
    name: &str,
    components: Option<&ComponentsFor>,
) -> Option<(String, String, Option<Slots>)> {
    let c = components?;
    let ctx = c
        .components
        .site_context(&c.locale, addressing, published(site));
    let (title, html) = c.components.render_page(name, &ctx)?;
    let href = mae_site::components::site_page_href(addressing, name);
    let slots = c.components.render(generated_page(&title, href), &ctx);
    Some((title, html, Some(slots)))
}

/// Every published page's address, by ID: what `published(id)` answers
/// (P-154), from the same derivation as the page.
fn published(site: &mae_site::Site) -> std::collections::BTreeMap<String, String> {
    site.pages
        .iter()
        .map(|(id, p)| (id.clone(), p.href.clone()))
        .collect()
}

/// `components` rendered for `page`; `None` when the KB has none.
fn slots_for(
    components: Option<&ComponentsFor>,
    site: &mae_site::Site,
    addressing: &Addressing,
    page: PageData,
) -> Option<Slots> {
    let c = components?;
    let ctx = c
        .components
        .site_context(&c.locale, addressing, published(site));
    Some(c.components.render(page, &ctx))
}

/// A derived page's own data, as its components see it (P-154).
fn page_data(p: &mae_site::DerivedPage) -> PageData {
    PageData {
        id: p.id.clone(),
        title: p.title.clone(),
        url: p.href.clone(),
        properties: p.properties.clone(),
        dates: p.dates.clone(),
        tags: p.tags.clone(),
    }
}

/// A page the engine or a component generates (a listing, a component
/// page): no ID, no properties, no dates, no tags.
fn generated_page(title: &str, url: String) -> PageData {
    PageData {
        id: String::new(),
        title: title.to_string(),
        url,
        properties: Default::default(),
        dates: Default::default(),
        tags: Vec::new(),
    }
}

/// A node as the Org text of its page: its ID and properties, title and file
/// tags, then its body. Any node has a page, however it was written (seed,
/// editor, tool), so this is not the seed export's file, which refuses a node
/// that would not seed back exactly.
///
/// The properties are the node's own field, where `kb seed` keeps the file
/// drawer's; a page built without them had none, so no slot component could
/// ever read a seeded page's `STATE` or `UPDATED` (P-154, P-161, P-163). Only
/// a property [`drawer_line`] can write as one line of its own is kept.
///
/// The tags come from the node's own field, which is where `kb seed` and the
/// editor keep `#+filetags`; a page built from title and body alone had none,
/// so no tag page ever listed a seeded page. Only valid Org tags are written
/// (P-73), so a tag cannot add a line, and a title's line breaks are folded so
/// it stays one keyword line. No page for a node whose ID could not be an
/// address (the core's own rule: controls, White_Space, bidi controls, U+FEFF).
fn page_source(node_id: &str, doc: &KbNodeDoc) -> Option<String> {
    if node_id.is_empty() || node_id.chars().any(mae_site::address::refused_in_id) {
        return None;
    }
    let title: String = doc
        .title()
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let tags: Vec<String> = doc
        .tags()
        .into_iter()
        .filter(|t| mae_site::address::valid_tag(t))
        .collect();
    let filetags = if tags.is_empty() {
        String::new()
    } else {
        format!("#+filetags: :{}:\n", tags.join(":"))
    };
    let mut props: Vec<(String, String)> = doc.properties().into_iter().collect();
    props.sort();
    let drawer: String = props
        .iter()
        .filter_map(|(k, v)| drawer_line(k, v))
        .collect();
    Some(format!(
        ":PROPERTIES:\n:ID: {node_id}\n{drawer}:END:\n#+title: {title}\n{filetags}\n{}\n",
        doc.body()
    ))
}

/// One property as a drawer line, `:KEY: value`, or `None` when it cannot be
/// one line that sets exactly that property: a key that is empty, holds
/// whitespace or `:`, appends (`KEY+`), or is `ID` (written from the node's
/// own ID), `END` or `PROPERTIES` in any case. A value's line breaks are
/// folded, as a title's are, so a value cannot end the drawer or add a line.
fn drawer_line(key: &str, value: &str) -> Option<String> {
    let plain = !key.is_empty()
        && !key.ends_with('+')
        && !key.chars().any(|c| c.is_whitespace() || c == ':')
        && !["ID", "END", "PROPERTIES"]
            .iter()
            .any(|r| key.eq_ignore_ascii_case(r));
    if !plain {
        return None;
    }
    let value: String = value
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    Some(format!(":{key}: {value}\n"))
}

/// The chrome around one page: the site's locale and, when the operator
/// named one, the site's title (P-16) with the address of its home page, and
/// the operator's components rendered for this page. A plain locale string is
/// a site without a title or components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome<'a> {
    pub lang: &'a str,
    /// `(site title, home href)`: operator configuration, never content (D3).
    pub site: Option<(&'a str, &'a str)>,
    /// The page's slots (D18, H1); `None` writes the shell exactly as a site
    /// without components has it (P-158).
    pub slots: Option<&'a Slots>,
}

impl<'a> From<&'a str> for Chrome<'a> {
    fn from(lang: &'a str) -> Self {
        Chrome {
            lang,
            site: None,
            slots: None,
        }
    }
}

impl Chrome<'_> {
    /// The sanitised HTML of `slot` on this page; `""` when it is empty or the
    /// KB has no components.
    fn slot(&self, slot: Slot) -> &str {
        self.slots.map_or("", |s| s.get(slot))
    }
}

/// The configured locale's catalogue, and how the chrome marks what it writes
/// from it (ADR-119 A4). A locale with no catalogue gets en-US's words, and
/// every element holding them carries `lang="en-US"` so a screen reader voices
/// them as written, while `<html lang>` stays the configured locale; content
/// inside such an element (a tag) carries the configured locale back. A found
/// catalogue is in the page's own language: no marks.
///
/// Not marked: a listing's `<title>`, which repeats its heading. It is plain
/// text holding the operator's site title too, so it cannot mark one part.
struct Words {
    s: &'static mae_site::strings::Strings,
    /// ` lang="en-US"` on an element holding fallback words; empty otherwise.
    mark: String,
    /// ` lang="<configured>"` on content inside a marked element; empty otherwise.
    unmark: String,
}

impl Words {
    fn of(lang: &str) -> Self {
        match mae_site::strings::resolve(lang) {
            (s, false) => Words {
                s,
                mark: String::new(),
                unmark: String::new(),
            },
            (s, true) => Words {
                s,
                mark: format!(" lang=\"{}\"", escape(s.locale)),
                unmark: format!(" lang=\"{}\"", escape(lang)),
            },
        }
    }

    /// Escaped content inside marked chrome, with the page's language back.
    fn content(&self, escaped: &str) -> String {
        if self.unmark.is_empty() {
            escaped.to_string()
        } else {
            format!("<span{}>{escaped}</span>", self.unmark)
        }
    }
}

/// The HTML document for a found page: the core's fixed chrome around the
/// sanitised article (added after sanitising, D19). `title` is escaped here.
pub fn document<'a>(
    chrome: impl Into<Chrome<'a>>,
    title: &str,
    article: &str,
    backlinks: &[mae_site::Backlink],
    tags: &[(String, String)],
    toc: &[mae_site::TocEntry],
    explorer: Option<&ExplorerRef>,
) -> String {
    let chrome = chrome.into();
    let words = Words::of(chrome.lang);
    // Before the body: after the title and its metadata (the tags); after
    // the body: the article's end (P-153).
    let article = format!(
        "<article id=\"mae-article\"{}><h1>{}</h1>{}{}{article}{}</article>",
        copy_words(&words),
        escape(title),
        tags_list(&words, tags),
        chrome.slot(Slot::BeforeBody),
        chrome.slot(Slot::AfterBody),
    );
    // The side panels sit outside the main landmark (P-165).
    let panels = format!(
        "{}{}",
        toc_nav(&words, toc),
        backlinks_nav(&words, backlinks)
    );
    shell(chrome, title, &article, &panels, explorer)
}

/// The words the router's copy control needs (P-109), on the article that
/// holds the source blocks, for the behaviour to read as the theme box's are
/// read: core chrome, never content. On a fallback the behaviour marks each
/// button with the catalogue's locale (`data-copy-lang`), since the article
/// itself holds content in the page's language.
fn copy_words(words: &Words) -> String {
    let lang = if words.mark.is_empty() {
        String::new()
    } else {
        format!(" data-copy-lang=\"{}\"", escape(words.s.locale))
    };
    format!(
        " data-copy-label=\"{}\" data-copy-done=\"{}\" data-copy-selected=\"{}\"{lang}",
        escape(words.s.copy_code),
        escape(words.s.code_copied),
        escape(words.s.code_selected),
    )
}

/// The page's table of contents (P-79): one link per heading, to the id the
/// heading carries on this page; nothing for a page with fewer than two.
fn toc_nav(words: &Words, toc: &[mae_site::TocEntry]) -> String {
    if toc.len() < 2 {
        return String::new();
    }
    let items: String = toc
        .iter()
        .map(|e| {
            format!(
                "<li class=\"mae-toc-l{}\"><a href=\"#{}\">{}</a></li>",
                e.level,
                escape(&e.anchor),
                escape(&e.text)
            )
        })
        .collect();
    format!(
        "<nav id=\"mae-toc\" aria-labelledby=\"mae-toc-h\"><h2 id=\"mae-toc-h\"{}>{}</h2><ol>{items}</ol></nav>",
        words.mark,
        escape(words.s.contents)
    )
}

/// The HTML document for a generated listing: the tag index or one tag's
/// pages (P-70, P-71), headed in the locale's words.
pub fn listing_document<'a>(
    chrome: impl Into<Chrome<'a>>,
    kind: &Listing,
    items: &[(String, String)],
    explorer: Option<&ExplorerRef>,
) -> String {
    let chrome = chrome.into();
    let words = Words::of(chrome.lang);
    let heading = match kind {
        Listing::Tags => words.s.tags.to_string(),
        Listing::Tag(t) => format!("{}{t}{}", words.s.tag_page_open, words.s.tag_page_close),
    };
    // The visible heading marks the catalogue's words, and gives the tag, which
    // is content, the page's language back.
    let shown = match kind {
        Listing::Tags => escape(&heading),
        Listing::Tag(t) => format!(
            "{}{}{}",
            escape(words.s.tag_page_open),
            words.content(&escape(t)),
            escape(words.s.tag_page_close)
        ),
    };
    let list: String = items
        .iter()
        .map(|(text, href)| format!("<li><a href=\"{}\">{}</a></li>", escape(href), escape(text)))
        .collect();
    shell(
        chrome,
        &heading,
        &format!(
            "<section id=\"mae-listing\"><h1{}>{shown}</h1>{}<ul>{list}</ul>{}</section>",
            words.mark,
            chrome.slot(Slot::BeforeBody),
            chrome.slot(Slot::AfterBody),
        ),
        "",
        explorer,
    )
}

/// The HTML document for a page a component emits (H2): its sanitised HTML
/// under its title, in the site's shell, with the slots a page has.
pub fn component_document<'a>(
    chrome: impl Into<Chrome<'a>>,
    title: &str,
    html: &str,
    explorer: Option<&ExplorerRef>,
) -> String {
    let chrome = chrome.into();
    let article = format!(
        "<article id=\"mae-article\"><h1>{}</h1>{}{html}{}</article>",
        escape(title),
        chrome.slot(Slot::BeforeBody),
        chrome.slot(Slot::AfterBody),
    );
    shell(chrome, title, &article, "", explorer)
}

/// The document around a page's main content: lang, the pre-paint script,
/// the palette, the title, the site's header, one main landmark holding the
/// content, and the side panels after it, in an `aside`. Header and panels
/// are never inside `main` (P-165). The header always holds the mode and
/// palette controls, and the site's title first when there is one (a bare
/// page title otherwise); no panels, no `aside`.
///
/// The pre-paint script is a classic script before the stylesheets (D17): it
/// blocks rendering until the reader's choice is on `<html>`, so the first
/// frame already has it (P-91, P-93).
///
/// The explorer's box sits between the header and `main`, empty and `hidden`:
/// the router's behaviour fills it from `data-src` and shows it, so without
/// JavaScript it is absent, never broken. It carries the words the behaviour
/// needs (its name, the untagged group's).
///
/// The operator's slots (P-153) go where [`Slots`] says, each only when it
/// has output (P-158): the document head after the runtime's assets, the
/// header row after the built-in controls, the left-panel footer after the
/// explorer (`#mae-left-footer`), the right panel at the end of the `aside`,
/// and the page footer last (`#mae-footer`). The components' stylesheets
/// follow the base stylesheet and their behaviours follow the router, so the
/// router has defined `mae.site.register` before any of them runs (module
/// scripts run in document order).
fn shell(
    chrome: Chrome<'_>,
    title: &str,
    main: &str,
    panels: &str,
    explorer: Option<&ExplorerRef>,
) -> String {
    let words = Words::of(chrome.lang);
    let (doc_title, site_link) = match chrome.site {
        Some((site, home)) => (
            format!("{} \u{2014} {}", escape(title), escape(site)),
            format!("<a href=\"{}\">{}</a>", escape(home), escape(site)),
        ),
        None => (escape(title), String::new()),
    };
    let header = format!(
        "<header id=\"mae-header\">{site_link}{}{}</header>",
        theme_controls(&words),
        chrome.slot(Slot::HeaderRow),
    );
    let explorer = explorer.map_or_else(String::new, |e| explorer_box(&words, e));
    let panels = format!("{panels}{}", chrome.slot(Slot::RightPanel));
    let aside = wrapped("aside", "mae-panels", &panels);
    let (styles, scripts) = component_assets(chrome.slots);
    format!(
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <script src=\"{}\"></script>\
         <link rel=\"stylesheet\" href=\"{}\">\
         <link rel=\"stylesheet\" href=\"{}\">{styles}\
         <script type=\"module\" src=\"{}\"></script>{scripts}{}\
         <title>{doc_title}</title></head><body>{header}{explorer}{}<main id=\"mae-main\"{}>{main}</main>{aside}{}</body></html>\n",
        escape(chrome.lang),
        mae_site::assets::href(mae_site::assets::PREPAINT),
        mae_site::assets::href(mae_site::assets::PALETTE),
        mae_site::assets::href(mae_site::assets::BASE),
        mae_site::assets::href(mae_site::assets::ROUTER),
        chrome.slot(Slot::DocumentHead),
        wrapped("div", "mae-left-footer", chrome.slot(Slot::LeftPanelFooter)),
        main_attrs(chrome.slots),
        wrapped("footer", "mae-footer", chrome.slot(Slot::PageFooter)),
    )
}

/// `<tag id="id">html</tag>`, or nothing at all when `html` is empty: an
/// empty slot or panel adds no element (P-158).
fn wrapped(tag: &str, id: &str, html: &str) -> String {
    if html.is_empty() {
        String::new()
    } else {
        format!("<{tag} id=\"{id}\">{html}</{tag}>")
    }
}

/// The explorer's box: see [`shell`].
fn explorer_box(words: &Words, e: &ExplorerRef) -> String {
    format!(
        "<nav id=\"mae-explorer\" class=\"mae-explorer\"{} aria-label=\"{}\" \
         data-src=\"{}\" data-version=\"{}\" data-untagged=\"{}\" hidden></nav>",
        words.mark,
        escape(words.s.explorer),
        escape(&e.src),
        escape(&e.version),
        escape(words.s.untagged),
    )
}

/// The components' `<link rel="stylesheet">`s and `<script type="module">`s;
/// nothing without components.
fn component_assets(slots: Option<&Slots>) -> (String, String) {
    let Some(s) = slots else {
        return (String::new(), String::new());
    };
    let styles = s
        .stylesheets
        .iter()
        .map(|h| format!("<link rel=\"stylesheet\" href=\"{}\">", escape(h)))
        .collect();
    let scripts = s
        .scripts
        .iter()
        .map(|h| format!("<script type=\"module\" src=\"{}\"></script>", escape(h)))
        .collect();
    (styles, scripts)
}

/// What a behaviour's `ctx` reads from `<main>` (D18): the KB, the site's
/// base path, this page's address and, on a node's page, its ID. Nothing
/// without components, so a site without them is unchanged (P-158).
fn main_attrs(slots: Option<&Slots>) -> String {
    let Some(s) = slots else {
        return String::new();
    };
    let p = &s.page;
    let id = p.id.as_deref().map_or_else(String::new, |id| {
        format!(" data-mae-page=\"{}\"", escape(id))
    });
    format!(
        " data-mae-kb=\"{}\" data-mae-base=\"{}\" data-mae-url=\"{}\"{id}",
        escape(&p.kb),
        escape(&p.base),
        escape(&p.url)
    )
}

/// The mode toggle and the palette button (P-91, P-93), in the locale's
/// words, inside a box that ships `hidden`: the router's behaviour builds the
/// menu from the palette data the pre-paint script published, and shows the
/// box once it works, so without JavaScript the controls are absent. The box
/// carries the words the menu needs (the default accent's label, each
/// accent's name) for the behaviour to read; core chrome, never content. A
/// fallback marks the box, so the menu the behaviour builds inside it is
/// marked too.
fn theme_controls(words: &Words) -> String {
    let accents: String = words
        .s
        .accents
        .iter()
        .map(|(id, name)| format!(" data-accent-{}=\"{}\"", escape(id), escape(name)))
        .collect();
    format!(
        "<div id=\"mae-theme\" class=\"mae-theme\" hidden{} data-default-label=\"{}\"{accents}>\
         <button type=\"button\" id=\"mae-mode-toggle\" aria-pressed=\"false\">\
         <span class=\"mae-mode-icon\" aria-hidden=\"true\"></span>{}</button>\
         <button type=\"button\" id=\"mae-palette-button\" aria-haspopup=\"menu\" \
         aria-expanded=\"false\" aria-controls=\"mae-palette-menu\">{}</button></div>",
        words.mark,
        escape(words.s.accent_default),
        escape(words.s.mode_toggle),
        escape(words.s.palette_menu),
    )
}

/// A page's tags, each linking to its tag page (P-70); nothing when untagged.
/// The list's name is the catalogue's; on a fallback the list is marked and
/// each tag, which is content, gets the page's language back.
fn tags_list(words: &Words, tags: &[(String, String)]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let items: String = tags
        .iter()
        .map(|(t, href)| {
            format!(
                "<li{}><a href=\"{}\">{}</a></li>",
                words.unmark,
                escape(href),
                escape(t)
            )
        })
        .collect();
    format!(
        "<ul id=\"mae-tags\"{} aria-label=\"{}\">{items}</ul>",
        words.mark,
        escape(words.s.tags)
    )
}

/// The page's backlinks (P-36), named by the locale's words; nothing when
/// no published page links here.
fn backlinks_nav(words: &Words, backlinks: &[mae_site::Backlink]) -> String {
    if backlinks.is_empty() {
        return String::new();
    }
    let items: String = backlinks
        .iter()
        .map(|b| {
            format!(
                "<li><a href=\"{}\">{}</a></li>",
                escape(&b.href),
                escape(&b.title)
            )
        })
        .collect();
    format!(
        "<nav id=\"mae-backlinks\" aria-labelledby=\"mae-backlinks-h\">\
         <h2 id=\"mae-backlinks-h\"{}>{}</h2><ul>{items}</ul></nav>",
        words.mark,
        escape(words.s.backlinks)
    )
}

/// The Content-Security-Policy every site response carries (D19): scripts and
/// styles from this origin only, no inline script, nothing framed or framing,
/// no base or form target.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
                       connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; \
                       require-trusted-types-for 'script'; trusted-types mae-router";

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_cannot_break_out_of_its_element_or_its_keyword_line() {
        let html = document(
            "en-US",
            "</title><script>alert(1)</script>",
            "<p>x</p>",
            &[],
            &[],
            &[],
            None,
        );
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;/title&gt;"), "{html}");
        // Neither a title nor a tag can add a keyword line.
        let evil = KbNodeDoc::new(
            "n",
            "T\n#+title: evil\n:END:",
            "b",
            &["ok".into(), "x:\n#+title: evil".into(), "a b".into()],
        );
        let src = page_source("n", &evil).unwrap();
        assert_eq!(
            src.lines().filter(|l| l.starts_with("#+title:")).count(),
            1,
            "{src}"
        );
        assert!(src.contains("#+filetags: :ok:\n"), "{src}");
        let plain = KbNodeDoc::new("n", "t", "b", &[]);
        assert!(page_source("n", &plain).is_some());
        // Nor can a property: a value's line breaks fold, and a key that is
        // not one plain property name, or would be the drawer's own, is
        // dropped. The page keeps the node's ID.
        let mut props = KbNodeDoc::new("n", "t", "b", &[]);
        let _ = props.set_properties(
            &[
                ("STATE", "draft\n:END:\n#+title: evil"),
                ("ID", "other"),
                ("id", "other"),
                ("END", "x"),
                ("a b", "x"),
                ("k:v", "x"),
                ("K+", "x"),
                ("", "x"),
                ("UPDATED", "2026-11-02"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        );
        let src = page_source("n", &props).unwrap();
        assert!(
            src.starts_with(
                ":PROPERTIES:\n:ID: n\n:STATE: draft :END: #+title: evil\n:UPDATED: 2026-11-02\n:END:\n"
            ),
            "{src}"
        );
        assert_eq!(
            src.lines().filter(|l| l.starts_with("#+title:")).count(),
            1,
            "{src}"
        );
        assert_eq!(page_source("a b", &plain), None);
        assert_eq!(page_source("", &plain), None);
    }

    /// A small KB: `a` tagged `t`, `b` untagged and linking to `a`.
    fn kb() -> (mae_site::Site, Addressing) {
        let live = Addressing::live("kb-1").unwrap();
        let src = |id: &str, extra: &str| SourcePage {
            attachments: Default::default(),
            origin: format!("{id}.org"),
            text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n{extra}\n"),
        };
        let site = mae_site::derive(
            &[
                src("a", "#+filetags: :t:\n"),
                src("b", "\n[[id:a][to a]]\n"),
            ],
            &live,
        );
        (site, live)
    }

    /// Every view a reader may have whether or not they may copy the KB.
    fn title_views() -> [View; 5] {
        [
            View::Page("a".into()),
            View::Tags,
            View::Tag("t".into()),
            View::Explorer,
            View::Home(None),
        ]
    }

    /// D7, Amendment 3 G2: a reader who may not copy the KB (query-only) is
    /// refused the content index, which holds every page's text, and nothing
    /// else: pages, the explorer and tag listings hold what they may read.
    #[test]
    fn a_reader_who_may_not_copy_the_kb_is_refused_its_content_index_only() {
        let (site, live) = kb();
        assert_eq!(
            answer(&site, &live, View::Index, false, None),
            SitePage::NotFound
        );
        let SitePage::Index(all) = answer(&site, &live, View::Index, true, None) else {
            panic!("the content index");
        };
        // What makes it a copy: page b's text is in it.
        assert!(all.iter().any(|e| e.text.contains("to a")), "{all:?}");
        for v in title_views() {
            assert_eq!(
                answer(&site, &live, v.clone(), false, None),
                answer(&site, &live, v.clone(), true, None),
                "{v:?}"
            );
            assert_ne!(
                answer(&site, &live, v.clone(), false, None),
                SitePage::NotFound,
                "{v:?}"
            );
        }
    }

    /// Every page and listing names the explorer's
    /// data and its version, which is the hash of exactly what
    /// `explorer.json` answers, so a changed KB is a changed version.
    #[test]
    fn every_page_names_the_explorer_at_the_version_it_answers() {
        let (site, live) = kb();
        let SitePage::Explorer(json) = answer(&site, &live, View::Explorer, true, None) else {
            panic!("the explorer");
        };
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "groups": [{"tag": "t", "pages": [{"title": "a", "href": "/kb/kb-1/a"}]}],
                "untagged": [{"title": "b", "href": "/kb/kb-1/b"}],
            })
        );
        let want = explorer_ref(&json, &live);
        assert_eq!(want.src, "/kb/kb-1/explorer.json");
        assert_eq!(want.version.len(), 16);
        for view in [View::Page("a".into()), View::Tags, View::Tag("t".into())] {
            let got = match answer(&site, &live, view.clone(), true, None) {
                SitePage::Found { explorer, tags, .. } => {
                    assert_eq!(tags, [("t".to_string(), "/kb/kb-1/tags/t".to_string())]);
                    explorer
                }
                SitePage::Listing { explorer, .. } => explorer,
                other => panic!("{view:?}: {other:?}"),
            };
            assert_eq!(got.as_ref(), Some(&want), "{view:?}");
        }
        // Another KB, another version.
        let other = mae_site::derive(
            &[SourcePage {
                attachments: Default::default(),
                origin: "c.org".into(),
                text: ":PROPERTIES:\n:ID: c\n:END:\n#+title: c\n".into(),
            }],
            &live,
        );
        let SitePage::Explorer(json2) = answer(&other, &live, View::Explorer, true, None) else {
            panic!()
        };
        assert_ne!(explorer_ref(&json2, &live).version, want.version);
    }

    /// The box: after the header, before `main`, hidden, its words and
    /// attributes escaped, and marked when the words fell back to en-US.
    #[test]
    fn the_explorer_box_sits_before_main_hidden_with_escaped_attributes() {
        let e = ExplorerRef {
            src: "/kb/k/explorer.json".into(),
            version: "\"><script>".into(),
        };
        let html = document("es-ES", "t", "", &[], &[], &[], Some(&e));
        let nav = html.find("<nav id=\"mae-explorer\"").expect("the box");
        assert!(html.find("</header>").unwrap() < nav);
        assert!(nav < html.find("<main").unwrap());
        assert!(!html.contains("<script>\""), "{html}");
        assert!(html.contains("aria-label=\"P\u{e1}ginas\""), "{html}");
        assert!(html.contains("data-untagged=\"Sin etiqueta\""), "{html}");
        let box_ = &html[nav..html[nav..].find("</nav>").unwrap() + nav];
        assert!(box_.ends_with("hidden>"), "{box_}");
        assert!(!box_.contains("lang="), "{box_}");
        let fallback = document("de", "t", "", &[], &[], &[], Some(&e));
        assert!(
            fallback.contains("class=\"mae-explorer\" lang=\"en-US\" aria-label=\"Pages\""),
            "{fallback}"
        );
        let listing = listing_document("en-US", &Listing::Tags, &[], Some(&e));
        assert!(listing.contains("<nav id=\"mae-explorer\""), "{listing}");
    }
}
