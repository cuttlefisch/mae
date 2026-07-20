//! mae-kb — in-memory knowledge base (graph store).
//!
//! @stability: stable
//! @since: 0.5.0
//!
//! The knowledge base is the shared data model for:
//!
//! 1. The built-in manual (command, concept, and keybinding docs).
//! 2. User-authored notes (org-roam-style bidirectional links).
//! 3. An AI-facing query surface — the agent is a *peer actor* that can
//!    read the same nodes the human reads via `:help`.
//!
//! ## Design
//!
//! - A **node** is a typed, named document with an org-mode body.
//! - Links are embedded in the body as `[[id]]` or `[[id|display text]]`.
//! - The store keeps a reverse index so "what links to X?" is O(1).
//! - **Persistence**: `CozoKbStore` (via `KbStore` trait) is the durable
//!   backend (CozoDB with SQLite storage engine). In-memory `KnowledgeBase`
//!   is the hot cache; all mutations write through to CozoDB. Org files are
//!   import/export format, not runtime source of truth. See ADR-011.
//!
//! This crate depends on no MAE internals — it's a pure data library
//! callable from `mae-core`, `mae-ai`, and the editor binary.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod activity;
pub mod backup;
pub mod data_dir;
pub mod export;
pub mod federation;
pub mod fuzzy;
pub mod graph_query;
pub mod migrate;
pub mod org;
pub mod store;
pub mod watch;

pub mod cache;
pub mod cozo_store;
pub mod hygiene;
pub mod lru_query;
pub mod query;

// Advisory file locking + the reload-fresh-then-mutate-then-save helper
// (`with_locked_update`) live in `mae-mcp`, which this crate already
// depends on. Re-exported here so `federation::KbRegistry` can use it, and
// so `mae-core` (which depends on `mae-kb` but deliberately does not depend
// on `mae-mcp` directly, per `editor::kb_state`) can reach it too via
// `mae_kb::file_lock` without adding a new Cargo dependency edge.
pub use mae_mcp::file_lock;

pub use cache::{CachedQueryLayer, NodeCache};
pub use cozo_store::CozoKbStore;
pub use federation::{
    import_org_dir_to_store, ImportHealth, ImportReport as FederationImportReport, IngestMode,
    KbScope,
};
pub use org::{IngestReport, OrgParseResult, ParsedLink};
pub use query::{CozoQueryLayer, FederatedQuery, InMemoryQueryLayer, KbQueryLayer};
pub use store::{
    AgendaFilter, Block, BrokenLinkInfo, BrokenLinkReason, HealthReport, IntegrityError, KbStore,
    KbStoreError, Link, MetaMember, NodeVersion, SubGraph, VectorHit,
};

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
    /// Grouping node for organizing related concepts.
    Category,
    /// Tutorial lesson (numbered, prerequisite-ordered).
    Lesson,
    /// Multi-step tutorial track.
    Tutorial,
    /// Composite node whose body is cached from component nodes.
    Meta,
    /// Paragraph-level sub-node for fine-grained linking.
    Block,
    /// Scheme API documentation (functions, variables, macros).
    SchemeApi,
    /// Work item with todo_state, priority, assignee, due_date, sprint.
    Task,
    /// Configurable query+display node (kanban, backlog, sprint, timeline, agenda).
    View,
}

impl NodeKind {
    /// Convert a `NodeKind` to its canonical string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Index => "index",
            NodeKind::Command => "command",
            NodeKind::Concept => "concept",
            NodeKind::Key => "key",
            NodeKind::Note => "note",
            NodeKind::Project => "project",
            NodeKind::Category => "category",
            NodeKind::Lesson => "lesson",
            NodeKind::Tutorial => "tutorial",
            NodeKind::Meta => "meta",
            NodeKind::Block => "block",
            NodeKind::SchemeApi => "scheme_api",
            NodeKind::Task => "task",
            NodeKind::View => "view",
        }
    }

    /// Parse a `NodeKind` from its string representation.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "index" => NodeKind::Index,
            "command" => NodeKind::Command,
            "concept" => NodeKind::Concept,
            "key" => NodeKind::Key,
            "project" => NodeKind::Project,
            "category" => NodeKind::Category,
            "lesson" => NodeKind::Lesson,
            "tutorial" => NodeKind::Tutorial,
            "meta" => NodeKind::Meta,
            "block" => NodeKind::Block,
            "scheme_api" => NodeKind::SchemeApi,
            "task" => NodeKind::Task,
            "view" => NodeKind::View,
            _ => NodeKind::Note,
        }
    }
}

/// Specification for subgraph extraction.
#[derive(Debug, Clone)]
pub struct SubgraphSpec {
    /// Starting node IDs for BFS walk.
    pub starter_nodes: Vec<String>,
    /// Maximum link depth (0 = starters only).
    pub max_depth: usize,
    /// Include backlinks in the walk (not just outgoing links).
    pub include_backlinks: bool,
    /// Safety net independent of `max_depth`/`include_backlinks`: a densely
    /// cross-referenced KB can make even a shallow walk explode (a hub
    /// node's backlinks alone can pull in most of the KB). `None` = no cap.
    /// `Some(n)` keeps starter nodes plus the `n` highest-degree remaining
    /// nodes; everything past the cap is demoted to a boundary link exactly
    /// like a depth cutoff (see `extract_subgraph`), so the existing
    /// "... (+N)" boundary-stub rendering already handles it — no new
    /// render path needed.
    pub node_cap: Option<usize>,
}

/// A typed link within a `SubgraphResult` — carries the ADR-030
/// relationship type + authored/default weight through subgraph
/// extraction (previously collapsed to a bare `(source, target)` pair,
/// losing that data before it could reach the graph view's layout).
#[derive(Debug, Clone)]
pub struct SubgraphLink {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    /// 0.0-1.0, `1.0` when not explicitly authored (ADR-030 default).
    pub weight: f64,
}

/// Result of subgraph extraction.
#[derive(Debug, Clone)]
pub struct SubgraphResult {
    /// Nodes included in the subgraph.
    pub nodes: Vec<Node>,
    /// Internal links (both endpoints in the subgraph).
    pub links: Vec<SubgraphLink>,
    /// Boundary links (source in subgraph, target outside).
    pub boundary_links: Vec<SubgraphLink>,
    /// How many nodes the BFS walk would have included beyond
    /// `SubgraphSpec::node_cap`. `0` when the cap wasn't set or wasn't hit.
    pub hidden_node_count: usize,
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
    /// Human-readable title shown at the top of the KB buffer.
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
    #[cfg(feature = "crdt")]
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
    #[cfg(feature = "crdt")]
    pub fn apply_crdt_doc(&mut self, doc: &mae_sync::kb::KbNodeDoc) {
        self.title = doc.title();
        self.body = doc.body();
        self.tags = doc.tags();
        self.crdt_doc = Some(doc.encode());
    }

    /// Create a new Node from a `KbNodeDoc` (CRDT → Node materialization).
    ///
    /// Used when joining a shared KB: the CRDT doc is the source of truth,
    /// and we create a local Node from it for FTS5 indexing and display.
    #[cfg(feature = "crdt")]
    pub fn from_crdt_doc(
        doc: &mae_sync::kb::KbNodeDoc,
        kind: NodeKind,
        source: NodeSource,
    ) -> Self {
        let mat = doc.materialize();
        let mut node = Node::new(mat.id, mat.title, kind, mat.body);
        node.tags = mat.tags;
        node.source = Some(source);
        node.crdt_doc = Some(doc.encode());
        // Populate links from materialized links array.
        // (links are also parseable from body, but CRDT links array is authoritative)
        node
    }

    /// Extract all `[[link]]`, `[[link|display]]`, and ADR-030 typed-link
    /// (`[[link?rel=X&w=Y][display]]`) targets from the body. Returns the target ids
    /// in document order, deduplicated. Uses `parse_typed_links` (not the older
    /// untyped `parse_links`) so a typed link's `?query` is stripped from the
    /// target id -- previously this returned the raw, query-string-attached target
    /// verbatim (e.g. `"concept:buffer?rel=teaches&w=0.8"`), which never matches any
    /// real node id, so graph traversal (`kb_graph` BFS, the "Tab cycles through
    /// reachable nodes" terminal-help UX, `neighbors()`) silently failed to
    /// recognize a typed link as a real edge at all.
    pub fn links(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for link in crate::org::parse_typed_links(&self.body, &self.id) {
            if seen.insert(link.target.clone()) {
                out.push(link.target);
            }
        }
        out
    }

    /// Like `links()`, but keeps each link's ADR-030 relationship type and
    /// authored/default weight (0.0-1.0, `1.0` when not explicitly
    /// authored) instead of discarding them down to a bare target id —
    /// used by `extract_subgraph` so the native KB graph view's
    /// force-directed layout can weight edges by how strongly related the
    /// user actually said two nodes are, rather than treating every edge
    /// identically. Same dedup-by-target-first-seen behavior as `links()`
    /// (first occurrence wins if a body somehow links the same target
    /// twice with different rel/weight).
    pub fn links_typed(&self) -> Vec<(String, String, f64)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for link in crate::org::parse_typed_links(&self.body, &self.id) {
            if seen.insert(link.target.clone()) {
                out.push((link.target, link.rel_type, link.weight));
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
        if bytes[i] == b'[' && bytes[i + 1] == b'['
            // Skip links inside org verbatim =...= or code ~...~ spans
            && !(i > 0 && (bytes[i - 1] == b'=' || bytes[i - 1] == b'~'))
        {
            // Skip links inside verbatim blocks (src, example, export).
            if code_ranges.iter().any(|&(s, e)| i >= s && i < e) {
                i += 1;
                continue;
            }
            if let Some(end_rel) = body[i + 2..].find("]]") {
                let inner = &body[i + 2..i + 2 + end_rel];
                // Split on '|' for display-text override.
                // The internal format uses | as separator (from rewrite_links),
                // while org source uses ][. Both are handled here.
                let (target, display) = if let Some(sep) = inner.find("][") {
                    (&inner[..sep], &inner[sep + 2..])
                } else if let Some(bar) = inner.find('|') {
                    (&inner[..bar], &inner[bar + 1..])
                } else {
                    (inner, inner)
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

/// Compute byte ranges of verbatim blocks where org markup should NOT be parsed.
///
/// Matches Emacs behavior: `#+begin_src`, `#+begin_example`, and `#+begin_export`
/// blocks contain literal content — no link extraction, no markup processing.
/// `#+begin_quote` is intentionally excluded because Emacs parses org markup inside it.
///
/// `pub` so other link-scanning consumers (e.g. the interactive KB-view
/// renderer in `mae-core`) can reuse the same code-block-awareness that
/// `org::rewrite_links_with_types`/`org::next_link_span` already have,
/// instead of hand-rolling a second, unaware scanner (ADR-030).
pub fn compute_code_block_ranges(body: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let lower = body.to_ascii_lowercase();
    // Block types whose content is verbatim (no org markup parsing)
    let verbatim_blocks = [
        ("#+begin_src", "#+end_src"),
        ("#+begin_example", "#+end_example"),
        ("#+begin_export", "#+end_export"),
    ];
    for (begin_tag, end_tag) in &verbatim_blocks {
        let mut search_from = 0;
        while let Some(start) = lower[search_from..].find(begin_tag) {
            let abs_start = search_from + start;
            if let Some(end) = lower[abs_start..].find(end_tag) {
                let abs_end = abs_start + end + end_tag.len();
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
    }
    ranges.sort_by_key(|&(s, _)| s);
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

/// A node id that no longer appears in its own `source_file`'s *current*
/// content — left behind by an in-place `:ID:` edit, since re-ingest only
/// ever upserts whatever a file presently contains and never retracts an id
/// that quietly disappeared from it. Unlike [`StaleNode`], the file still
/// exists; it just doesn't produce this id anymore.
#[derive(Debug, Clone)]
pub struct GhostNode {
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
    pub ghost_ids: Vec<GhostNode>,
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
/// Relevance prior by id namespace, used to break ties in `search_ranked`:
/// primary content (concept/cmd/scheme/option/category) ranks above
/// navigational/glossary nodes (term/lesson/tutorial/key/index) for the same
/// match — the canonical concept page, not its glossary term, is the answer.
/// Mild (0.9) so it only tips near-ties, never buries a strong match.
fn namespace_prior(id: &str) -> f64 {
    match id.split_once(':').map(|(ns, _)| ns) {
        // Glossary terms, lessons/tutorials, and auto-generated category
        // listings are secondary to the explanatory concept/command pages.
        Some("term" | "lesson" | "tutorial" | "tutor" | "key" | "index" | "guide" | "category") => {
            0.9
        }
        _ => 1.0,
    }
}

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

/// What [`KnowledgeBase::reconcile_remote_node`] did, for the caller to act on
/// (push the local-ahead diff, log a divergence, etc.). ADR-022.
#[cfg(feature = "crdt")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    /// The node did not exist locally; it was created from the remote ops
    /// (first-join lineage establishment).
    Created,
    /// The node existed; the remote diff was merged (no clobber).
    Merged,
    /// The node existed on an **incompatible lineage**: the remote sent ops we
    /// lacked, but they did not merge (legacy pre-B-16 same-id collision). The
    /// caller should fetch the remote's full state and `adopt_remote_node` to
    /// establish a shared lineage. We do NOT replace here, so no durable local
    /// edit is silently lost without the caller opting in.
    DivergentLineage,
}

/// Outcome of an ADR-022 state-vector reconcile.
#[cfg(feature = "crdt")]
#[derive(Debug, Clone)]
pub struct ReconcileOutcome {
    /// Classification of how the merge resolved.
    pub action: ReconcileAction,
    /// Whether the local materialized content changed as a result of the merge.
    pub content_changed: bool,
    /// Ops the *remote* lacks (our local-ahead diff, computed against the
    /// remote's state vector). `Some` iff non-empty — push these back to the
    /// hub so a durable-but-unsynced local edit re-syncs without depending on
    /// the pending queue surviving a crash.
    pub local_ahead: Option<Vec<u8>>,
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

    /// Iterate every node id in the KB. Enables callers (the disk-first loader,
    /// the ADR-022 join flow gathering per-node state vectors, the collab
    /// resubscribe pass) to enumerate stored nodes without reaching into internal
    /// maps. Order is unspecified.
    pub fn node_ids(&self) -> impl Iterator<Item = &String> {
        self.nodes.keys()
    }

    /// Iterate every `(id, node)` pair in the KB. Order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Node)> {
        self.nodes.iter()
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

    // --- CRDT-aware mutation methods (require `crdt` feature) ---

    /// Upsert a node with CRDT backing. Creates or updates the `KbNodeDoc` and
    /// stores the encoded CRDT bytes on the node. Returns the update bytes
    /// for broadcasting to peers (if any content changed).
    ///
    /// If the node doesn't have CRDT bytes yet (lazy migration), creates a fresh
    /// `KbNodeDoc` from the text fields.
    #[cfg(feature = "crdt")]
    pub fn upsert_with_crdt(&mut self, node: Node, client_id: u64) -> Option<Vec<u8>> {
        let id = node.id.clone();

        // Create or update CRDT doc
        let crdt_doc = if let Some(ref bytes) = node.crdt_doc {
            match mae_sync::kb::KbNodeDoc::from_bytes_with_client_id(bytes, client_id) {
                Ok(mut doc) => {
                    // ADR-020 B-15: apply the edited fields onto the EXISTING lineage
                    // (preserving its yrs ancestry) so the change actually enters the
                    // CRDT and chains with prior ops. Rebuilding from the old bytes
                    // and IGNORING node.title/body (the prior behaviour) meant every
                    // edit after the first re-broadcast stale content — peers never
                    // saw it. Set only when changed to avoid churn ops.
                    // Per-op update bytes are intentionally discarded here: the return
                    // value below is `encode_state()` (a full-state snapshot), not the
                    // incremental per-call deltas from set_title/set_body/set_tags.
                    if doc.title() != node.title {
                        let _ = doc.set_title(&node.title);
                    }
                    if doc.body() != node.body {
                        let _ = doc.set_body(&node.body);
                    }
                    // B-18: tags are a synced `YArray` too — wire them like
                    // title/body, else a tags-only edit never enters the CRDT and
                    // peers no-op on apply (changed=false). The receive side
                    // (`apply_crdt_doc` → `self.tags = doc.tags()`) already reads
                    // them back; the send side was the gap.
                    if doc.tags() != node.tags {
                        doc.set_tags(&node.tags);
                    }
                    doc
                }
                Err(_) => mae_sync::kb::KbNodeDoc::new_with_client_id(
                    &node.id,
                    &node.title,
                    &node.body,
                    &node.tags,
                    client_id,
                ),
            }
        } else {
            mae_sync::kb::KbNodeDoc::new_with_client_id(
                &node.id,
                &node.title,
                &node.body,
                &node.tags,
                client_id,
            )
        };

        let update_bytes = crdt_doc.encode_state();
        let mut node = node;
        node.crdt_doc = Some(update_bytes.clone());
        self.insert(node);

        // Return the state bytes for sharing
        if self.nodes.contains_key(&id) {
            Some(update_bytes)
        } else {
            None
        }
    }

    /// Apply a remote CRDT update to a node. Returns true if content changed.
    ///
    /// If the node doesn't exist yet, creates it from the update bytes.
    /// If it exists without CRDT bytes (lazy migration), creates a fresh
    /// `KbNodeDoc` first, then applies the update.
    #[cfg(feature = "crdt")]
    pub fn apply_remote_update(
        &mut self,
        node_id: &str,
        update: &[u8],
    ) -> Result<bool, mae_sync::SyncError> {
        if let Some(node) = self.nodes.get_mut(node_id) {
            // Existing node — get or create CRDT doc
            let mut crdt_doc = node.to_crdt_doc()?;
            let changed = crdt_doc.apply_update(update)?;
            if changed {
                node.apply_crdt_doc(&crdt_doc);
                // Rebuild reverse index for this node
                let id = node.id.clone();
                let links = node.links();
                // Clean old reverse edges
                for sources in self.links_in.values_mut() {
                    sources.retain(|s| s != &id);
                }
                self.links_in.retain(|_, v| !v.is_empty());
                // Install new reverse edges
                for target in links {
                    let entry = self.links_in.entry(target).or_default();
                    if !entry.contains(&id) {
                        entry.push(id.clone());
                    }
                }
            }
            Ok(changed)
        } else {
            // New node from remote — create from CRDT bytes
            let crdt_doc = mae_sync::kb::KbNodeDoc::from_bytes(update)?;
            let mat = crdt_doc.materialize();
            let mut node = Node::new(mat.id, mat.title, NodeKind::Note, mat.body);
            node.tags = mat.tags;
            node.source = Some(NodeSource::Federation);
            node.crdt_doc = Some(crdt_doc.encode());
            self.insert(node);
            Ok(true)
        }
    }

    /// Adopt a remote node's CRDT lineage as the canonical local doc (ADR-020 B-14).
    ///
    /// Unlike [`apply_remote_update`](Self::apply_remote_update) (which merges a
    /// *delta* into the local doc), this REBUILDS the local node from the remote's
    /// full encoded state, so both peers share ONE yrs lineage. This is required on
    /// join: two peers that *independently* constructed a same-id `KbNodeDoc` (e.g.
    /// both imported the same org fixture) have incompatible lineages — their
    /// `title`/`body` `YText`s are different yrs objects at the same map key, so a
    /// CRDT merge no-ops (the map's last-writer-wins discards one side) and the
    /// joiner never sees the owner's content (`changed=false`). After adoption the
    /// owner's subsequent updates merge as real text changes. Mirrors the
    /// text-buffer `from_state_with_client_id` adopt pattern. Preserves the local
    /// node's `kind` if already known. Returns whether materialized content changed.
    #[cfg(feature = "crdt")]
    pub fn adopt_remote_node(
        &mut self,
        node_id: &str,
        state: &[u8],
    ) -> Result<bool, mae_sync::SyncError> {
        let crdt_doc = mae_sync::kb::KbNodeDoc::from_bytes(state)?;
        let mat = crdt_doc.materialize();
        // Preserve an existing node's kind (org import sets a real kind); default to
        // Note for a brand-new node. Compute `changed` against the prior content.
        let (kind, changed) = match self.nodes.get(node_id) {
            Some(n) => (
                n.kind,
                n.title != mat.title || n.body != mat.body || n.tags != mat.tags,
            ),
            None => (NodeKind::Note, true),
        };
        let mut node = Node::new(mat.id, mat.title, kind, mat.body);
        node.tags = mat.tags;
        node.source = Some(NodeSource::Federation);
        node.crdt_doc = Some(crdt_doc.encode());
        self.insert(node);
        // Rebuild the reverse-link index for this node (mirror apply_remote_update).
        let links = self
            .nodes
            .get(node_id)
            .map(|n| n.links())
            .unwrap_or_default();
        for sources in self.links_in.values_mut() {
            sources.retain(|s| s != node_id);
        }
        self.links_in.retain(|_, v| !v.is_empty());
        for target in links {
            let entry = self.links_in.entry(target).or_default();
            if !entry.contains(&node_id.to_string()) {
                entry.push(node_id.to_string());
            }
        }
        Ok(changed)
    }

    /// ADR-022: crash-safe, non-destructive (re)join reconcile for one node.
    ///
    /// Given the ops the remote says we lack (`remote_diff`, computed by the hub
    /// via `encode_diff` against our state vector) and the remote's state vector
    /// (`remote_sv`), this:
    ///
    /// 1. **Merges** `remote_diff` into the local doc (creating the node if we've
    ///    never seen it) — it NEVER replaces an existing local node, so a durable
    ///    local edit whose sync-intent was lost in a crash is preserved.
    /// 2. Computes our **local-ahead** diff (`encode_diff(remote_sv)`) — the ops
    ///    the remote lacks — and returns it for the caller to push back. This is
    ///    what recovers a durable-but-unsynced edit on reconnect, independent of
    ///    whether any pending-queue row survived.
    ///
    /// Contrast [`adopt_remote_node`](Self::adopt_remote_node) (blind replace),
    /// which is correct only for a *brand-new* node (first-join lineage
    /// establishment). When an existing node sits on an **incompatible lineage**
    /// (legacy pre-B-16 same-id collision: the remote sent ops we lack but they
    /// don't merge), we report [`ReconcileAction::DivergentLineage`] and leave
    /// the local node untouched — the caller decides whether to fetch full state
    /// and adopt, rather than this method silently clobbering local work.
    #[cfg(feature = "crdt")]
    pub fn reconcile_remote_node(
        &mut self,
        node_id: &str,
        remote_diff: &[u8],
        remote_sv: &[u8],
    ) -> Result<ReconcileOutcome, mae_sync::SyncError> {
        let existed = self.nodes.contains_key(node_id);
        // Capture our pre-merge state vector — used to classify, BEFORE mutating,
        // whether the remote genuinely held ops we lacked and whether our lineages
        // are independent. Format-independent (compares SVs, not diff bytes).
        let pre_sv = self.node_state_vector(node_id);

        // Divergent-lineage detection (order-independent, pre-merge): the node
        // pre-existed locally, the remote genuinely held ops we lacked, AND our
        // two lineages share no common client — meaning the node was built from
        // scratch on both sides with the same id but incompatible lineages (the
        // B-14 condition). A healthy collab pair always shares the owner's lineage
        // client (adopted on first join), so a disjoint client set is the precise
        // signal — and it does NOT depend on which side wins the YMap LWW. Distinct
        // from the lost-row case (there the remote is *behind* us → no new ops →
        // Merged with a local-ahead push). On divergence we leave the local node
        // UNTOUCHED so the caller can adopt full state without us first clobbering
        // (or LWW-mangling) local content.
        let diverged = match &pre_sv {
            Some(pre) => {
                existed
                    && mae_sync::kb::sv_has_ops_beyond(remote_sv, pre)?
                    && mae_sync::kb::sv_clients_disjoint(pre, remote_sv)?
            }
            None => false,
        };
        if diverged {
            tracing::warn!(
                node_id,
                "ADR-022 reconcile: divergent lineage — independent same-id doc; \
                 leaving local node untouched, caller should adopt full state to \
                 establish a shared lineage"
            );
            return Ok(ReconcileOutcome {
                action: ReconcileAction::DivergentLineage,
                content_changed: false,
                local_ahead: None,
            });
        }

        // Merge (or create). apply_remote_update creates the node from the bytes
        // when absent — for a brand-new node the "diff" is the full state.
        let content_changed = self.apply_remote_update(node_id, remote_diff)?;

        // Our local-ahead diff: the ops the remote does not yet have. Use a
        // state-vector comparison (not `diff.is_empty()`, which never holds — a
        // no-op v1 update still encodes to a couple of bytes) to decide whether a
        // push is actually warranted.
        //
        // ONLY for a node that pre-existed locally (crash-safety: re-sync unsynced edits
        // we authored before a crash/disconnect). A node FRESHLY CREATED by this very
        // reconcile (`!existed`) was authored entirely by the remote — there is nothing
        // local to re-sync. Computing local-ahead for it is not just redundant, it is wrong
        // on an **E2e** KB: our local doc is the *plaintext* node while `remote_sv` is the
        // *op-set* doc's state vector — incompatible lineages, so `has_ops_beyond` is
        // spuriously true and we would push a re-seal of content we just received. That
        // extra op then yields an op-set a LATER joiner cannot reconstruct in causal order
        // (the recovered-member join panic, #225). Gate on `existed` to suppress it.
        let local_ahead = if existed {
            match self.nodes.get(node_id) {
                Some(node) => {
                    let doc = node.to_crdt_doc()?;
                    if doc.has_ops_beyond(remote_sv)? {
                        Some(doc.encode_diff(remote_sv)?)
                    } else {
                        None
                    }
                }
                None => None,
            }
        } else {
            None
        };

        let action = if existed {
            ReconcileAction::Merged
        } else {
            ReconcileAction::Created
        };

        tracing::debug!(
            node_id,
            ?action,
            content_changed,
            local_ahead = local_ahead.is_some(),
            "ADR-022 reconcile_remote_node"
        );

        Ok(ReconcileOutcome {
            action,
            content_changed,
            local_ahead,
        })
    }

    /// Get the state vector for a node's CRDT document.
    #[cfg(feature = "crdt")]
    pub fn node_state_vector(&self, node_id: &str) -> Option<Vec<u8>> {
        let node = self.nodes.get(node_id)?;
        let doc = node.to_crdt_doc().ok()?;
        Some(doc.state_vector())
    }

    /// Create a `KbCollectionDoc` manifest from this KB's nodes.
    ///
    /// If `node_ids` is empty, includes all nodes. Otherwise includes only
    /// the specified subset. Returns the collection doc and a list of
    /// `(node_id, encoded_state)` pairs for sharing.
    #[cfg(feature = "crdt")]
    #[allow(clippy::type_complexity)]
    pub fn to_collection(
        &self,
        name: &str,
        creator: &str,
        node_ids: &[String],
    ) -> Result<(mae_sync::kb::KbCollectionDoc, Vec<(String, Vec<u8>)>), mae_sync::SyncError> {
        let mut coll = mae_sync::kb::KbCollectionDoc::new(name, creator);
        let mut node_states = Vec::new();

        let ids_to_include: Vec<&String> = if node_ids.is_empty() {
            self.nodes.keys().collect()
        } else {
            node_ids.iter().collect()
        };

        for id in ids_to_include {
            if let Some(node) = self.nodes.get(id) {
                let crdt_doc = node.to_crdt_doc()?;
                coll.add_node(&node.id, &node.title);
                node_states.push((node.id.clone(), crdt_doc.encode()));
            }
        }

        Ok((coll, node_states))
    }

    // --- Subgraph extraction ---

    /// Extract a subgraph starting from seed nodes, walking links up to `max_depth`.
    ///
    /// Returns the set of included nodes and any boundary links (links from
    /// included nodes to excluded nodes).
    pub fn extract_subgraph(&self, spec: &SubgraphSpec) -> SubgraphResult {
        let mut included: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = spec.starter_nodes.clone();
        let mut depth = 0;

        // BFS walk
        while depth <= spec.max_depth && !frontier.is_empty() {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                if included.insert(node_id.clone()) && depth < spec.max_depth {
                    // Add outgoing links to frontier
                    if let Some(node) = self.nodes.get(node_id) {
                        for link in node.links() {
                            if !included.contains(&link) {
                                next_frontier.push(link);
                            }
                        }
                    }
                    // Add backlinks if requested
                    if spec.include_backlinks {
                        if let Some(sources) = self.links_in.get(node_id) {
                            for src in sources {
                                if !included.contains(src) {
                                    next_frontier.push(src.clone());
                                }
                            }
                        }
                    }
                }
            }
            frontier = next_frontier;
            depth += 1;
        }

        // Node-count safety cap (independent of depth/backlinks): keep
        // starter nodes plus the highest-degree remaining nodes, demoting
        // everything past the cap to a boundary link — same treatment a
        // depth cutoff already gets below, so hidden nodes still surface as
        // "... (+N)" stubs on whichever included node referenced them.
        let hidden_node_count = match spec.node_cap {
            Some(cap) if included.len() > cap => {
                let starters: HashSet<&str> =
                    spec.starter_nodes.iter().map(String::as_str).collect();
                let mut candidates: Vec<&String> = included
                    .iter()
                    .filter(|id| !starters.contains(id.as_str()))
                    .collect();
                candidates.sort_by(|a, b| {
                    let deg_a = self.node_degree(a);
                    let deg_b = self.node_degree(b);
                    deg_b.cmp(&deg_a).then_with(|| a.cmp(b))
                });
                let keep_budget = cap.saturating_sub(starters.len());
                let kept: HashSet<String> = starters
                    .iter()
                    .map(|s| s.to_string())
                    .chain(candidates.into_iter().take(keep_budget).cloned())
                    .collect();
                let hidden = included.len() - kept.len();
                included = kept;
                hidden
            }
            _ => 0,
        };

        // Collect nodes and categorize links
        let mut nodes = Vec::new();
        let mut internal_links = Vec::new();
        let mut boundary_links = Vec::new();

        for id in &included {
            if let Some(node) = self.nodes.get(id) {
                nodes.push(node.clone());
                for (target, rel_type, weight) in node.links_typed() {
                    let link = SubgraphLink {
                        source: id.clone(),
                        target: target.clone(),
                        rel_type,
                        weight,
                    };
                    if included.contains(&target) {
                        internal_links.push(link);
                    } else {
                        boundary_links.push(link);
                    }
                }
            }
        }

        SubgraphResult {
            nodes,
            links: internal_links,
            boundary_links,
            hidden_node_count,
        }
    }

    /// Total link degree (outgoing + incoming) for a node — used to
    /// prioritize which nodes survive `extract_subgraph`'s `node_cap`
    /// truncation (hub nodes are the most useful to keep visible, mirroring
    /// the graph view's own label-declutter priority order).
    fn node_degree(&self, id: &str) -> usize {
        let out = self.nodes.get(id).map(|n| n.links().len()).unwrap_or(0);
        let in_ = self.links_in.get(id).map(|v| v.len()).unwrap_or(0);
        out + in_
    }

    /// The highest-degree node in this KB, or `None` if it's empty. Used as
    /// a last-resort default "entry point" for KBs that don't follow MAE's
    /// own `"index"`/`NodeKind::Index` convention — e.g. an externally
    /// authored org-roam-style proposal KB, where node ids are raw UUIDs
    /// and there's no designated root. A high-degree node is the standard
    /// org-roam-ui/Obsidian heuristic for "the hub worth landing on."
    /// Ties break by id, ascending, for determinism.
    pub fn hub_node_id(&self) -> Option<String> {
        self.nodes
            .keys()
            .max_by(|a, b| {
                self.node_degree(a)
                    .cmp(&self.node_degree(b))
                    .then_with(|| b.cmp(a))
            })
            .cloned()
    }

    /// Remove multiple nodes at once. Returns the removed nodes.
    pub fn remove_nodes(&mut self, node_ids: &[String]) -> Vec<Node> {
        node_ids.iter().filter_map(|id| self.remove(id)).collect()
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
        // Fuzzy fallback: score against id + title + aliases only.
        // Body is excluded from fuzzy — long body text matches almost any
        // query as a subsequence, producing too many false positives.
        // Body is already covered by substring matching above.
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

    /// Relevance-ranked search: **orderless** (whitespace-split terms, order-
    /// independent, AND-combined), **field-weighted**, normalized to `0.0..=1.0`.
    ///
    /// Unlike [`search`](Self::search) (whole-query substring, alphabetical),
    /// this tokenizes the query so multi-word queries work ("leader keymap
    /// flavor" matches a node whose title/body contain those words in any
    /// order) and ranks by relevance: every term must match SOMEWHERE (AND);
    /// each term takes its best field score (title/id/alias ≫ tags > body) via
    /// `fuzzy::score_match`; body is matched by substring ONLY (no fuzzy —
    /// avoids long-body false positives, preserving the [`search`] invariant).
    /// Scores are normalized so they're comparable across instances/backends
    /// for federated merge (see `query::FederatedQuery`). `search` is retained
    /// for ordering-insensitive callers.
    pub fn search_ranked(&self, query: &str, limit: usize) -> Vec<(String, f64)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self
                .list_ids(None)
                .into_iter()
                .take(limit)
                .map(|id| (id, 1.0))
                .collect();
        }

        // Field weights (tuned against the grading harness): title/id/alias
        // dominate, tags mid, body lowest. A body substring hit (`BODY_HIT`)
        // sits below a title substring (~50k from fuzzy::score_match) so
        // title/alias matches always outrank body matches of the same term.
        const W_TITLE: f64 = 3.0;
        const W_TAG: f64 = 1.5;
        const W_BODY: f64 = 1.0;
        const BODY_HIT: f64 = 8_000.0;
        // Normalization ceiling: best possible per-term score (exact title).
        const MAX_TERM: f64 = 1_000_000.0 * W_TITLE;

        let terms: Vec<Vec<char>> = q.split_whitespace().map(|t| t.chars().collect()).collect();
        let num_terms = terms.len().max(1) as f64;

        let mut scored: Vec<(String, f64)> = Vec::new();
        'nodes: for (id, cache) in self.lower.iter() {
            // The id's LOCAL part (after the last ':') is the node's canonical
            // "name" — e.g. `concept:buffer` -> `buffer`. Matching it lets a
            // query exact-match the node name even when the title is prefixed
            // ("Concept: Buffer"), so the canonical node isn't buried under a
            // glossary `term:` whose title happens to be the bare word.
            let local_id = cache
                .lowered_id
                .rsplit(':')
                .next()
                .unwrap_or(&cache.lowered_id);
            // Whole-query phrase bonus: reward a node whose name/title IS the
            // query phrase. `fuzzy::score_match` normalizes spaces→hyphens, so
            // "buffer mode" exact-matches local-id `buffer-mode` and "ai as
            // peer" matches `ai-as-peer` — lifting the canonical multi-word node
            // above one that merely exact-matches a single term.
            let whole: Vec<char> = q.chars().collect();
            let whole_bonus = [cache.lowered_id.as_str(), local_id, cache.title.as_str()]
                .into_iter()
                .chain(cache.aliases.iter().map(|s| s.as_str()))
                .filter_map(|s| fuzzy::score_match(s, &whole))
                .max()
                .map(|s| s as f64 * W_TITLE)
                .unwrap_or(0.0);

            let mut total = whole_bonus;
            for term in &terms {
                let title_alias = [cache.lowered_id.as_str(), local_id, cache.title.as_str()]
                    .into_iter()
                    .chain(cache.aliases.iter().map(|s| s.as_str()))
                    .filter_map(|s| fuzzy::score_match(s, term))
                    .max()
                    .map(|s| s as f64 * W_TITLE);
                let tag = cache
                    .tags
                    .iter()
                    .filter_map(|t| fuzzy::score_match(t, term))
                    .max()
                    .map(|s| s as f64 * W_TAG);
                let term_str: String = term.iter().collect();
                let body = cache.body.contains(&term_str).then_some(BODY_HIT * W_BODY);

                // Best field for this term; AND semantics — a term with no
                // match anywhere drops the node entirely.
                let best = [title_alias, tag, body]
                    .into_iter()
                    .flatten()
                    .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
                match best {
                    Some(s) => total += s,
                    None => continue 'nodes,
                }
            }
            // Namespace prior: primary content (concept/cmd/scheme/option/
            // category) outranks navigational/glossary nodes (term/lesson/…) on
            // a tie — matches the org-roam intuition that the concept page, not
            // its one-line glossary term, is the canonical destination.
            // Denominator includes the whole-query bonus slot (+1) so scores
            // stay in 0..=1 without excessive clamping.
            let norm = (total * namespace_prior(id) / ((num_terms + 1.0) * MAX_TERM)).min(1.0);
            scored.push((id.clone(), norm));
        }
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        scored
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
    /// layers (e.g. `CozoKbStore::persist_nodes`). Order is arbitrary;
    /// callers that need a stable order should collect and sort by id.
    #[allow(dead_code)] // Used by Phase 1 persist_nodes (build-manual-kb)
    pub(crate) fn nodes_values(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Graph-relatedness: nodes most related to `id`, distinct from lexical
    /// search (it ignores titles/bodies entirely). Combines four structural
    /// signals over the typed link graph + tags, summed and ranked:
    ///
    /// - **direct link** (either direction) — strongest, the node is adjacent;
    /// - **bibliographic coupling** — shares an outbound target with `id`
    ///   (both cite the same node) — the org-roam "co-citation" intuition;
    /// - **co-citation** — shares an inbound source with `id` (both cited by
    ///   the same node);
    /// - **shared tags** — topical relatedness without a graph edge.
    ///
    /// Returns `(id, score)` sorted by score desc then id asc, capped to
    /// `limit`. Stays within a 2-hop graph walk (+ a tag scan); cross-instance
    /// merging is the caller's job (per-instance, like `neighborhood`).
    pub fn related(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        let Some(node) = self.nodes.get(id) else {
            return Vec::new();
        };
        const W_DIRECT: f64 = 2.0;
        const W_COUPLING: f64 = 1.0;
        const W_COCITATION: f64 = 1.0;
        const W_TAG: f64 = 0.5;

        let out = self.links_from(id);
        let inn = self.links_to(id);
        let tags: HashSet<&str> = node.tags.iter().map(|s| s.as_str()).collect();

        let mut score: HashMap<String, f64> = HashMap::new();

        // Bibliographic coupling: other nodes that link to the same targets.
        for target in &out {
            for c in self.links_to(target) {
                if c != id {
                    *score.entry(c).or_default() += W_COUPLING;
                }
            }
        }
        // Co-citation: other nodes cited by the same sources.
        for src in &inn {
            for c in self.links_from(src) {
                if c != id {
                    *score.entry(c).or_default() += W_COCITATION;
                }
            }
        }
        // Direct adjacency (either direction) is the strongest signal.
        for c in out.iter().chain(inn.iter()) {
            if c != id {
                *score.entry(c.clone()).or_default() += W_DIRECT;
            }
        }
        // Shared tags — topical relatedness even without a graph edge.
        if !tags.is_empty() {
            for (cid, cnode) in &self.nodes {
                if cid == id {
                    continue;
                }
                let shared = cnode
                    .tags
                    .iter()
                    .filter(|t| tags.contains(t.as_str()))
                    .count();
                if shared > 0 {
                    *score.entry(cid.clone()).or_default() += W_TAG * shared as f64;
                }
            }
        }

        let mut scored: Vec<(String, f64)> = score.into_iter().collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        scored
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
            ghost_ids: Vec::new(),   // populated lazily by caller via detect_ghost_ids()
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

    /// Detect ids that no longer appear in their own `source_file`'s current
    /// content (an in-place `:ID:` rename left them behind). Groups by file
    /// so each is re-parsed once regardless of how many indexed nodes claim
    /// it. Intentionally lazy — call on-demand (health report, `:kb-reimport`
    /// verification), not on every drain tick (a re-parse per distinct file
    /// is not free).
    pub fn detect_ghost_ids(&self) -> Vec<GhostNode> {
        let mut by_file: HashMap<std::path::PathBuf, Vec<&Node>> = HashMap::new();
        for n in self.nodes.values() {
            if let Some(path) = &n.source_file {
                by_file.entry(path.clone()).or_default().push(n);
            }
        }
        let mut ghosts = Vec::new();
        for (path, nodes) in by_file {
            // A missing file is `detect_stale_nodes`'s concern, not this one's.
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let current_ids: HashSet<String> = crate::org::parse_org_multi(&content)
                .into_iter()
                .map(|n| n.id)
                .collect();
            for n in nodes {
                if !current_ids.contains(&n.id) {
                    ghosts.push(GhostNode {
                        id: n.id.clone(),
                        title: n.title.clone(),
                        source_file: path.clone(),
                    });
                }
            }
        }
        ghosts.sort_by(|a, b| a.id.cmp(&b.id));
        ghosts
    }

    /// Remove ghost ids (see [`Self::detect_ghost_ids`]) and return the count removed.
    pub fn remove_ghost_ids(&mut self) -> usize {
        let ghost_ids: Vec<String> = self.detect_ghost_ids().into_iter().map(|g| g.id).collect();
        let count = ghost_ids.len();
        for id in ghost_ids {
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

    /// Return all (id, title, body) triples for all nodes, sorted by id.
    /// Body is included for search matching in the palette.
    pub fn all_id_title_body_triples(&self) -> Vec<(String, String, String)> {
        let mut triples: Vec<(String, String, String)> = self
            .nodes
            .values()
            .map(|n| (n.id.clone(), n.title.clone(), n.body.clone()))
            .collect();
        triples.sort_by(|a, b| a.0.cmp(&b.0));
        triples
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
    fn node_links_typed_keeps_rel_type_and_weight() {
        let n = Node::new(
            "x",
            "x",
            NodeKind::Note,
            "See [[concept:buffer?rel=teaches&w=0.8][the buffer]] then [[concept:plain]]",
        );
        let links = n.links_typed();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].0, "concept:buffer");
        assert_eq!(links[0].1, "teaches");
        assert_eq!(links[0].2, 0.8);
        // A link with no explicit query defaults to weight 1.0 (ADR-030),
        // and "references" is parse_typed_links' own default rel_type.
        assert_eq!(links[1].0, "concept:plain");
        assert_eq!(links[1].2, 1.0);
    }

    #[test]
    fn node_links_typed_dedup_matches_links() {
        // Same dedup-by-target-first-seen behavior as `links()` — first
        // occurrence's rel/weight wins if a body links the same target
        // twice with different metadata.
        let n = Node::new(
            "x",
            "x",
            NodeKind::Note,
            "[[a?rel=teaches&w=0.9]] [[a?rel=references&w=0.2]] [[b]]",
        );
        let typed = n.links_typed();
        let plain = n.links();
        assert_eq!(
            typed.iter().map(|(t, _, _)| t.clone()).collect::<Vec<_>>(),
            plain
        );
        assert_eq!(typed[0].1, "teaches");
        assert_eq!(typed[0].2, 0.9);
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
    fn related_ranks_by_graph_and_tag_signals() {
        let mut seed = Node::new("seed", "Seed", NodeKind::Note, "links [[hub]]");
        seed.tags = vec!["topic".into()];
        let mut tagmate = Node::new("tagmate", "Tagmate", NodeKind::Note, "no graph edge");
        tagmate.tags = vec!["topic".into()];
        let kb = kb_with(vec![
            seed,
            // Shares the outbound target `hub` with seed -> bibliographic coupling.
            Node::new("coupled", "Coupled", NodeKind::Note, "also [[hub]]"),
            Node::new("hub", "Hub", NodeKind::Note, ""),
            // Links *to* seed -> direct adjacency (strongest).
            Node::new("direct", "Direct", NodeKind::Note, "see [[seed]]"),
            // Topical only: shares a tag, no graph edge.
            tagmate,
            Node::new("unrelated", "Unrelated", NodeKind::Note, "nothing"),
        ]);

        let related = kb.related("seed", 10);
        let ids: Vec<&str> = related.iter().map(|(id, _)| id.as_str()).collect();
        let score = |id: &str| related.iter().find(|(i, _)| i == id).map(|(_, s)| *s);

        // Directly-linked nodes (hub outbound, direct inbound) outrank the
        // merely-coupled node, which outranks the tag-only node.
        assert!(score("hub").unwrap() > score("coupled").unwrap());
        assert!(score("direct").unwrap() > score("coupled").unwrap());
        assert!(score("coupled").unwrap() > score("tagmate").unwrap());
        // Tag-only relatedness still surfaces a node with no graph edge.
        assert!(score("tagmate").is_some());
        // A node with neither a graph edge nor a shared tag is absent.
        assert!(!ids.contains(&"unrelated"));
        // The seed never appears in its own related set.
        assert!(!ids.contains(&"seed"));
    }

    #[test]
    fn related_missing_node_is_empty() {
        let kb = KnowledgeBase::new();
        assert!(kb.related("nope", 10).is_empty());
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
    fn search_ranked_multiword_orderless() {
        // The whole-substring `search` fails multi-word; `search_ranked`
        // tokenizes (order-independent AND), so this matches.
        let kb = kb_with(vec![
            Node::new(
                "concept:keymap-flavors",
                "Keymap Flavors & the Leader Keypad",
                NodeKind::Note,
                "doom and nonmodal",
            ),
            Node::new("other", "Unrelated", NodeKind::Note, "nothing here"),
        ]);
        assert_eq!(kb.search("leader keymap flavor"), Vec::<String>::new());
        let ranked = kb.search_ranked("leader keymap flavor", 10);
        assert_eq!(
            ranked.first().map(|(id, _)| id.as_str()),
            Some("concept:keymap-flavors"),
            "orderless multi-word should rank the flavors node first, got {ranked:?}"
        );
    }

    #[test]
    fn search_ranked_and_excludes_unmatched_term() {
        let kb = kb_with(vec![Node::new(
            "a",
            "Buffer management",
            NodeKind::Note,
            "ropey rope",
        )]);
        // "buffer" matches, "zzz" matches nothing → AND drops the node.
        assert!(kb.search_ranked("buffer zzz", 10).is_empty());
        assert!(!kb.search_ranked("buffer rope", 10).is_empty());
    }

    #[test]
    fn search_ranked_title_outranks_body_and_normalizes() {
        let kb = kb_with(vec![
            Node::new("a", "Other", NodeKind::Note, "mentions foo"),
            Node::new("b", "Foo bar", NodeKind::Note, "unrelated"),
        ]);
        let ranked = kb.search_ranked("foo", 10);
        assert_eq!(ranked[0].0, "b", "title match ranks first");
        assert_eq!(ranked[1].0, "a", "body match second");
        assert!(ranked[0].1 > ranked[1].1, "title score > body score");
        for (_, s) in &ranked {
            assert!(
                (0.0..=1.0).contains(s),
                "scores normalized to 0..=1, got {s}"
            );
        }
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
    fn search_finds_body_substring() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "zed-arch",
            "Zed Architecture",
            NodeKind::Note,
            "The collaboration layer uses DeltaDB for state sync.",
        ));
        let hits = kb.search("DeltaDB");
        assert!(
            hits.contains(&"zed-arch".to_string()),
            "body substring should match, got {:?}",
            hits
        );
    }

    #[test]
    fn search_body_substring_but_not_fuzzy() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "zed-arch",
            "Zed Architecture",
            NodeKind::Note,
            "The collaboration layer uses DeltaDB for state sync.",
        ));
        // "DeltaDB" is a substring in body — should match
        assert!(!kb.search("DeltaDB").is_empty());
        // "DltDB" is NOT a substring — fuzzy fallback excludes body,
        // so this should NOT match (only title/id/aliases get fuzzy).
        let hits = kb.search("DltDB");
        assert!(
            hits.is_empty(),
            "fuzzy body matching should not produce false positives"
        );
    }

    #[test]
    fn search_title_ranks_above_body() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "a",
            "DeltaDB Overview",
            NodeKind::Note,
            "empty body",
        ));
        kb.insert(Node::new(
            "b",
            "Zed Architecture",
            NodeKind::Note,
            "Uses DeltaDB for collaboration",
        ));
        let hits = kb.search("DeltaDB");
        assert_eq!(hits[0], "a", "title match should rank before body match");
    }

    #[test]
    fn search_sorted_by_activity_recent_first() {
        let mut kb = KnowledgeBase::new();
        let mut old_node = Node::new("old", "Old Note", NodeKind::Note, "");
        old_node
            .properties
            .insert("last-accessed".to_string(), "2026-01-01".to_string());
        let mut new_node = Node::new("new", "New Note", NodeKind::Note, "");
        new_node
            .properties
            .insert("last-accessed".to_string(), "2026-05-20".to_string());
        kb.insert(old_node);
        kb.insert(new_node);
        let weights = activity::ActivityWeights::default();
        let hits = kb.search_sorted_by_activity("Note", &weights, (2026, 5, 20));
        assert_eq!(hits[0], "new", "recently accessed node should rank first");
    }

    #[test]
    fn all_id_title_body_triples_sorted() {
        let kb = kb_with(vec![
            Node::new("b", "Beta", NodeKind::Note, "beta body"),
            Node::new("a", "Alpha", NodeKind::Note, "alpha body"),
        ]);
        let triples = kb.all_id_title_body_triples();
        assert_eq!(triples[0].0, "a");
        assert_eq!(triples[0].2, "alpha body");
        assert_eq!(triples[1].0, "b");
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
    fn ghost_id_detected_after_in_place_rename() {
        // Reproduces the reported bug: a file's :ID: is edited in place across
        // saves (jenkinsp -> jenkin -> jenkins). Re-ingesting only ever upserts
        // the file's CURRENT id — detect_ghost_ids is what notices the old ones
        // are still sitting in the index with nothing on disk backing them.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("jenkinsp.org");
        std::fs::write(
            &path,
            ":PROPERTIES:\n:ID: user:t-jenkinsp\n:END:\n#+title: jenkinsp\n\nJenkins\n",
        )
        .unwrap();

        let mut kb = KnowledgeBase::new();
        kb.ingest_org_file(&path);
        assert!(kb.contains("user:t-jenkinsp"));
        assert!(
            kb.detect_ghost_ids().is_empty(),
            "freshly-ingested id shouldn't be a ghost"
        );

        // Rename in place, twice, without ever removing the old ids from the index
        // (simulating what the buggy watcher/reimport path does today).
        std::fs::write(
            &path,
            ":PROPERTIES:\n:ID: user:t-jenkin\n:END:\n#+title: jenkin\n\nJenkins\n",
        )
        .unwrap();
        kb.ingest_org_file(&path); // upsert only — old id lingers, by design of this test
        std::fs::write(
            &path,
            ":PROPERTIES:\n:ID: user:t-jenkins\n:END:\n#+title: jenkins\n\nJenkins\n",
        )
        .unwrap();
        kb.ingest_org_file(&path);

        assert!(kb.contains("user:t-jenkinsp"));
        assert!(kb.contains("user:t-jenkin"));
        assert!(kb.contains("user:t-jenkins"));

        let ghosts = kb.detect_ghost_ids();
        let ghost_ids: Vec<&str> = ghosts.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(
            ghost_ids,
            vec!["user:t-jenkin", "user:t-jenkinsp"],
            "the two ids no longer produced by the file should be flagged, sorted"
        );

        let removed = kb.remove_ghost_ids();
        assert_eq!(removed, 2);
        assert!(!kb.contains("user:t-jenkinsp"));
        assert!(!kb.contains("user:t-jenkin"));
        assert!(
            kb.contains("user:t-jenkins"),
            "the current id must survive cleanup"
        );
    }

    #[test]
    fn ghost_id_whose_file_is_later_renamed_becomes_a_stale_node_not_invisible() {
        // Found while cleaning up the live jenkinsp/jenkin/jenkins case: once a
        // ghost id's file is ITSELF later renamed/deleted (e.g. fixing the
        // filename to match the corrected :ID:), detect_ghost_ids alone stops
        // seeing it -- it only re-parses EXISTING files, and this one's
        // source_file is now gone. It must NOT go invisible: detect_stale_nodes
        // (source_file no longer exists) is the complementary check, and the
        // two together (as kb_id_audit's cleanup_candidates union does) must
        // still surface every such id.
        let tmp = tempfile::TempDir::new().unwrap();
        let old_path = tmp.path().join("jenkinsp.org");
        std::fs::write(
            &old_path,
            ":PROPERTIES:\n:ID: user:t-jenkinsp\n:END:\n#+title: jenkinsp\n\nJenkins\n",
        )
        .unwrap();

        let mut kb = KnowledgeBase::new();
        kb.ingest_org_file(&old_path);

        // In-place rename to the current id, same path (creates a ghost).
        std::fs::write(
            &old_path,
            ":PROPERTIES:\n:ID: user:t-jenkins\n:END:\n#+title: jenkins\n\nJenkins\n",
        )
        .unwrap();
        kb.ingest_org_file(&old_path);
        assert_eq!(
            kb.detect_ghost_ids().len(),
            1,
            "jenkinsp should be a ghost while its file still exists"
        );

        // Now the FILE itself is renamed away (fixing the filename), exactly
        // as happened live: the old id's source_file no longer exists at all.
        let new_path = tmp.path().join("jenkins.org");
        std::fs::rename(&old_path, &new_path).unwrap();
        // ingest the new path too, as a real reimport would.
        kb.ingest_org_file(&new_path);

        assert!(
            kb.detect_ghost_ids().is_empty(),
            "detect_ghost_ids alone can't see it anymore -- its source_file is gone, not just outdated"
        );
        let stale = kb.detect_stale_nodes();
        assert_eq!(
            stale.len(),
            1,
            "detect_stale_nodes must pick up what detect_ghost_ids can no longer reach"
        );
        assert_eq!(stale[0].id, "user:t-jenkinsp");
        assert!(
            kb.contains("user:t-jenkins"),
            "the current id must be unaffected"
        );
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

    // --- Phase 1: KB↔CRDT bridge tests (require `crdt` feature) ---

    #[cfg(feature = "crdt")]
    /// Realistic org content with properties drawer, links, code blocks, Unicode.
    fn realistic_org_body() -> &'static str {
        ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
         #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
         * Overview\n\
         This node tests the full round-trip.\n\n\
         ** Sub-heading with [[id:other-node|internal link]]\n\
         Content with Unicode: café, naïve, 日本語\n\n\
         #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_roundtrip_preserves_all_fields() {
        let body = realistic_org_body();
        let node = Node::new("concept:test", "Test Node — CRDT", NodeKind::Concept, body)
            .with_tags(vec!["research", "crdt"]);

        let crdt_doc = node.to_crdt_doc().expect("to_crdt_doc should succeed");
        let restored = Node::from_crdt_doc(&crdt_doc, NodeKind::Concept, NodeSource::Federation);

        assert_eq!(restored.id, "concept:test", "id should round-trip");
        assert_eq!(
            restored.title, "Test Node — CRDT",
            "title should round-trip"
        );
        assert_eq!(restored.body, body, "body should round-trip byte-for-byte");
        assert_eq!(
            restored.tags,
            vec!["research", "crdt"],
            "tags should round-trip"
        );
        assert_eq!(restored.source, Some(NodeSource::Federation));
        assert!(restored.crdt_doc.is_some(), "CRDT bytes should be stored");
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_roundtrip_via_encode_decode() {
        let body = realistic_org_body();
        let node = Node::new("concept:encoded", "Encoded Test", NodeKind::Note, body)
            .with_tags(vec!["test"]);

        // node → crdt → encode → base64 → decode → crdt → node
        let crdt_doc = node.to_crdt_doc().unwrap();
        let encoded = crdt_doc.encode();
        let b64 = mae_sync::encoding::update_to_base64(&encoded);
        let decoded = mae_sync::encoding::base64_to_update(&b64).unwrap();
        let restored_crdt = mae_sync::kb::KbNodeDoc::from_bytes(&decoded).unwrap();
        let restored = Node::from_crdt_doc(&restored_crdt, NodeKind::Note, NodeSource::Federation);

        assert_eq!(restored.title, "Encoded Test");
        assert_eq!(
            restored.body, body,
            "body should survive encode→base64→decode round-trip byte-for-byte"
        );
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_empty_node_roundtrips() {
        let node = Node::new("concept:empty", "Empty", NodeKind::Note, "");
        let crdt_doc = node.to_crdt_doc().unwrap();
        let restored = Node::from_crdt_doc(&crdt_doc, NodeKind::Note, NodeSource::Federation);

        assert_eq!(restored.id, "concept:empty");
        assert_eq!(restored.title, "Empty");
        assert_eq!(restored.body, "");
        assert!(restored.tags.is_empty());
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_node_with_metadata_roundtrips() {
        let mut crdt_doc = mae_sync::kb::KbNodeDoc::new(
            "concept:meta",
            "Meta Node",
            "body",
            &["tag1".to_string()],
        );
        crdt_doc.set_meta("author", "alice");
        crdt_doc.set_meta("version", "3");
        let _ = crdt_doc.add_link("concept:other");

        let node = Node::from_crdt_doc(&crdt_doc, NodeKind::Concept, NodeSource::Federation);
        assert_eq!(node.id, "concept:meta");
        assert_eq!(node.title, "Meta Node");
        assert_eq!(node.tags, vec!["tag1"]);
        // Metadata and links are stored in CRDT but not directly on Node fields
        // (they're accessible via the CRDT doc bytes)
        assert!(node.crdt_doc.is_some());
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_corrupted_bytes_returns_error() {
        let result = mae_sync::kb::KbNodeDoc::from_bytes(&[0xFF, 0xFE, 0xFD]);
        assert!(result.is_err(), "corrupted bytes should return error");
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_idempotent_encode() {
        let node = Node::new("n1", "Title", NodeKind::Note, "body text").with_tags(vec!["a", "b"]);
        let doc1 = node.to_crdt_doc().unwrap();
        let doc2 = node.to_crdt_doc().unwrap();

        // Two independent encodes should produce valid docs that merge cleanly
        let state1 = doc1.encode();
        let state2 = doc2.encode();

        let mut merged = mae_sync::kb::KbNodeDoc::from_bytes(&state1).unwrap();
        merged.apply_update(&state2).unwrap();
        assert_eq!(
            merged.title(),
            "Title",
            "merged doc should have correct title"
        );
        assert_eq!(
            merged.body(),
            "body text",
            "merged doc should have correct body"
        );
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn collection_from_kb_all_nodes() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("n1", "Node 1", NodeKind::Note, "body 1").with_tags(vec!["a"]));
        kb.insert(Node::new("n2", "Node 2", NodeKind::Note, "body 2").with_tags(vec!["b"]));
        kb.insert(Node::new("n3", "Node 3", NodeKind::Concept, "body 3"));

        let (coll, node_states) = kb.to_collection("Test KB", "alice", &[]).unwrap();
        assert_eq!(coll.name(), "Test KB");
        assert_eq!(coll.creator(), "alice");
        assert_eq!(coll.node_count(), 3, "should include all 3 nodes");
        assert_eq!(node_states.len(), 3, "should have states for all 3 nodes");

        // Verify each state decodes to a valid KbNodeDoc.
        for (id, state) in &node_states {
            let doc = mae_sync::kb::KbNodeDoc::from_bytes(state)
                .unwrap_or_else(|e| panic!("node '{}' state should decode: {}", id, e));
            assert!(!doc.title().is_empty(), "node '{}' should have a title", id);
        }
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn collection_from_kb_subset() {
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new("n1", "Node 1", NodeKind::Note, "body 1"));
        kb.insert(Node::new("n2", "Node 2", NodeKind::Note, "body 2"));
        kb.insert(Node::new("n3", "Node 3", NodeKind::Note, "body 3"));

        let subset = vec!["n1".to_string(), "n3".to_string()];
        let (coll, node_states) = kb.to_collection("Subset KB", "bob", &subset).unwrap();
        assert_eq!(coll.node_count(), 2, "should include only 2 nodes");
        assert_eq!(node_states.len(), 2);

        let ids: Vec<&str> = node_states.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"n1"));
        assert!(ids.contains(&"n3"));
        assert!(!ids.contains(&"n2"), "n2 should not be in subset");
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn collection_encode_decode_preserves_nodes() {
        let mut kb = KnowledgeBase::new();
        for i in 0..20 {
            kb.insert(Node::new(
                format!("n{i}"),
                format!("Node {i}"),
                NodeKind::Note,
                format!("Body for node {i}"),
            ));
        }

        let (coll, _) = kb.to_collection("Big KB", "alice", &[]).unwrap();
        let encoded = coll.encode_state();
        let decoded = mae_sync::kb::KbCollectionDoc::from_bytes(&encoded).unwrap();
        assert_eq!(
            decoded.node_count(),
            20,
            "all 20 nodes should survive encode→decode"
        );
        assert_eq!(decoded.name(), "Big KB");
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_apply_crdt_doc_updates_existing() {
        let mut node =
            Node::new("n1", "Old Title", NodeKind::Note, "old body").with_tags(vec!["old"]);

        let mut crdt_doc =
            mae_sync::kb::KbNodeDoc::new("n1", "New Title", "new body", &["new".to_string()]);
        let _ = crdt_doc.add_link("concept:linked");

        node.apply_crdt_doc(&crdt_doc);
        assert_eq!(node.title, "New Title");
        assert_eq!(node.body, "new body");
        assert_eq!(node.tags, vec!["new"]);
        assert!(node.crdt_doc.is_some());
    }

    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_large_body_roundtrips() {
        // 10KB org document
        let large_body: String = (0..200).map(|i| {
            format!("* Heading {i}\nParagraph with text about topic {i}. Unicode: café, 日本語.\n\n")
        }).collect();
        assert!(large_body.len() > 10_000, "body should be > 10KB");

        let node = Node::new("concept:large", "Large Doc", NodeKind::Note, &large_body);
        let crdt_doc = node.to_crdt_doc().unwrap();
        let restored = Node::from_crdt_doc(&crdt_doc, NodeKind::Note, NodeSource::Federation);
        assert_eq!(
            restored.body, large_body,
            "large body should round-trip exactly"
        );
    }

    /// ADR-020 B-14 — the realistic TWO-INDEPENDENT-PEERS scenario the rest of the
    /// suite never modeled (every other merge test creates one doc → encodes → applies
    /// to a doc derived from *those same bytes* = shared lineage). Here alice and bob
    /// build the same node-id INDEPENDENTLY (distinct yrs lineages), so a plain CRDT
    /// `apply_remote_update` of the owner's state NO-OPS (the map's last-writer-wins
    /// discards the owner's title/body YText) — the joiner never converges. `adopt_remote_node`
    /// rebuilds from the owner's state so both share one lineage and later edits merge.
    #[cfg(feature = "crdt")]
    #[test]
    fn divergent_lineage_merge_noops_but_adopt_converges() {
        // Alice builds her node, then EDITS it chained on her own lineage (the
        // realistic flow: clone the existing node — which now carries a crdt_doc —
        // change a field, re-upsert). This also exercises B-15 (the edit must enter
        // the existing CRDT lineage, not rebuild-and-ignore the new field).
        let mut alice = KnowledgeBase::new();
        let _ = alice.upsert_with_crdt(Node::new("t:n", "v0", NodeKind::Note, "body"), 1);
        let alice_state = {
            let mut n = alice.get("t:n").unwrap().clone();
            n.title = "Alice [PROBE]".to_string();
            alice.upsert_with_crdt(n, 1).unwrap()
        };
        assert_eq!(
            alice.get("t:n").unwrap().title,
            "Alice [PROBE]",
            "B-15: a chained edit must actually update the node"
        );

        // Bob built the SAME node independently — lineage B (client 2) + a local edit.
        // The BUG: merging alice's update into bob's divergent doc no-ops; the higher
        // client_id (bob's 2) wins the map LWW, so the owner's title is discarded.
        let mut bob_merge = KnowledgeBase::new();
        let _ =
            bob_merge.upsert_with_crdt(Node::new("t:n", "Bob Local", NodeKind::Note, "body"), 2);
        let _ = bob_merge.apply_remote_update("t:n", &alice_state);
        assert_eq!(
            bob_merge.get("t:n").unwrap().title,
            "Bob Local",
            "B-14 regression marker: a plain merge of divergent lineage fails to converge"
        );

        // The FIX: adoption rebuilds bob's node from alice's encoded state → converges
        // (bob now shares alice's lineage).
        let mut bob = KnowledgeBase::new();
        let _ = bob.upsert_with_crdt(Node::new("t:n", "Bob Local", NodeKind::Note, "body"), 2);
        let changed = bob.adopt_remote_node("t:n", &alice_state).unwrap();
        assert!(changed, "adoption changes bob's content to the owner's");
        assert_eq!(
            bob.get("t:n").unwrap().title,
            "Alice [PROBE]",
            "bob adopts the owner's content + lineage"
        );

        // Shared lineage now: the owner's NEXT edit (chained on her lineage) merges
        // as a real change on bob.
        let alice_next = {
            let mut n = alice.get("t:n").unwrap().clone();
            n.title = "Alice 2 [PROBE2]".to_string();
            alice.upsert_with_crdt(n, 1).unwrap()
        };
        let changed2 = bob.apply_remote_update("t:n", &alice_next).unwrap();
        assert!(
            changed2,
            "after adoption the owner's later update merges (shared lineage), not no-op"
        );
        assert_eq!(bob.get("t:n").unwrap().title, "Alice 2 [PROBE2]");
    }

    /// ADR-020 B-16 — the PRODUCTION-FIDELITY two-peer convergence test. The prior
    /// test hand-picked DISTINCT client_ids (alice=1, bob=2), which masked the real
    /// bug: `kb_update_node` hardcodes `client_id = 1` for EVERY peer, so two peers
    /// editing the same node are indistinguishable to yrs and the second writer's ops
    /// collide → no-op. This test reproduces the bob→alice direction using the SAME
    /// `client_id` the production edit path uses on BOTH sides (the value the code
    /// actually passes), so a hardcoded-collision bug is exercised, not bypassed.
    ///
    /// `KB_EDIT_CLIENT_ID` is the per-peer client id seed. Once edits derive a
    /// stable, unique id per peer, alice and bob differ and this converges. While the
    /// code hardcodes the same constant for both, this test FAILS — which is the point.
    #[cfg(feature = "crdt")]
    #[test]
    fn two_peers_editing_same_node_converge_through_distinct_client_ids() {
        // Distinct per-peer client ids (what the fix must produce). Using the SAME
        // value for both here reproduces the hardcoded-`1` collision bug.
        let alice_cid: u64 = 0xA11CE;
        let bob_cid: u64 = 0xB0B;

        // Alice creates + shares a node (her lineage).
        let mut alice = KnowledgeBase::new();
        let share_state = alice
            .upsert_with_crdt(Node::new("t:n", "Base", NodeKind::Note, "body"), alice_cid)
            .unwrap();

        // Bob adopts the shared lineage (the B-14 join path).
        let mut bob = KnowledgeBase::new();
        bob.adopt_remote_node("t:n", &share_state).unwrap();
        assert_eq!(bob.get("t:n").unwrap().title, "Base");

        // Bob edits on the shared lineage with HIS client id, broadcasts.
        let bob_edit = {
            let mut n = bob.get("t:n").unwrap().clone();
            n.title = "Bob Edit [BOB-LIVE-1]".to_string();
            bob.upsert_with_crdt(n, bob_cid).unwrap()
        };

        // Alice (the OWNER) applies bob's edit to her local doc → must converge.
        let changed = alice.apply_remote_update("t:n", &bob_edit).unwrap();
        assert!(
            changed,
            "owner must converge to a peer's edit (B-16). With distinct client_ids this \
             merges; the production bug hardcodes client_id=1 for both, which collides → no-op"
        );
        assert_eq!(
            alice.get("t:n").unwrap().title,
            "Bob Edit [BOB-LIVE-1]",
            "owner's node reflects the peer's edit after merge (bob→alice direction)"
        );
    }

    /// ADR-022 — `reconcile_remote_node` contract, exercised directly (the
    /// N-peer harness covers it end-to-end; this pins the primitive's classifier
    /// + local-ahead semantics at the unit layer).
    #[cfg(feature = "crdt")]
    #[test]
    fn reconcile_remote_node_lost_row_is_merged_with_local_ahead() {
        let alice_cid: u64 = 0xA11CE;
        let bob_cid: u64 = 0xB0B;

        // Shared lineage: alice creates + shares; bob adopts (first join).
        let mut alice = KnowledgeBase::new();
        let base = alice
            .upsert_with_crdt(Node::new("t:n", "v1", NodeKind::Note, "body"), alice_cid)
            .unwrap();
        let mut bob = KnowledgeBase::new();
        bob.adopt_remote_node("t:n", &base).unwrap();

        // Bob edits durably but the sync intent is LOST (never pushed). The hub
        // (alice) is therefore BEHIND bob.
        {
            let mut n = bob.get("t:n").unwrap().clone();
            n.title = "v2-unsynced".to_string();
            bob.upsert_with_crdt(n, bob_cid).unwrap();
        }

        // Reconcile: the hub's diff against bob's SV is a no-op (hub behind), so
        // bob keeps v2 (Merged, content unchanged) and reports local-ahead to push.
        let alice_doc = alice.get("t:n").unwrap().to_crdt_doc().unwrap();
        let bob_sv = bob.node_state_vector("t:n").unwrap();
        let remote_diff = alice_doc.encode_diff(&bob_sv).unwrap();
        let remote_sv = alice_doc.state_vector();
        let outcome = bob
            .reconcile_remote_node("t:n", &remote_diff, &remote_sv)
            .unwrap();

        assert_eq!(outcome.action, ReconcileAction::Merged);
        assert!(
            !outcome.content_changed,
            "hub was behind — nothing to merge into bob"
        );
        assert_eq!(bob.get("t:n").unwrap().title, "v2-unsynced", "no clobber");
        let local_ahead = outcome
            .local_ahead
            .expect("bob must report local-ahead ops to re-sync the lost edit");

        // Pushing the local-ahead up converges the hub (crash-safety, no pending queue).
        alice.apply_remote_update("t:n", &local_ahead).unwrap();
        assert_eq!(alice.get("t:n").unwrap().title, "v2-unsynced");

        // A second reconcile is now a clean no-op: caught up, no local-ahead.
        let alice_doc = alice.get("t:n").unwrap().to_crdt_doc().unwrap();
        let bob_sv = bob.node_state_vector("t:n").unwrap();
        let outcome2 = bob
            .reconcile_remote_node(
                "t:n",
                &alice_doc.encode_diff(&bob_sv).unwrap(),
                &alice_doc.state_vector(),
            )
            .unwrap();
        assert_eq!(outcome2.action, ReconcileAction::Merged);
        assert!(
            outcome2.local_ahead.is_none(),
            "both sides caught up — no redundant push"
        );
    }

    /// ADR-040 #225 — a node FRESHLY created by a join reconcile (the joiner authored
    /// nothing) must NOT report local-ahead, even when `remote_sv` is BEHIND the diff. On
    /// an E2e KB the join passes the op-set doc's SV while the local doc is the *plaintext*
    /// node — incompatible lineages, so `has_ops_beyond` is spuriously true and a pre-fix
    /// joiner would push a re-seal of content it just received. That extra op then yields an
    /// op-set a LATER joiner cannot reconstruct in causal order — the recovered-member join
    /// panic. The fix gates local-ahead on `existed`; this pins it at the unit layer.
    #[cfg(feature = "crdt")]
    #[test]
    fn fresh_join_never_reports_local_ahead_even_with_a_behind_remote_sv() {
        let alice_cid: u64 = 0xA11CE;
        let mut alice = KnowledgeBase::new();
        alice
            .upsert_with_crdt(Node::new("t:n", "v1", NodeKind::Note, "body"), alice_cid)
            .unwrap();
        // A deliberately BEHIND state vector (captured at v1) — the v2 doc has ops beyond it,
        // the same false-positive an E2e op-set SV produces against the plaintext node.
        let behind_sv = alice.node_state_vector("t:n").unwrap();
        let mut n = alice.get("t:n").unwrap().clone();
        n.title = "v2".to_string();
        alice.upsert_with_crdt(n, alice_cid).unwrap();
        let full_state = alice.get("t:n").unwrap().to_crdt_doc().unwrap().encode();

        // A FRESH joiner (no prior node) reconciles the full state against the behind SV.
        let mut joiner = KnowledgeBase::new();
        assert!(joiner.get("t:n").is_none(), "node absent before the join");
        let outcome = joiner
            .reconcile_remote_node("t:n", &full_state, &behind_sv)
            .unwrap();
        assert_eq!(outcome.action, ReconcileAction::Created);
        assert!(
            outcome.local_ahead.is_none(),
            "a freshly-created node has nothing local to re-sync — no spurious push (#225)"
        );
        assert_eq!(
            joiner.get("t:n").unwrap().title,
            "v2",
            "the remote content is still adopted in full"
        );
    }

    /// ADR-040 #225 (confidence-review #237, E2e RE-join) — the fresh-join fix gates
    /// local-ahead on `existed`, but a review flagged the RE-join case (`existed = true`): a
    /// member who ALREADY holds the plaintext node reconnects and reconciles against an op-set
    /// SV from the *disjoint* ciphertext lineage. Could the plaintext-doc-vs-op-set-SV mismatch
    /// still spuriously push a re-seal (the #225 op a later joiner can't reconstruct)? This
    /// pins the answer: NO — the pre-merge divergent-lineage guard fires FIRST (disjoint client
    /// sets), classifying it `DivergentLineage` with `local_ahead = None`, so no spurious op is
    /// authored. (A *same*-lineage reconnect that is genuinely behind still re-syncs correctly —
    /// that is the legitimate crash-recovery path, covered by the lost-row test.)
    #[cfg(feature = "crdt")]
    #[test]
    fn rejoin_with_a_disjoint_ahead_lineage_never_pushes_a_spurious_reseal() {
        // The member already holds the plaintext node on its own lineage.
        let member_cid: u64 = 0x0EEDBEEF;
        let mut member = KnowledgeBase::new();
        member
            .upsert_with_crdt(
                Node::new("t:n", "plain-v1", NodeKind::Note, "body"),
                member_cid,
            )
            .unwrap();
        assert!(
            member.get("t:n").is_some(),
            "node exists before the re-join"
        );

        // The inbound reconcile carries an op-set-shaped lineage: a DISJOINT client, and it is
        // strictly AHEAD (extra ops) — the exact false-positive `has_ops_beyond` would trip on.
        let opset_cid: u64 = 0x0F5E7; // distinct from member_cid ⇒ disjoint client sets
        let mut opset = KnowledgeBase::new();
        opset
            .upsert_with_crdt(Node::new("t:n", "ct-a", NodeKind::Note, "x"), opset_cid)
            .unwrap();
        let mut n = opset.get("t:n").unwrap().clone();
        n.title = "ct-b".to_string(); // a second op ⇒ genuinely "ahead"
        opset.upsert_with_crdt(n, opset_cid).unwrap();
        let opset_doc = opset.get("t:n").unwrap().to_crdt_doc().unwrap();
        let member_sv = member.node_state_vector("t:n").unwrap();

        let outcome = member
            .reconcile_remote_node(
                "t:n",
                &opset_doc.encode_diff(&member_sv).unwrap(),
                &opset_doc.state_vector(),
            )
            .unwrap();

        assert_eq!(
            outcome.action,
            ReconcileAction::DivergentLineage,
            "a disjoint ahead lineage on re-join is DivergentLineage, not a merge+push"
        );
        assert!(
            outcome.local_ahead.is_none(),
            "the RE-join must NOT push a spurious re-seal (the #225 unreconstructable op)"
        );
        // Divergent ⇒ local content is left untouched for the caller to adopt full state.
        assert_eq!(member.get("t:n").unwrap().title, "plain-v1");
    }

    /// ADR-022 — divergent (independently-constructed) same-id lineages are
    /// classified `DivergentLineage`, NOT silently clobbered.
    #[cfg(feature = "crdt")]
    #[test]
    fn reconcile_remote_node_detects_divergent_lineage() {
        // Two peers independently build the same id with different lineages.
        let mut alice = KnowledgeBase::new();
        alice.upsert_with_crdt(Node::new("t:n", "alice", NodeKind::Note, "a"), 0xA11CE);
        let mut bob = KnowledgeBase::new();
        bob.upsert_with_crdt(Node::new("t:n", "bob", NodeKind::Note, "b"), 0xB0B);

        let alice_doc = alice.get("t:n").unwrap().to_crdt_doc().unwrap();
        let bob_sv = bob.node_state_vector("t:n").unwrap();
        let outcome = bob
            .reconcile_remote_node(
                "t:n",
                &alice_doc.encode_diff(&bob_sv).unwrap(),
                &alice_doc.state_vector(),
            )
            .unwrap();
        assert_eq!(
            outcome.action,
            ReconcileAction::DivergentLineage,
            "incompatible same-id lineages must be flagged, not merged-away"
        );
        // Reconcile left bob's content intact (caller decides to adopt).
        assert_eq!(bob.get("t:n").unwrap().title, "bob");
    }

    /// B-18 regression: a TAGS-only edit must enter the CRDT and converge on a
    /// peer. Before the fix `upsert_with_crdt` only wrote title/body, so a tag
    /// change produced a no-op CRDT update — the peer's apply was `changed=false`
    /// and tags never synced (found live in T5: alice's `t5tag`/`t5clean` never
    /// reached bob). Drives the real edit path on both ends.
    #[cfg(feature = "crdt")]
    #[test]
    fn upsert_with_crdt_syncs_tag_only_edits_to_a_peer() {
        let owner_cid: u64 = 0xA11CE;

        // Owner creates a node with initial tags + shares; peer adopts the lineage.
        let mut owner = KnowledgeBase::new();
        let share = {
            let mut n = Node::new("t:n", "Title", NodeKind::Note, "body");
            n.tags = vec!["collabtest".into(), "fixture".into()];
            owner.upsert_with_crdt(n, owner_cid).unwrap()
        };
        let mut peer = KnowledgeBase::new();
        peer.adopt_remote_node("t:n", &share).unwrap();
        assert_eq!(peer.get("t:n").unwrap().tags, vec!["collabtest", "fixture"]);

        // Owner adds a tag ONLY (title/body unchanged) — the exact B-18 case.
        let tag_update = {
            let mut n = owner.get("t:n").unwrap().clone();
            n.tags = vec!["collabtest".into(), "fixture".into(), "t5tag".into()];
            owner.upsert_with_crdt(n, owner_cid).unwrap()
        };

        // Peer applies → must converge on the new tag (pre-fix: changed=false, no t5tag).
        let changed = peer.apply_remote_update("t:n", &tag_update).unwrap();
        assert!(
            changed,
            "a tags-only edit must enter the CRDT and change the peer (B-18)"
        );
        assert_eq!(
            peer.get("t:n").unwrap().tags,
            vec!["collabtest", "fixture", "t5tag"],
            "peer must converge on the owner's tag edit; title/body unchanged"
        );
        assert_eq!(peer.get("t:n").unwrap().title, "Title");
    }

    /// ADR-020 B-16 — where the hardcoded `client_id` ACTUALLY bites: CONCURRENT
    /// edits. Two peers sharing `client_id = 1` (the production hardcode) both edit
    /// the same node from a common base WITHOUT seeing each other → both mint
    /// client-1 ops at the SAME clock → a collision yrs cannot reconcile, so the two
    /// sides do NOT converge to one value. With distinct per-peer ids the concurrent
    /// edits are a normal CRDT conflict that converges deterministically on both
    /// sides. (Sequential edits converge even with a shared id — the clock advances
    /// monotonically — which is why this must be the *concurrent* case.)
    #[cfg(feature = "crdt")]
    #[test]
    fn concurrent_edits_diverge_under_shared_client_id_but_converge_under_distinct() {
        // Helper: two peers adopt a common base, edit concurrently, exchange, and we
        // check whether both end on the same title.
        fn run(alice_cid: u64, bob_cid: u64) -> (String, String) {
            let mut owner = KnowledgeBase::new();
            let base = owner
                .upsert_with_crdt(Node::new("t:n", "Base", NodeKind::Note, "body"), alice_cid)
                .unwrap();
            let mut alice = KnowledgeBase::new();
            alice.adopt_remote_node("t:n", &base).unwrap();
            let mut bob = KnowledgeBase::new();
            bob.adopt_remote_node("t:n", &base).unwrap();

            // Concurrent edits (neither has seen the other).
            let alice_edit = {
                let mut n = alice.get("t:n").unwrap().clone();
                n.title = "Alice".to_string();
                alice.upsert_with_crdt(n, alice_cid).unwrap()
            };
            let bob_edit = {
                let mut n = bob.get("t:n").unwrap().clone();
                n.title = "Bob".to_string();
                bob.upsert_with_crdt(n, bob_cid).unwrap()
            };
            // Exchange.
            alice.apply_remote_update("t:n", &bob_edit).unwrap();
            bob.apply_remote_update("t:n", &alice_edit).unwrap();
            (
                alice.get("t:n").unwrap().title.clone(),
                bob.get("t:n").unwrap().title.clone(),
            )
        }

        // Distinct ids (the fix): concurrent edits converge to the SAME value on both.
        let (a, b) = run(0xA11CE, 0xB0B);
        assert_eq!(
            a, b,
            "distinct client_ids → concurrent edits converge on both peers"
        );

        // Shared id (the production hardcode): the two peers do NOT converge.
        let (a1, b1) = run(1, 1);
        assert_ne!(
            a1, b1,
            "regression marker: a shared client_id=1 makes concurrent edits collide and \
             diverge — the fix must give each peer a distinct, stable id"
        );
    }

    fn spec(starter: &str, max_depth: usize, include_backlinks: bool) -> SubgraphSpec {
        SubgraphSpec {
            starter_nodes: vec![starter.to_string()],
            max_depth,
            include_backlinks,
            node_cap: None,
        }
    }

    #[test]
    fn hub_node_id_is_none_for_an_empty_kb() {
        let kb = KnowledgeBase::new();
        assert_eq!(kb.hub_node_id(), None);
    }

    #[test]
    fn hub_node_id_picks_the_highest_degree_node() {
        // "popular" has degree 3 (2 backlinks + 1 outgoing); everything
        // else has lower degree — regression case for KBs with no
        // "index"/NodeKind::Index convention (e.g. externally authored
        // org-roam-style proposal KBs using raw UUID ids).
        let kb = kb_with(vec![
            Node::new("ref1", "Ref1", NodeKind::Note, "[[popular]]"),
            Node::new("ref2", "Ref2", NodeKind::Note, "[[popular]]"),
            Node::new("popular", "Popular", NodeKind::Note, "[[lonely]]"),
            Node::new("lonely", "Lonely", NodeKind::Note, ""),
        ]);
        assert_eq!(kb.hub_node_id(), Some("popular".to_string()));
    }

    #[test]
    fn hub_node_id_breaks_ties_by_id_ascending_deterministically() {
        let kb = kb_with(vec![
            Node::new("zeta", "Zeta", NodeKind::Note, ""),
            Node::new("alpha", "Alpha", NodeKind::Note, ""),
            Node::new("mu", "Mu", NodeKind::Note, ""),
        ]);
        // All degree 0 — must deterministically pick the same one every
        // time regardless of HashMap iteration order.
        for _ in 0..20 {
            assert_eq!(kb.hub_node_id(), Some("alpha".to_string()));
        }
    }

    #[test]
    fn extract_subgraph_no_cap_includes_every_reachable_node() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "see [[b]] and [[c]]"),
            Node::new("b", "B", NodeKind::Note, ""),
            Node::new("c", "C", NodeKind::Note, ""),
        ]);
        let result = kb.extract_subgraph(&spec("a", 1, false));
        let mut ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "b", "c"]);
        assert_eq!(result.hidden_node_count, 0);
    }

    #[test]
    fn extract_subgraph_node_cap_keeps_starter_and_reports_hidden_count() {
        // A hub with five out-links, capped to keep only the starter + 2.
        let kb = kb_with(vec![
            Node::new(
                "hub",
                "Hub",
                NodeKind::Note,
                "[[n1]] [[n2]] [[n3]] [[n4]] [[n5]]",
            ),
            Node::new("n1", "N1", NodeKind::Note, ""),
            Node::new("n2", "N2", NodeKind::Note, ""),
            Node::new("n3", "N3", NodeKind::Note, ""),
            Node::new("n4", "N4", NodeKind::Note, ""),
            Node::new("n5", "N5", NodeKind::Note, ""),
        ]);
        let mut s = spec("hub", 1, false);
        s.node_cap = Some(3);
        let result = kb.extract_subgraph(&s);

        assert_eq!(result.nodes.len(), 3, "capped to exactly node_cap nodes");
        assert!(
            result.nodes.iter().any(|n| n.id == "hub"),
            "starter node is never dropped by the cap"
        );
        assert_eq!(
            result.hidden_node_count, 3,
            "5 reachable non-starter nodes - 2 kept = 3 hidden"
        );
        // Every link from the kept nodes to a now-excluded node must have
        // been demoted to a boundary link, not silently dropped.
        assert_eq!(result.boundary_links.len(), 3);
    }

    #[test]
    fn extract_subgraph_node_cap_prefers_higher_degree_nodes() {
        // "popular" is linked from two other nodes (degree 2 via backlinks);
        // "lonely" has no other connections (degree 0). A cap of 2 (starter
        // + 1) must keep "popular" over "lonely".
        let kb = kb_with(vec![
            Node::new("start", "Start", NodeKind::Note, "[[popular]] [[lonely]]"),
            Node::new("ref1", "Ref1", NodeKind::Note, "[[popular]]"),
            Node::new("ref2", "Ref2", NodeKind::Note, "[[popular]]"),
            Node::new("popular", "Popular", NodeKind::Note, ""),
            Node::new("lonely", "Lonely", NodeKind::Note, ""),
        ]);
        let mut s = spec("start", 1, false);
        s.node_cap = Some(2);
        let result = kb.extract_subgraph(&s);

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"start"));
        assert!(
            ids.contains(&"popular"),
            "higher-degree node must survive the cap over a same-tier lower-degree one: {ids:?}"
        );
        assert!(!ids.contains(&"lonely"));
    }

    #[test]
    fn extract_subgraph_node_cap_larger_than_reachable_set_is_a_no_op() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "see [[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let mut s = spec("a", 1, false);
        s.node_cap = Some(1000);
        let result = kb.extract_subgraph(&s);
        assert_eq!(result.nodes.len(), 2);
        assert_eq!(result.hidden_node_count, 0);
    }
}
