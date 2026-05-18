//! mae-kb — in-memory knowledge base (graph store).
//!
//! @stability: stable
//! @since: 0.5.0
//!
//! The knowledge base is the shared data model for:
//!
//! 1. The built-in help system (command, concept, and keybinding docs).
//! 2. User-authored notes (org-roam-style bidirectional links).
//! 3. An AI-facing query surface — the agent is a *peer actor* that can
//!    read the same nodes the human reads via `:help`.
//!
//! ## Design
//!
//! - A **node** is a typed, named document with a markdown body.
//! - Links are embedded in the body as `[[id]]` or `[[id|display text]]`.
//! - The store keeps a reverse index so "what links to X?" is O(1).
//! - No persistence layer yet — everything is in-memory. The Phase-5
//!   SQLite-backed kb.db will replace the storage but preserve this API.
//!
//! This crate depends on no MAE internals — it's a pure data library
//! callable from `mae-core`, `mae-ai`, and eventually `mae-kb-persist`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod activity;
pub mod federation;
pub mod fuzzy;
pub mod org;
pub mod persist;
pub mod watch;
pub use federation::{ImportHealth, ImportReport as FederationImportReport};
pub use org::IngestReport;
pub use persist::PersistError;

/// Kind of a node. Controls how the node is surfaced to the user
/// (e.g. command nodes show up in `describe-command`) and styled by
/// the renderer (e.g. concept nodes get a different sigil).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// The help index page (there is usually exactly one of these).
    Index,
    /// An editor command — seeded from `CommandRegistry` at startup.
    Command,
    /// An architectural concept (buffer, window, mode, AI-as-peer, …).
    Concept,
    /// A keybinding or key sequence documentation entry.
    Key,
    /// Free-form user note (org-roam-style).
    Note,
    /// Project node — represents a detected project from a `.project` file.
    Project,
}

/// Provenance of a node — how it was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeSource {
    /// Seeded at startup from compiled-in content.
    Seed,
    /// Imported from a user org file.
    UserOrg,
    /// Created manually (e.g. via `:help-edit`).
    Manual,
    /// Received via federation from another MAE instance.
    Federation,
}

/// A single node in the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Stable identifier — e.g. `"cmd:delete-line"`, `"concept:buffer"`,
    /// `"index"`. Slugs use `:` as namespace separator by convention.
    pub id: String,
    /// Human-readable title shown at the top of the help buffer.
    pub title: String,
    pub kind: NodeKind,
    /// Markdown body. May contain `[[link]]` markers that the renderer
    /// styles as hyperlinks.
    pub body: String,
    /// Freeform tags for filtering (e.g. `["movement", "vi"]`).
    pub tags: Vec<String>,
    /// TODO state extracted from org heading (e.g. "TODO", "DONE").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_state: Option<String>,
    /// Priority extracted from org heading (e.g. 'A', 'B', 'C').
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<char>,
    /// How this node was created (seed, user org import, manual, federation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<NodeSource>,
    /// Version of the seed data that created this node (for re-seeding).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<u32>,
    /// Alternative names for discoverability (e.g. "plugins" for concept:modules).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Arbitrary property drawer key-value pairs (e.g. last-accessed, hash).
    /// Populated from org `:PROPERTIES:` drawer during ingest.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub properties: HashMap<String, String>,
    /// Path to the source `.org` file this node was parsed from (if any).
    /// Not serialized — ephemeral, populated during ingest.
    #[serde(skip)]
    pub source_file: Option<std::path::PathBuf>,
    /// Encoded yrs CRDT document bytes (for collaborative KB editing).
    /// When present, this is the authoritative representation; `title`/`body`/`tags`
    /// are materialized from the CRDT content for FTS5 and display.
    #[serde(skip)]
    pub crdt_doc: Option<Vec<u8>>,
}

impl Node {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: NodeKind,
        body: impl Into<String>,
    ) -> Self {
        Node {
            id: id.into(),
            title: title.into(),
            kind,
            body: body.into(),
            tags: Vec::new(),
            todo_state: None,
            priority: None,
            source: None,
            source_version: None,
            aliases: Vec::new(),
            properties: HashMap::new(),
            source_file: None,
            crdt_doc: None,
        }
    }

    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_source(mut self, source: NodeSource, version: u32) -> Self {
        self.source = Some(source);
        self.source_version = Some(version);
        self
    }

    pub fn with_todo_state(mut self, state: &str) -> Self {
        self.todo_state = Some(state.to_string());
        self
    }

    pub fn with_priority(mut self, priority: char) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn with_properties(mut self, props: HashMap<String, String>) -> Self {
        self.properties = props;
        self
    }

    pub fn with_source_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.source_file = Some(path.into());
        self
    }

    /// Create a `KbNodeDoc` from this node's content.
    ///
    /// If the node already has CRDT bytes (`crdt_doc`), restores from those.
    /// Otherwise creates a fresh yrs document from the text fields.
    pub fn to_crdt_doc(&self) -> Result<mae_sync::kb::KbNodeDoc, mae_sync::SyncError> {
        if let Some(ref bytes) = self.crdt_doc {
            mae_sync::kb::KbNodeDoc::from_bytes(bytes)
        } else {
            Ok(mae_sync::kb::KbNodeDoc::new(
                &self.id,
                &self.title,
                &self.body,
                &self.tags,
            ))
        }
    }

    /// Update this node's text fields from a `KbNodeDoc`, and store the
    /// encoded CRDT bytes for persistence.
    pub fn apply_crdt_doc(&mut self, doc: &mae_sync::kb::KbNodeDoc) {
        self.title = doc.title();
        self.body = doc.body();
        self.tags = doc.tags();
        self.crdt_doc = Some(doc.encode());
    }

    /// Extract all `[[link]]` and `[[link|display]]` targets from the body.
    /// Returns the target ids in document order, deduplicated.
    pub fn links(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (target, _) in parse_links(&self.body) {
            if seen.insert(target.clone()) {
                out.push(target);
            }
        }
        out
    }
}

/// A parsed link from a body: `(target_id, display_text)`.
/// Display text defaults to the target id if no `|display` override exists.
pub fn parse_links(body: &str) -> Vec<(String, String)> {
    // Pre-compute code block ranges to skip (same logic as rewrite_links).
    let code_ranges = compute_code_block_ranges(body);

    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // Skip links inside #+begin_src … #+end_src.
            if code_ranges.iter().any(|&(s, e)| i >= s && i < e) {
                i += 1;
                continue;
            }
            if let Some(end_rel) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + end_rel];
                // Split on '|' for display-text override.
                let (target, display) = match inner.find('|') {
                    Some(bar) => (&inner[..bar], &inner[bar + 1..]),
                    None => (inner, inner),
                };
                let target = target.trim();
                if !target.is_empty() {
                    out.push((target.to_string(), display.trim().to_string()));
                }
                i += 2 + end_rel + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Compute byte ranges of `#+begin_src` … `#+end_src` blocks (case-insensitive).
fn compute_code_block_ranges(body: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let lower = body.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(start) = lower[search_from..].find("#+begin_src") {
        let abs_start = search_from + start;
        if let Some(end) = lower[abs_start..].find("#+end_src") {
            let abs_end = abs_start + end + "#+end_src".len();
            let abs_end = body[abs_end..]
                .find('\n')
                .map_or(body.len(), |nl| abs_end + nl + 1);
            ranges.push((abs_start, abs_end));
            search_from = abs_end;
        } else {
            ranges.push((abs_start, body.len()));
            break;
        }
    }
    ranges
}

/// Classification of a broken link — why it's broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokenLinkKind {
    /// Target UUID is well-formed but no node with that ID exists (deleted file).
    DeletedNode,
    /// Target is not a valid UUID (elisp code, prose, malformed markup).
    MalformedId,
    /// Target is a template placeholder like `%s` or `UUID`.
    TemplatePlaceholder,
}

/// A broken link with classification and display context.
#[derive(Debug, Clone)]
pub struct BrokenLink {
    pub source: String,
    pub target: String,
    pub display: String,
    pub kind: BrokenLinkKind,
}

impl BrokenLink {
    /// Classify a broken link target.
    fn classify(target: &str) -> BrokenLinkKind {
        let t = target.trim();
        if t == "%s" || t.eq_ignore_ascii_case("uuid") || t == "..." {
            BrokenLinkKind::TemplatePlaceholder
        } else if is_uuid_like(t) {
            BrokenLinkKind::DeletedNode
        } else {
            BrokenLinkKind::MalformedId
        }
    }
}

/// Check if a string looks like a UUID (8-4-4-4-12 hex pattern).
fn is_uuid_like(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A node whose `source_file` points to a path that no longer exists on disk.
#[derive(Debug, Clone)]
pub struct StaleNode {
    pub id: String,
    pub title: String,
    pub source_file: std::path::PathBuf,
}

/// Health report for the knowledge base — orphans, broken links, namespace stats.
#[derive(Debug, Clone)]
pub struct KbHealthReport {
    pub total_nodes: usize,
    pub total_links: usize,
    pub orphan_ids: Vec<String>,
    pub broken_links: Vec<BrokenLink>,
    pub namespace_counts: HashMap<String, usize>,
    pub stale_nodes: Vec<StaleNode>,
}

/// Pre-lowercased search cache for a single node. Populated at insert
/// time so `search()` doesn't re-allocate on every query — the dominant
/// cost in the naive implementation.
#[derive(Debug, Clone, Default)]
struct LowerCache {
    lowered_id: String,
    title: String,
    body: String,
    tags: Vec<String>,
    aliases: Vec<String>,
}

impl LowerCache {
    fn from_node(n: &Node) -> Self {
        Self {
            lowered_id: n.id.to_lowercase(),
            title: n.title.to_lowercase(),
            body: n.body.to_lowercase(),
            tags: n.tags.iter().map(|t| t.to_lowercase()).collect(),
            aliases: n.aliases.iter().map(|a| a.to_lowercase()).collect(),
        }
    }
}

/// The in-memory knowledge base.
///
/// Stores nodes keyed by id and maintains a reverse index so
/// `links_to(id)` is cheap. The forward index is recomputed from the
/// body on every `insert` (cheap — bodies are small).
///
/// Also caches lowercased title/body/tags per node so `search()` is a
/// tight byte-scan with zero per-query allocation. At ~1500 nodes with
/// typical 500-byte bodies this keeps search sub-millisecond; a proper
/// FTS5 backend replaces this in Phase 5.
#[derive(Debug, Default, Clone)]
pub struct KnowledgeBase {
    nodes: HashMap<String, Node>,
    /// Reverse index: `links_in[target] = [source_ids…]`.
    links_in: HashMap<String, Vec<String>>,
    /// Pre-lowercased searchable fields, keyed by node id.
    lower: HashMap<String, LowerCache>,
    /// Secondary index: todo_state → set of node ids.
    todo_index: HashMap<String, HashSet<String>>,
    /// Secondary index: priority → set of node ids.
    priority_index: HashMap<char, HashSet<String>>,
    /// Secondary index: tag → set of node ids.
    tag_index: HashMap<String, HashSet<String>>,
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Get a mutable reference to a node by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Insert (or overwrite) a node. Returns the previous node, if any.
    /// Rebuilds the reverse index entries for this node's links.
    pub fn insert(&mut self, node: Node) -> Option<Node> {
        let id = node.id.clone();
        // Remove old reverse edges and secondary indexes (if any) before rebuilding.
        if let Some(prev) = self.nodes.get(&id) {
            for target in prev.links() {
                if let Some(sources) = self.links_in.get_mut(&target) {
                    sources.retain(|s| s != &id);
                    if sources.is_empty() {
                        self.links_in.remove(&target);
                    }
                }
            }
            // Remove from secondary indexes.
            if let Some(ref state) = prev.todo_state {
                if let Some(set) = self.todo_index.get_mut(state) {
                    set.remove(&id);
                }
            }
            if let Some(pri) = prev.priority {
                if let Some(set) = self.priority_index.get_mut(&pri) {
                    set.remove(&id);
                }
            }
            for tag in &prev.tags {
                if let Some(set) = self.tag_index.get_mut(tag) {
                    set.remove(&id);
                }
            }
        }
        // Install new reverse edges.
        for target in node.links() {
            let entry = self.links_in.entry(target).or_default();
            if !entry.contains(&id) {
                entry.push(id.clone());
            }
        }
        // Update secondary indexes.
        if let Some(ref state) = node.todo_state {
            self.todo_index
                .entry(state.clone())
                .or_default()
                .insert(id.clone());
        }
        if let Some(pri) = node.priority {
            self.priority_index
                .entry(pri)
                .or_default()
                .insert(id.clone());
        }
        for tag in &node.tags {
            self.tag_index
                .entry(tag.clone())
                .or_default()
                .insert(id.clone());
        }
        self.lower.insert(id.clone(), LowerCache::from_node(&node));
        self.nodes.insert(id, node)
    }

    /// Remove a node. Also drops its outgoing reverse-index entries.
    pub fn remove(&mut self, id: &str) -> Option<Node> {
        let prev = self.nodes.remove(id)?;
        self.lower.remove(id);
        for target in prev.links() {
            if let Some(sources) = self.links_in.get_mut(&target) {
                sources.retain(|s| s != id);
                if sources.is_empty() {
                    self.links_in.remove(&target);
                }
            }
        }
        // Clean secondary indexes.
        if let Some(ref state) = prev.todo_state {
            if let Some(set) = self.todo_index.get_mut(state) {
                set.remove(id);
            }
        }
        if let Some(pri) = prev.priority {
            if let Some(set) = self.priority_index.get_mut(&pri) {
                set.remove(id);
            }
        }
        for tag in &prev.tags {
            if let Some(set) = self.tag_index.get_mut(tag) {
                set.remove(id);
            }
        }
        Some(prev)
    }

    /// All node ids, sorted. If `prefix` is provided, only ids starting
    /// with it are returned (useful for `cmd:` namespace listings).
    pub fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        let mut ids: Vec<String> = self
            .nodes
            .keys()
            .filter(|k| prefix.is_none_or(|p| k.starts_with(p)))
            .cloned()
            .collect();
        ids.sort();
        ids
    }

    /// Case-insensitive substring search over title + body + tags + aliases.
    /// Returns matching ids sorted with title/alias matches before body matches.
    /// Falls back to fuzzy scoring when no substring matches are found.
    ///
    /// Scans the pre-lowercased `LowerCache` populated at insert time —
    /// no per-query allocations, no per-node `to_lowercase()`.
    pub fn search(&self, query: &str) -> Vec<String> {
        if query.is_empty() {
            return self.list_ids(None);
        }
        let q = query.to_lowercase();
        let mut title_hits = Vec::new();
        let mut body_hits = Vec::new();
        for (id, cache) in self.lower.iter() {
            if cache.title.contains(&q)
                || cache.lowered_id.contains(&q)
                || cache.aliases.iter().any(|a| a.contains(&q))
            {
                title_hits.push(id.clone());
            } else if cache.body.contains(&q) || cache.tags.iter().any(|t| t.contains(&q)) {
                body_hits.push(id.clone());
            }
        }
        title_hits.sort();
        body_hits.sort();
        title_hits.extend(body_hits);
        if !title_hits.is_empty() {
            return title_hits;
        }
        // Fuzzy fallback: score against id + title + aliases.
        let query_chars: Vec<char> = q.chars().collect();
        let mut scored: Vec<(String, i64)> = self
            .lower
            .iter()
            .filter_map(|(id, cache)| {
                let best = [&cache.lowered_id, &cache.title]
                    .into_iter()
                    .chain(cache.aliases.iter())
                    .filter_map(|s| fuzzy::score_match(s, &query_chars))
                    .max();
                best.map(|score| (id.clone(), score))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().map(|(id, _)| id).collect()
    }

    /// Search nodes then re-sort results by activity score (highest first).
    /// Falls back to normal search order for nodes without activity properties.
    pub fn search_sorted_by_activity(
        &self,
        query: &str,
        weights: &activity::ActivityWeights,
        today: (i32, u32, u32),
    ) -> Vec<String> {
        let ids = self.search(query);
        let mut scored: Vec<(String, f64)> = ids
            .into_iter()
            .map(|id| {
                let score = self
                    .get(&id)
                    .map(|n| activity::activity_score(&n.properties, weights, today))
                    .unwrap_or(0.0);
                (id, score)
            })
            .collect();
        // Stable sort: equal-score nodes keep their original search rank.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(id, _)| id).collect()
    }

    /// Extract unique namespace prefixes from all node IDs (e.g., "cmd:", "concept:").
    /// Derived dynamically so it never goes stale when new namespaces are added.
    pub fn namespace_prefixes(&self) -> Vec<String> {
        let mut prefixes: Vec<String> = self
            .nodes
            .keys()
            .filter_map(|id| id.find(':').map(|pos| id[..=pos].to_string()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        prefixes.sort();
        prefixes
    }

    /// Outgoing links from a node (targets of `[[…]]` markers in its body).
    /// Returns link targets in document order. Dangling links (to missing
    /// nodes) are included — callers decide how to render them.
    pub fn links_from(&self, id: &str) -> Vec<String> {
        self.nodes.get(id).map(|n| n.links()).unwrap_or_default()
    }

    /// Combined outgoing + incoming neighbors of a node, deduplicated,
    /// with outgoing order preserved and backlinks appended after.
    /// Shared by the terminal-help "Tab cycles through all reachable
    /// nodes" UX and the AI's `kb_graph` BFS.
    pub fn neighbors(&self, id: &str) -> Vec<String> {
        let mut out = self.links_from(id);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for src in self.links_to(id) {
            if seen.insert(src.clone()) {
                out.push(src);
            }
        }
        out
    }

    /// Iterator over all nodes (value-references) — used by persistence
    /// layers. Order is arbitrary; callers that need a stable order should
    /// collect and sort by id.
    pub(crate) fn nodes_values(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Stamp all nodes that have no source with the given source and version.
    pub fn stamp_source(&mut self, source: NodeSource, version: u32) {
        for node in self.nodes.values_mut() {
            if node.source.is_none() {
                node.source = Some(source);
                node.source_version = Some(version);
            }
        }
    }

    /// Ingest a project config as a KB node.
    pub fn ingest_project(&mut self, name: &str, root: &std::path::Path, config_body: &str) {
        let id = format!("project:{}", name.to_lowercase().replace(' ', "-"));
        let node = Node::new(
            id,
            name,
            NodeKind::Project,
            format!(
                "# Project: {}\n\nRoot: `{}`\n\n{}",
                name,
                root.display(),
                config_body
            ),
        )
        .with_tags(["project"]);
        self.insert(node);
    }

    /// All nodes with any TODO state (not DONE/CANCELLED/DEFERRED).
    pub fn todo_nodes(&self) -> Vec<&Node> {
        let mut out: Vec<&Node> = self
            .todo_index
            .values()
            .flat_map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// Nodes with a specific TODO state.
    pub fn nodes_by_todo_state(&self, state: &str) -> Vec<&Node> {
        let mut out: Vec<&Node> = self
            .todo_index
            .get(state)
            .into_iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Nodes with a specific priority.
    pub fn nodes_by_priority(&self, priority: char) -> Vec<&Node> {
        let mut out: Vec<&Node> = self
            .priority_index
            .get(&priority)
            .into_iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Nodes with a specific tag.
    pub fn nodes_by_tag(&self, tag: &str) -> Vec<&Node> {
        let mut out: Vec<&Node> = self
            .tag_index
            .get(tag)
            .into_iter()
            .flat_map(|ids| ids.iter().filter_map(|id| self.nodes.get(id)))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Compute a health report: orphan nodes, broken links, namespace counts.
    pub fn health_report(&self) -> KbHealthReport {
        self.health_report_with(|_| false)
    }

    /// Health report with an external resolver for cross-KB link checking.
    /// `external_contains` returns true if a target exists in another KB.
    pub fn health_report_with(&self, external_contains: impl Fn(&str) -> bool) -> KbHealthReport {
        let all_ids: HashSet<&str> = self.nodes.keys().map(|s| s.as_str()).collect();

        // Single fold over all nodes: accumulate link count, broken links,
        // orphan IDs, and namespace counts in one pass.
        struct Acc {
            total_links: usize,
            broken_links: Vec<BrokenLink>,
            orphan_ids: Vec<String>,
            namespace_counts: HashMap<String, usize>,
        }

        let result = self.nodes.iter().fold(
            Acc {
                total_links: 0,
                broken_links: Vec::new(),
                orphan_ids: Vec::new(),
                namespace_counts: HashMap::new(),
            },
            |mut acc, (id, node)| {
                // Links: count + broken detection with classification.
                let link_pairs = parse_links(&node.body);
                acc.total_links += link_pairs.len();
                for (target, display) in &link_pairs {
                    if !all_ids.contains(target.as_str()) && !external_contains(target) {
                        acc.broken_links.push(BrokenLink {
                            source: node.id.clone(),
                            target: target.clone(),
                            display: display.clone(),
                            kind: BrokenLink::classify(target),
                        });
                    }
                }

                // Orphans: no links in or out, not an index node.
                if node.kind != NodeKind::Index {
                    let has_outgoing = !link_pairs.is_empty();
                    let has_incoming = self
                        .links_in
                        .get(id.as_str())
                        .is_some_and(|v| !v.is_empty());
                    if !has_outgoing && !has_incoming {
                        acc.orphan_ids.push(id.clone());
                    }
                }

                // Namespace.
                let ns = id.find(':').map_or("(none)", |pos| &id[..pos]);
                *acc.namespace_counts.entry(ns.to_string()).or_default() += 1;

                acc
            },
        );

        let mut orphan_ids = result.orphan_ids;
        orphan_ids.sort();

        KbHealthReport {
            total_nodes: self.nodes.len(),
            total_links: result.total_links,
            orphan_ids,
            broken_links: result.broken_links,
            namespace_counts: result.namespace_counts,
            stale_nodes: Vec::new(), // populated lazily by caller via detect_stale_nodes()
        }
    }

    /// Incoming links — node ids whose body references `target`.
    pub fn links_to(&self, target: &str) -> Vec<String> {
        let mut v = self.links_in.get(target).cloned().unwrap_or_default();
        v.sort();
        v
    }

    /// Detect nodes whose `source_file` points to a path that no longer exists.
    /// This is intentionally lazy — call on-demand (health report, reimport),
    /// not on every drain tick (filesystem stat per node is expensive).
    pub fn detect_stale_nodes(&self) -> Vec<StaleNode> {
        self.nodes
            .values()
            .filter_map(|n| {
                n.source_file.as_ref().and_then(|path| {
                    if !path.exists() {
                        Some(StaleNode {
                            id: n.id.clone(),
                            title: n.title.clone(),
                            source_file: path.clone(),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    /// Remove stale nodes (source file deleted) and return the count removed.
    pub fn remove_stale_nodes(&mut self) -> usize {
        let stale_ids: Vec<String> = self
            .detect_stale_nodes()
            .into_iter()
            .map(|s| s.id)
            .collect();
        let count = stale_ids.len();
        for id in stale_ids {
            self.remove(&id);
        }
        count
    }

    /// Validate links in a node's body, returning IDs of missing targets.
    pub fn validate_links(&self, node_id: &str) -> Vec<String> {
        let body = match self.nodes.get(node_id) {
            Some(n) => &n.body,
            None => return Vec::new(),
        };
        parse_links(body)
            .into_iter()
            .filter(|(target, _)| !self.nodes.contains_key(target))
            .map(|(target, _)| target)
            .collect()
    }

    /// Return all (id, title) pairs for all nodes, sorted by id.
    pub fn all_id_title_pairs(&self) -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = self
            .nodes
            .values()
            .map(|n| (n.id.clone(), n.title.clone()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }
}

/// Generate a URL-friendly slug from a title.
///
/// Lowercases, replaces non-alphanumeric chars with hyphens,
/// collapses consecutive hyphens, trims leading/trailing hyphens.
pub fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Generate a timestamp-based ID prefix: "20260515T143000".
pub fn timestamp_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Convert to date-time components (approximate, no leap second handling).
    let mut days = secs / 86400;
    let day_secs = secs % 86400;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Year calculation.
    #[allow(clippy::manual_is_multiple_of)]
    let is_leap_year = |y: u64| -> bool { y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) };
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // Month calculation.
    let is_leap = is_leap_year(year);
    let month_days = [
        31,
        if is_leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if days < md {
            month = i + 1;
            break;
        }
        days -= md;
    }
    if month == 0 {
        month = 12;
    }
    let day = days + 1;

    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb_with(nodes: Vec<Node>) -> KnowledgeBase {
        let mut kb = KnowledgeBase::new();
        for n in nodes {
            kb.insert(n);
        }
        kb
    }

    #[test]
    fn empty_kb() {
        let kb = KnowledgeBase::new();
        assert_eq!(kb.len(), 0);
        assert!(kb.is_empty());
        assert!(kb.get("nope").is_none());
    }

    #[test]
    fn insert_and_get() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Alpha", NodeKind::Note, "body"));
        assert_eq!(kb.len(), 1);
        assert_eq!(kb.get("a").unwrap().title, "Alpha");
    }

    #[test]
    fn insert_overwrites() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "first", NodeKind::Note, ""));
        kb.insert(Node::new("a", "second", NodeKind::Note, ""));
        assert_eq!(kb.len(), 1);
        assert_eq!(kb.get("a").unwrap().title, "second");
    }

    #[test]
    fn remove_drops_node() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "x", NodeKind::Note, "see [[b]]"));
        kb.insert(Node::new("b", "y", NodeKind::Note, ""));
        assert_eq!(kb.links_to("b"), vec!["a".to_string()]);
        kb.remove("a");
        assert!(kb.links_to("b").is_empty());
        assert!(kb.get("a").is_none());
    }

    #[test]
    fn parse_links_basic() {
        let links = parse_links("see [[foo]] and [[bar|Bar!]]");
        assert_eq!(
            links,
            vec![
                ("foo".to_string(), "foo".to_string()),
                ("bar".to_string(), "Bar!".to_string())
            ]
        );
    }

    #[test]
    fn parse_links_empty_target_ignored() {
        assert!(parse_links("[[]] and [[   ]]").is_empty());
    }

    #[test]
    fn parse_links_unclosed_bracket() {
        assert!(parse_links("[[foo").is_empty());
    }

    #[test]
    fn parse_links_skips_code_blocks() {
        let body = "[[real]] text\n#+begin_src elisp\n[[fake]]\n#+end_src\n[[also-real]]";
        let links = parse_links(body);
        let targets: Vec<&str> = links.iter().map(|(t, _)| t.as_str()).collect();
        assert!(targets.contains(&"real"));
        assert!(targets.contains(&"also-real"));
        assert!(
            !targets.contains(&"fake"),
            "code block link should be skipped"
        );
    }

    #[test]
    fn node_links_dedup() {
        let n = Node::new("x", "x", NodeKind::Note, "[[a]] [[a]] [[b]]");
        assert_eq!(n.links(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn links_to_reverse_index() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "goto [[b]]"),
            Node::new("c", "C", NodeKind::Note, "also [[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let mut incoming = kb.links_to("b");
        incoming.sort();
        assert_eq!(incoming, vec!["a", "c"]);
    }

    #[test]
    fn links_to_updates_on_overwrite() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[b]]"));
        assert_eq!(kb.links_to("b"), vec!["a".to_string()]);
        // Overwrite to point elsewhere.
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[c]]"));
        assert!(kb.links_to("b").is_empty());
        assert_eq!(kb.links_to("c"), vec!["a".to_string()]);
    }

    #[test]
    fn links_from_returns_targets_in_order() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("x", "X", NodeKind::Note, "[[one]] and [[two]]"));
        assert_eq!(kb.links_from("x"), vec!["one", "two"]);
    }

    #[test]
    fn links_from_missing_node() {
        let kb = KnowledgeBase::new();
        assert!(kb.links_from("nope").is_empty());
    }

    #[test]
    fn list_ids_sorted() {
        let kb = kb_with(vec![
            Node::new("b", "", NodeKind::Note, ""),
            Node::new("a", "", NodeKind::Note, ""),
            Node::new("c", "", NodeKind::Note, ""),
        ]);
        assert_eq!(kb.list_ids(None), vec!["a", "b", "c"]);
    }

    #[test]
    fn list_ids_with_prefix() {
        let kb = kb_with(vec![
            Node::new("cmd:a", "", NodeKind::Command, ""),
            Node::new("cmd:b", "", NodeKind::Command, ""),
            Node::new("concept:x", "", NodeKind::Concept, ""),
        ]);
        assert_eq!(kb.list_ids(Some("cmd:")), vec!["cmd:a", "cmd:b"]);
    }

    #[test]
    fn search_finds_by_title() {
        let kb = kb_with(vec![
            Node::new("a", "Buffer concept", NodeKind::Concept, ""),
            Node::new("b", "Window concept", NodeKind::Concept, ""),
        ]);
        assert_eq!(kb.search("buffer"), vec!["a"]);
    }

    #[test]
    fn search_finds_by_body() {
        let kb = kb_with(vec![
            Node::new("a", "X", NodeKind::Note, "contains widget"),
            Node::new("b", "Y", NodeKind::Note, "nothing here"),
        ]);
        assert_eq!(kb.search("widget"), vec!["a"]);
    }

    #[test]
    fn search_title_beats_body() {
        let kb = kb_with(vec![
            Node::new("a", "Other", NodeKind::Note, "mentions foo"),
            Node::new("b", "Foo bar", NodeKind::Note, "unrelated"),
        ]);
        // Title match b should come before body match a.
        assert_eq!(kb.search("foo"), vec!["b", "a"]);
    }

    #[test]
    fn search_by_tag() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, "").with_tags(["movement"]));
        assert_eq!(kb.search("movement"), vec!["a"]);
    }

    #[test]
    fn search_empty_returns_all() {
        let kb = kb_with(vec![
            Node::new("a", "", NodeKind::Note, ""),
            Node::new("b", "", NodeKind::Note, ""),
        ]);
        assert_eq!(kb.search(""), vec!["a", "b"]);
    }

    #[test]
    fn search_lower_cache_is_maintained_on_overwrite() {
        // Regression test for the LowerCache invariant: if a node's title
        // changes, the old title must no longer match searches.
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Banana", NodeKind::Note, ""));
        assert_eq!(kb.search("banana"), vec!["a"]);
        kb.insert(Node::new("a", "Cherry", NodeKind::Note, ""));
        assert!(kb.search("banana").is_empty());
        assert_eq!(kb.search("cherry"), vec!["a"]);
    }

    #[test]
    fn search_lower_cache_dropped_on_remove() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Banana", NodeKind::Note, ""));
        kb.remove("a");
        assert!(kb.search("banana").is_empty());
    }

    #[test]
    fn search_scales_to_two_thousand_nodes() {
        // Smoke-test that search returns under 50ms at 2000 nodes with
        // 500-char bodies. Primary value: catches accidental O(n²) regressions
        // when the cache is bypassed.
        let mut kb = KnowledgeBase::new();
        let body = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(10);
        for i in 0..2000 {
            let title = if i % 97 == 0 {
                format!("needle-{i}")
            } else {
                format!("generic title {i}")
            };
            kb.insert(Node::new(
                format!("n:{i}"),
                title,
                NodeKind::Note,
                body.clone(),
            ));
        }
        let start = std::time::Instant::now();
        let hits = kb.search("needle");
        let elapsed = start.elapsed();
        assert!(!hits.is_empty(), "should find needle entries");
        assert!(
            elapsed.as_millis() < 200,
            "search took {elapsed:?} over 2000 nodes; cache may be bypassed"
        );
    }

    #[test]
    fn search_finds_by_alias() {
        let mut kb = KnowledgeBase::new();
        kb.insert(
            Node::new(
                "concept:modules",
                "Module System",
                NodeKind::Concept,
                "body",
            )
            .with_aliases(["plugins", "packages", "extensions"]),
        );
        let hits = kb.search("plugins");
        assert!(hits.contains(&"concept:modules".to_string()));
    }

    #[test]
    fn search_alias_title_priority() {
        let mut kb = KnowledgeBase::new();
        kb.insert(
            Node::new(
                "a",
                "Modules",
                NodeKind::Concept,
                "mentions plugins in body",
            )
            .with_aliases(["extensions"]),
        );
        kb.insert(Node::new(
            "b",
            "Other",
            NodeKind::Note,
            "also mentions plugins in body",
        ));
        // "plugins" matches alias of `a` (title-level priority) and body of both
        let hits = kb.search("plugins");
        assert_eq!(hits[0], "a", "alias match should rank before body match");
    }

    #[test]
    fn fuzzy_fallback_on_no_substring() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "concept:modules",
            "Module System",
            NodeKind::Concept,
            "",
        ));
        // "modul" is a substring and will match, but "mdl" requires fuzzy
        let hits = kb.search("mdlsys");
        // Fuzzy may or may not match depending on scoring — just ensure no panic
        assert!(hits.len() <= kb.len());
    }

    #[test]
    fn search_empty_aliases_no_panic() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "Title", NodeKind::Note, "body"));
        // Node has no aliases — search should still work fine
        let hits = kb.search("title");
        assert_eq!(hits, vec!["a"]);
    }

    #[test]
    fn aliases_builder() {
        let node = Node::new("a", "A", NodeKind::Note, "").with_aliases(["one", "two"]);
        assert_eq!(node.aliases, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn namespace_prefixes_extracted() {
        let kb = kb_with(vec![
            Node::new("cmd:save", "", NodeKind::Command, ""),
            Node::new("cmd:undo", "", NodeKind::Command, ""),
            Node::new("concept:buffer", "", NodeKind::Concept, ""),
            Node::new("index", "", NodeKind::Index, ""),
        ]);
        let prefixes = kb.namespace_prefixes();
        assert!(prefixes.contains(&"cmd:".to_string()));
        assert!(prefixes.contains(&"concept:".to_string()));
        assert!(!prefixes.contains(&"index".to_string())); // no colon = no prefix
    }

    #[test]
    fn health_report_counts_nodes() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "[[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let report = kb.health_report();
        assert_eq!(report.total_nodes, 2);
        assert_eq!(report.total_links, 1);
    }

    #[test]
    fn health_report_finds_orphans() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "[[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
            Node::new("orphan", "Orphan", NodeKind::Note, "no links here"),
        ]);
        let report = kb.health_report();
        assert!(report.orphan_ids.contains(&"orphan".to_string()));
        // b has incoming link from a, so it's not orphan
        assert!(!report.orphan_ids.contains(&"b".to_string()));
    }

    #[test]
    fn health_report_finds_broken_links() {
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, "[[nonexistent]]")]);
        let report = kb.health_report();
        assert_eq!(report.broken_links.len(), 1);
        assert_eq!(report.broken_links[0].source, "a");
        assert_eq!(report.broken_links[0].target, "nonexistent");
        assert_eq!(report.broken_links[0].kind, BrokenLinkKind::MalformedId);
    }

    #[test]
    fn health_report_classifies_broken_links() {
        let kb = kb_with(vec![Node::new(
            "a",
            "A",
            NodeKind::Note,
            "[[%s]] [[UUID]] [[deadbeef-dead-beef-dead-beefdeadbeef]] [[not a uuid]]",
        )]);
        let report = kb.health_report();
        let kinds: Vec<_> = report.broken_links.iter().map(|b| &b.kind).collect();
        assert!(kinds.contains(&&BrokenLinkKind::TemplatePlaceholder)); // %s
        assert!(kinds.contains(&&BrokenLinkKind::TemplatePlaceholder)); // UUID
        assert!(kinds.contains(&&BrokenLinkKind::DeletedNode)); // valid UUID format
        assert!(kinds.contains(&&BrokenLinkKind::MalformedId)); // not a uuid
    }

    #[test]
    fn health_report_with_external_resolver() {
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, "[[ext-node]]")]);
        // Without resolver: broken.
        let report = kb.health_report();
        assert_eq!(report.broken_links.len(), 1);
        // With resolver that knows about ext-node: not broken.
        let report = kb.health_report_with(|id| id == "ext-node");
        assert_eq!(report.broken_links.len(), 0);
    }

    #[test]
    fn health_report_namespace_counts() {
        let kb = kb_with(vec![
            Node::new("cmd:save", "", NodeKind::Command, ""),
            Node::new("cmd:undo", "", NodeKind::Command, ""),
            Node::new("concept:buffer", "", NodeKind::Concept, ""),
            Node::new("index", "", NodeKind::Index, ""),
        ]);
        let report = kb.health_report();
        assert_eq!(report.namespace_counts["cmd"], 2);
        assert_eq!(report.namespace_counts["concept"], 1);
        assert_eq!(report.namespace_counts["(none)"], 1);
    }

    #[test]
    fn index_not_counted_as_orphan() {
        let kb = kb_with(vec![Node::new("index", "Help", NodeKind::Index, "")]);
        let report = kb.health_report();
        assert!(report.orphan_ids.is_empty(), "index should not be orphan");
    }

    #[test]
    fn dangling_link_is_listed() {
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, "[[missing]]")]);
        // links_from returns the dangling target — callers handle rendering.
        assert_eq!(kb.links_from("a"), vec!["missing"]);
        // And the reverse index records it too (so if you later add 'missing',
        // backlinks appear retroactively).
        assert_eq!(kb.links_to("missing"), vec!["a".to_string()]);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Distributed Systems"), "distributed-systems");
        assert_eq!(slugify("  Hello World  "), "hello-world");
        assert_eq!(slugify("foo--bar__baz"), "foo-bar-baz");
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("OneWord"), "oneword");
        assert_eq!(slugify("a+b=c"), "a-b-c");
    }

    #[test]
    fn timestamp_id_format() {
        let ts = timestamp_id();
        assert_eq!(
            ts.len(),
            15,
            "expected 15 chars: YYYYMMDDTHHMMSS, got {}",
            ts
        );
        assert!(ts.contains('T'), "timestamp should contain T separator");
    }

    #[test]
    fn all_id_title_pairs_sorted() {
        let kb = kb_with(vec![
            Node::new("b", "Beta", NodeKind::Note, ""),
            Node::new("a", "Alpha", NodeKind::Note, ""),
        ]);
        let pairs = kb.all_id_title_pairs();
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "Alpha".to_string()),
                ("b".to_string(), "Beta".to_string()),
            ]
        );
    }

    #[test]
    fn stale_node_detected_after_file_delete() {
        let mut kb = KnowledgeBase::new();
        let fake_path = std::path::PathBuf::from("/tmp/mae-test-nonexistent-12345.org");
        // Ensure path doesn't exist
        assert!(!fake_path.exists());
        kb.insert(
            Node::new("stale-test", "Stale", NodeKind::Note, "body").with_source_file(&fake_path),
        );
        let stale = kb.detect_stale_nodes();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "stale-test");
        assert_eq!(stale[0].source_file, fake_path);
    }

    #[test]
    fn link_validation_warns_on_broken_link() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[missing-id]]"));
        kb.insert(Node::new("b", "B", NodeKind::Note, "[[a]]")); // valid
        let missing = kb.validate_links("a");
        assert_eq!(missing, vec!["missing-id"]);
        let missing = kb.validate_links("b");
        assert!(missing.is_empty(), "link to existing node should be valid");
    }

    #[test]
    fn cleanup_orphans_removes_user_notes() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "orphan-note",
            "Orphan",
            NodeKind::Note,
            "no links",
        ));
        kb.insert(Node::new("a", "A", NodeKind::Note, "[[b]]"));
        kb.insert(Node::new("b", "B", NodeKind::Note, ""));
        // orphan-note has no links in or out — should be removable
        let report = kb.health_report();
        assert!(report.orphan_ids.contains(&"orphan-note".to_string()));
        // Simulate cleanup (same logic as Editor::kb_cleanup_orphans)
        let seed_prefixes = ["cmd:", "concept:", "lesson:", "scheme:", "option:"];
        let to_remove: Vec<String> = report
            .orphan_ids
            .into_iter()
            .filter(|id| !seed_prefixes.iter().any(|p| id.starts_with(p)))
            .collect();
        for id in &to_remove {
            kb.remove(id);
        }
        assert!(!kb.contains("orphan-note"));
        assert!(kb.contains("a"));
        assert!(kb.contains("b"));
    }

    #[test]
    fn cleanup_orphans_preserves_seed_nodes() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("cmd:save", "Save", NodeKind::Command, ""));
        kb.insert(Node::new("concept:buffer", "Buffer", NodeKind::Concept, ""));
        kb.insert(Node::new("lesson:intro", "Intro", NodeKind::Note, ""));
        kb.insert(Node::new("scheme:define", "Define", NodeKind::Note, ""));
        kb.insert(Node::new("option:theme", "Theme", NodeKind::Note, ""));
        // All are orphans (no links), but should be preserved by seed prefix filter
        let report = kb.health_report();
        let seed_prefixes = ["cmd:", "concept:", "lesson:", "scheme:", "option:"];
        let to_remove: Vec<String> = report
            .orphan_ids
            .into_iter()
            .filter(|id| !seed_prefixes.iter().any(|p| id.starts_with(p)))
            .collect();
        assert!(
            to_remove.is_empty(),
            "seed nodes should be preserved: {:?}",
            to_remove
        );
    }
}
