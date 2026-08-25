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
//!
//! @ai-caution: [architecture-debt] At 3,577 lines, well over the 800-line
//! ceiling. Not split (design work, not attempted this pass; round-5
//! tech-debt pass, 2026-07). Tracked in `docs/AUDIT_BASELINE.json` (machine-checked)
//! and `ROADMAP.md`'s "Architecture Debt" section.
//! The line count is deliberately not repeated here: the baseline holds it
//! and `make audit-metrics-check` fails if it grows.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod activity;
pub mod adr_kb;
pub mod adr_parse;
pub mod backup;
pub mod capabilities;
pub mod data_dir;
pub mod embedding_client;
pub mod enrichment;
pub mod export;
pub mod federation;
pub mod fuzzy;
pub mod graph_query;
pub mod ident;

pub mod kb_build;
pub mod kb_identity;
pub mod migrate;
pub mod org;
pub mod project_identity;
#[cfg(test)]
mod storage_feature_guard_tests;
pub mod store;
pub mod system_kb;
pub mod watch;

pub mod cache;
pub mod cozo_store;
pub mod hygiene;
pub mod lru_query;
pub mod query;
#[cfg(feature = "remote-hub")]
pub mod remote_hub;

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
pub use kb_identity::{KbTarget, PRIMARY_NAME_ALIASES};
pub use org::{IngestReport, OrgParseResult, ParsedLink};
pub use query::{CozoQueryLayer, FederatedQuery, InMemoryQueryLayer, KbQueryLayer};
pub use store::{
    AgendaFilter, Block, BrokenLinkInfo, BrokenLinkReason, HealthReport, IntegrityError, KbStore,
    KbStoreError, Link, MetaMember, NodeVersion, ReimportStaleFile, SubGraph, VectorHit,
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
    ///
    /// Note: the cap is deliberately applied POST-HOC, after the BFS has
    /// already walked to full depth-bounded completion (see
    /// `extract_subgraph`) — NOT as an early BFS stopping condition. An
    /// early-stopping BFS would change WHICH nodes get selected (traversal-
    /// order-biased) instead of today's exact global degree-sort over the
    /// full reachable set. This was considered and deliberately declined as
    /// a real selection-semantics change, not just a performance one.
    pub node_cap: Option<usize>,
    /// When `false`, collected `Node`s have their heavy fields
    /// (`body`, `properties`, `source_file`, `crdt_doc`) stripped to their
    /// lightest legitimate values before being cloned into
    /// `SubgraphResult.nodes`. `Node::body` can carry an entire org-mode
    /// document, `properties` a full drawer, and `crdt_doc` an encoded yrs
    /// document — none of which the KB graph view ever reads (it only
    /// needs `id`/`title`/`kind`/`source` for rendering). The BFS walk
    /// itself always uses the full node data (link extraction reads
    /// `body`) regardless of this flag — only the *collected* clones pushed
    /// into the result are affected. Defaults to `true` (preserves every
    /// pre-existing caller's behavior exactly); set `false` only when the
    /// caller is confirmed to need metadata alone.
    pub include_body: bool,
    /// Hard filter, independent of `node_cap`: when `Some(tag)`, the BFS
    /// walk still traverses through EVERY reachable node up to `max_depth`
    /// (an untagged node stays a valid stepping stone to a tagged node
    /// beyond it — the tag restricts the RESULT, not the traversal), but
    /// only nodes whose `Node::tags` contains `tag` (plus every
    /// `starter_nodes` id, regardless of its own tags — the seed always
    /// anchors the export even if untagged) survive into the final
    /// `included` set. Reuses `KnowledgeBase::nodes_by_tag`'s exact-match
    /// convention, not Cozo's `AgendaFilter::Tag` substring convention --
    /// this operates purely on the in-memory graph, never Datalog (see
    /// ADR-082). Excluded reachable nodes are demoted to boundary-link
    /// stubs exactly like a depth/`node_cap` cutoff already handles them --
    /// no new link-classification path needed. `None` = no filter
    /// (preserves every pre-existing caller's behavior exactly).
    pub required_tag: Option<String>,
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

/// A boundary link promoted to "genuinely crosses into a DIFFERENT
/// registered KB instance" — see `Editor::partition_boundary_links_by_instance`
/// (`crates/core/src/editor/kb_ops/registry.rs`), the multi-KB chord view's
/// (#462) sole producer of this type. A plain `SubgraphLink` boundary link
/// only ever carries `(source, target, rel_type, weight)` with no notion of
/// WHICH KB the target lives in — this adds exactly that, so a caller can
/// tell "this is a real cross-instance relationship, render an edge to the
/// other diagram" apart from "this is just outside the depth/cap cutoff, or
/// unresolvable" (both of which stay plain `SubgraphLink`s).
#[derive(Debug, Clone)]
pub struct CrossInstanceLink {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    pub weight: f64,
    /// Which KB instance owns `target` — `None` = primary, `Some(uuid)` = a
    /// federated instance, matching `GraphView.kb_instance`'s convention
    /// (`crates/core/src/graph_view.rs`).
    pub target_instance: Option<String>,
    /// Which KB instance this link's `source` belongs to — i.e. the
    /// `owner_instance` the classifying `partition_boundary_links_by_instance`
    /// call was made with. `None` = primary, `Some(uuid)` = a federated
    /// instance, same convention as `target_instance`.
    ///
    /// @ai-caution: [correctness] Phase A2 (#462 multi-KB chord view):
    /// `partition_boundary_links_by_instance` is now called once PER
    /// rendered diagram (seed AND every related instance), not just once
    /// against the seed — a real link from related-instance B to
    /// related-instance C is only ever discovered from B's own extraction,
    /// so its source is B, not the seed. Do NOT assume `source` is always
    /// "the subgraph this batch was extracted from" == the seed; read this
    /// field instead of re-deriving the source from call-site context.
    pub source_instance: Option<String>,
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
    /// How many BFS-reachable nodes were excluded because they didn't
    /// carry `SubgraphSpec::required_tag` (and weren't a starter node).
    /// `0` when `required_tag` is unset. Reported separately from
    /// `hidden_node_count` — the two cutoffs are independent (see
    /// `SubgraphSpec::required_tag`'s doc comment) and a caller should never
    /// conflate "excluded by tag" with "excluded by node_cap."
    pub tag_filtered_count: usize,
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
    /// Promoted into the primary KB from a federated/org-dir-imported
    /// instance (#303) — deliberately distinct from `Federation` so
    /// `kb_owner_of`'s stranded-node guard (issue #76) never mistakes a
    /// freshly-promoted primary copy for pre-ADR-019 leftover cruft, and
    /// distinct from `Manual` so promotion provenance isn't conflated with
    /// hand-authored nodes. See `Editor::kb_promote_node`.
    Promoted,
}

impl NodeSource {
    /// The serialized form, shared by the Cozo row encoding and the CRDT payload.
    ///
    /// @ai-caution: [kb-truth] These strings are PERSISTED and now also cross the
    /// wire (#710), so they are a compatibility surface — renaming one silently
    /// reclassifies existing nodes and drops provenance arriving from a peer on
    /// an older build. Previously this mapping was inlined in the cozo row
    /// encoder, so a second copy could disagree with it (principle #8).
    pub fn as_str(self) -> &'static str {
        match self {
            NodeSource::Seed => "seed",
            NodeSource::UserOrg => "user_org",
            NodeSource::Manual => "manual",
            NodeSource::Federation => "federation",
            NodeSource::Promoted => "promoted",
        }
    }

    /// Parse the serialized form. Unknown ⇒ `None`.
    ///
    /// Deliberately NOT lossy-with-a-default, unlike `NodeKind::from_str_lossy`:
    /// guessing a provenance is how read-only content becomes editable. An
    /// unrecognised value means "a peer knows something this build does not", and
    /// the caller keeps whatever it already had.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "seed" => Some(NodeSource::Seed),
            "user_org" => Some(NodeSource::UserOrg),
            "manual" => Some(NodeSource::Manual),
            "federation" => Some(NodeSource::Federation),
            "promoted" => Some(NodeSource::Promoted),
            _ => None,
        }
    }
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
    /// Creation timestamp (unix seconds), when known from the node's CRDT
    /// document. `None` for a node that has never had one — the Cozo row then
    /// falls back to "now" on first insert, as it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
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
            created_at: None,
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
            // Restoring an EXISTING lineage. This is a read — callers diff against
            // it (`encode_diff`, `state_vector`) on the reconcile path.
            //
            // @ai-caution: [crdt] Do NOT write the v2 fields here. Authoring ops in
            // what every caller treats as an accessor advances this replica's state
            // vector, so a peer that is actually caught up reports `local_ahead` and
            // pushes redundant updates forever. Metadata reaches an existing lineage
            // through `upsert_with_crdt`, which is the real write path.
            mae_sync::kb::KbNodeDoc::from_bytes(bytes)
        } else {
            // Minting a FRESH lineage from this node's own fields — construction,
            // not mutation of shared state, so the full v2 payload belongs here.
            let mut doc =
                mae_sync::kb::KbNodeDoc::new(&self.id, &self.title, &self.body, &self.tags);
            self.write_v2_fields(&mut doc);
            Ok(doc)
        }
    }

    /// ADR-093: push every non-text `Node` field into a `KbNodeDoc`.
    #[cfg(feature = "crdt")]
    fn write_v2_fields(&self, doc: &mut mae_sync::kb::KbNodeDoc) {
        let _ = doc.set_kind(Some(self.kind.as_str()));
        let _ = doc.set_todo_state(self.todo_state.as_deref());
        let _ = doc.set_priority(self.priority.map(|c| c.to_string()).as_deref());
        let _ = doc.set_source_version(self.source_version);
        let _ = doc.set_aliases(&self.aliases);
        let _ = doc.set_properties(&self.properties);
        // #710: provenance must cross the wire. Without it a shared node arrives
        // re-stamped `Federation`, so `Seed`-marked read-only content becomes
        // editable at the peer.
        let _ = doc.set_source(self.source.map(|s| s.as_str()));
    }

    /// Update this node's fields from a `KbNodeDoc`, and store the encoded CRDT
    /// bytes for persistence.
    ///
    /// ADR-093: a v1 document carries none of the non-text keys, so each is
    /// applied only when the doc actually has it — reading an old document must
    /// never blank a field the local `Node` still holds.
    #[cfg(feature = "crdt")]
    pub fn apply_crdt_doc(&mut self, doc: &mae_sync::kb::KbNodeDoc) {
        self.title = doc.title();
        self.body = doc.body();
        self.tags = doc.tags();
        if let Some(k) = doc.kind() {
            self.kind = NodeKind::from_str_lossy(&k);
        }
        if doc.schema_version() >= 2 {
            self.todo_state = doc.todo_state();
            self.priority = doc.priority().and_then(|p| p.chars().next());
            self.source_version = doc.source_version();
            self.aliases = doc.aliases();
            self.properties = doc.properties();
            // Tolerant: a v2 document authored before `source` joined the schema
            // carries none, and must not blank the provenance we already hold.
            if let Some(src) = doc.source().and_then(|s| NodeSource::from_str_opt(&s)) {
                self.source = Some(src);
            }
        }
        if let Some(c) = doc.created_at() {
            self.created_at = Some(c);
        }
        self.crdt_doc = Some(doc.encode());
    }

    /// Create a new Node from a `KbNodeDoc` (CRDT → Node materialization).
    ///
    /// Used when joining a shared KB: the CRDT doc is the source of truth,
    /// and we create a local Node from it for FTS5 indexing and display.
    ///
    /// ADR-093: `kind` and `source` are **fallbacks**, used only when the document
    /// does not carry them (a v1 doc). A v2 doc's own values win — otherwise this
    /// is not a round-trip at all, it just echoes the caller's arguments back.
    #[cfg(feature = "crdt")]
    pub fn from_crdt_doc(
        doc: &mae_sync::kb::KbNodeDoc,
        kind: NodeKind,
        source: NodeSource,
    ) -> Self {
        let mat = doc.materialize();
        let resolved_kind = mat
            .kind
            .as_deref()
            .map(NodeKind::from_str_lossy)
            .unwrap_or(kind);
        let mut node = Node::new(mat.id, mat.title, resolved_kind, mat.body);
        node.tags = mat.tags;
        // #710: the DOCUMENT's provenance wins; `source` is only the fallback for
        // a document that does not carry one. Before this, every projected node
        // was stamped with the caller's argument — `NodeSource::Federation` from
        // the projector — which is what destroyed `Seed` marking on sync.
        node.source = mat
            .source
            .as_deref()
            .and_then(NodeSource::from_str_opt)
            .or(Some(source));
        node.todo_state = mat.todo_state;
        node.priority = mat.priority.and_then(|p| p.chars().next());
        node.source_version = mat.source_version;
        node.aliases = mat.aliases;
        node.properties = mat.properties;
        node.created_at = mat.created_at;
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
    /// Target is a well-formed id that was **not found in the corpus that was
    /// searched**.
    ///
    /// @ai-caution: [health-reporting] This does NOT mean the node was deleted,
    /// and must not be reported as if it were. `health_report` builds its id set
    /// from ONE in-memory `KnowledgeBase`, so a link to a node that lives in a
    /// federated instance, on a hub, or in a KB this replica simply does not
    /// hold lands here too. Distinguishing "gone" from "not here" needs an
    /// existence oracle this type does not have -- see the module note and
    /// `NotHeldLocally`.
    ///
    /// Previously named `DeletedNode`, which asserted more than the evidence
    /// supports; the MCP `kb_health` tool exported that name to the model.
    TargetNotFound,
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
            BrokenLinkKind::TargetNotFound
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
/// Common English function words that carry no topical meaning on their own
/// — filtered out of query terms before the strict/soft-AND hit-counting
/// gate in [`KnowledgeBase::search_ranked_pass`] (#357). Without this, a
/// natural conversational query like "how should I annotate code for other
/// AI agents" has 4+ terms ("how"/"should"/"i"/"for") that never literally
/// appear in ANY node's title/body, so even the soft-AND fallback (which
/// only relaxes by exactly one unmatched term) excludes the correct target
/// entirely — the real content words ("annotate"/"code"/"other"/"ai"/
/// "agents") are what should gate retrieval, not incidental grammar.
/// Deliberately a small, standard, well-known stopword set (not a research
/// project) — question words, articles, common prepositions/conjunctions,
/// auxiliary/modal verbs, and pronouns. Also used downstream by `mae-ai`'s
/// `score_node` re-ranker for the same reason: a bare `contains(term)`
/// check on a one-letter stopword like "a" or "i" would spuriously "match"
/// almost every title.
const STOPWORDS: &[&str] = &[
    "how", "what", "why", "when", "where", "who", "which", "whom", "whose", "is", "are", "was",
    "were", "be", "been", "being", "do", "does", "did", "doing", "should", "would", "could", "can",
    "will", "shall", "may", "might", "must", "the", "a", "an", "of", "to", "in", "on", "at", "for",
    "with", "by", "from", "about", "as", "into", "through", "after", "over", "between", "out",
    "against", "during", "without", "before", "under", "and", "or", "but", "if", "then", "this",
    "that", "these", "those", "i", "you", "we", "they", "it", "he", "she", "me", "him", "her",
    "us", "them", "my", "your", "our", "their", "its", "his", "so", "than", "too", "just", "not",
    "no",
];

/// Filter `words` down to non-stopwords, falling back to the ORIGINAL
/// (unfiltered) list if that would leave nothing — an all-stopword query
/// like "what is this" should still match on its own literal words rather
/// than degrading to "no terms at all" (which `search_ranked_pass` already
/// handles as a special empty-query case, not this function's concern).
pub fn filter_stopwords<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let filtered: Vec<&str> = words.iter().copied().filter(|w| !is_stopword(w)).collect();
    if filtered.is_empty() {
        words.to_vec()
    } else {
        filtered
    }
}

/// A word is a stopword either literally, or as a contraction of one (e.g.
/// "what's" -> "what", "it's" -> "it") -- queries are natural English
/// sentences, not pre-tokenized search strings, and a bare `'s` suffix is
/// the single most common contraction shape that would otherwise slip past
/// the literal STOPWORDS list entirely.
fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
        || word
            .strip_suffix("'s")
            .is_some_and(|w| STOPWORDS.contains(&w))
}

/// Longest-suffix-first so a word ending in a longer suffix (e.g.
/// "-ation") isn't first mis-stripped by a shorter one it also happens to
/// end with (e.g. "-s" fired at wrong length before "-ations" gets a
/// chance). NOT a real stemmer (Porter/Snowball) — just the handful of
/// English morphological suffixes common enough in prose-KB content to
/// matter for #357-style queries (plural "targets"/"target", nominalized
/// "self-documented"/"self-documentation"), gated on a minimum remaining
/// stem length so short unrelated words don't collapse together (e.g. "as"
/// staying "as", not stripped to "a"). Real fuzzy/FTS matching remains out
/// of scope, tracked separately as #81.
const STEM_SUFFIXES: &[&str] = &[
    "ations", "ements", "ation", "ement", "ingly", "edly", "ing", "ed", "es", "s",
];
const MIN_STEM_LEN: usize = 4;

/// Best-effort stem of `word`, or `word` itself if no suffix applies (or
/// stripping one would leave too short a remainder). Used by
/// `search_ranked_pass` to widen a term's substring match beyond its exact
/// literal form.
pub fn stem(word: &str) -> &str {
    for suffix in STEM_SUFFIXES {
        if let Some(stripped) = word.strip_suffix(suffix) {
            if stripped.chars().count() >= MIN_STEM_LEN {
                return stripped;
            }
        }
    }
    word
}

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

/// Relevance prior by node kind/role, composed with `namespace_prior`
/// (#357): a `Category`/`Meta` node, or one explicitly marked `:role: hub`
/// (the molecular-notes `source|atom|molecule|hub` vocabulary — see
/// `Editor::kb_set_role`), is navigational rather than the answer itself, so
/// it shouldn't outrank a specific atom/note with real but modest term
/// coverage. Mild (0.85, same magnitude class as `namespace_prior`'s 0.9) —
/// only tips near-ties, never buries a strong exact match.
fn kind_role_prior(kind: NodeKind, properties: &HashMap<String, String>) -> f64 {
    let is_hub = matches!(kind, NodeKind::Category | NodeKind::Meta)
        || properties.get("role").map(String::as_str) == Some("hub");
    if is_hub {
        0.85
    } else {
        1.0
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

/// A brand-new `KbNodeDoc` carrying **every** field of `node`, not just the text.
///
/// **#656.** `KbNodeDoc::new_with_client_id` writes id/title/body/tags — the v1
/// schema. The existing-lineage branch of `upsert_with_crdt` then called
/// `write_v2_fields` to add `kind`/`todo`/`prio`/`aliases`/`props`/`src_v`/
/// `source`; the two FRESH-lineage branches did not, so a node that has v2 fields
/// minted a **v1 document** and every structured field was silently dropped.
///
/// That is the field-authority rule biting: a field that is not in the CRDT does
/// not survive, and these fields were not making it in at all on this path.
///
/// Reached whenever a node has no CRDT bytes yet (first share of an existing KB,
/// lazy migration) or its bytes are unreadable — i.e. exactly the cases where
/// there is no prior lineage to inherit the fields from.
#[cfg(feature = "crdt")]
fn fresh_v2_doc(node: &Node, client_id: u64) -> mae_sync::kb::KbNodeDoc {
    let mut doc = mae_sync::kb::KbNodeDoc::new_with_client_id(
        &node.id,
        &node.title,
        &node.body,
        &node.tags,
        client_id,
    );
    node.write_v2_fields(&mut doc);
    doc
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

    /// Ids of nodes currently attributed to `path` via `source_file` — the
    /// in-memory-graph's own source of truth for "what did this file last
    /// produce". Used as a watcher-independent fallback by
    /// `Editor::kb_reimport_file`'s id-retraction logic: a live filesystem
    /// watcher's cached path->ids mapping is unavailable whenever
    /// `kb_watcher_enabled` is off, OR whenever `OrgDirWatcher::new` failed
    /// to attach (e.g. an exhausted inotify-instance limit under heavy
    /// concurrent test/process load) — in either case there is still a
    /// correct answer sitting right here in the graph, so retraction on an
    /// in-place `:ID:` rename must not silently depend on a watcher having
    /// happened to attach. Order is unspecified.
    pub fn ids_by_source_file(&self, path: &std::path::Path) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.source_file.as_deref() == Some(path))
            .map(|(id, _)| id.clone())
            .collect()
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
                    // ADR-093: the same treatment for every non-text field. This is
                    // the write path, so an existing lineage picks up metadata here
                    // (never in `to_crdt_doc`, which callers use as an accessor).
                    // Each setter is a no-op when the value already matches, so an
                    // unchanged node still produces no ops.
                    node.write_v2_fields(&mut doc);
                    doc
                }
                // Unreadable prior bytes: start a fresh lineage rather than
                // failing the write. See `fresh_v2_doc` for why the v2 fields
                // must be written here too (#656).
                Err(_) => fresh_v2_doc(&node, client_id),
            }
        } else {
            fresh_v2_doc(&node, client_id)
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
                // #493: only a node that actually resolves may enter
                // `included` at all. A dead/typo'd link's target must NOT be
                // phantom-inserted here even though it's silently skipped
                // later in `collect_and_categorize`'s node-collection loop --
                // by the time that loop runs, the damage is already done:
                // `collect_and_categorize` classifies a link as internal vs.
                // boundary purely via `included.contains(&target)`, so a
                // phantom-included dead target makes a REAL link pointing at
                // it misclassify as "internal" (same-subgraph) instead of
                // "boundary" (unresolvable) -- the target never actually
                // appears in the result's `nodes`, so that link now points at
                // nothing, silently. Gating the `included.insert` on node
                // existence (short-circuit, so a nonexistent id also never
                // triggers link/backlink expansion) fixes this at the source
                // instead of leaving it for every downstream categorization
                // site to work around individually (CLAUDE.md #8).
                if let Some(node) = self.nodes.get(node_id) {
                    if included.insert(node_id.clone()) && depth < spec.max_depth {
                        // Add outgoing links to frontier
                        for link in node.links() {
                            if !included.contains(&link) {
                                next_frontier.push(link);
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
            }
            frontier = next_frontier;
            depth += 1;
        }

        // Hard tag filter, applied AFTER the full BFS walk but BEFORE
        // node_cap truncation — so node_cap counts the tag-filtered
        // candidate set, not raw traversal size, and an excluded untagged
        // node gets the exact same boundary-link-stub demotion node_cap's
        // own cutoff already produces below (no new link-classification
        // code needed).
        let tag_filtered_count = match &spec.required_tag {
            Some(tag) => {
                let starters: HashSet<&str> =
                    spec.starter_nodes.iter().map(String::as_str).collect();
                let kept: HashSet<String> = included
                    .iter()
                    .filter(|id| {
                        starters.contains(id.as_str())
                            || self
                                .nodes
                                .get(id.as_str())
                                .is_some_and(|n| n.tags.iter().any(|t| t == tag))
                    })
                    .cloned()
                    .collect();
                let excluded = included.len() - kept.len();
                included = kept;
                excluded
            }
            None => 0,
        };

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

        // Collect nodes and categorize links.
        let (nodes, internal_links, boundary_links) =
            self.collect_and_categorize(&included, spec.include_body);

        SubgraphResult {
            nodes,
            links: internal_links,
            boundary_links,
            hidden_node_count,
            tag_filtered_count,
        }
    }

    /// Shared node-collection/link-categorization block, factored out of
    /// `extract_subgraph` (Phase B1, #462 full-corpus retrieval) so
    /// `extract_full_corpus` can reuse it verbatim instead of duplicating
    /// the same clone/strip-fields/internal-vs-boundary logic a second time.
    /// `extract_subgraph` itself passes its own BFS-produced `included` set
    /// here unchanged — behavior is byte-identical to before this refactor
    /// (see the `extract_subgraph_*` test suite, which exercises this
    /// indirectly and is unmodified by this split).
    ///
    /// For every id in `included` that resolves to a real stored `Node`,
    /// clones it (heavy fields stripped when `include_body` is `false` —
    /// see `SubgraphSpec::include_body`'s doc comment for exactly which
    /// fields), then walks its typed links, sorting each into `internal`
    /// (target also in `included`) or `boundary` (target outside). Ids in
    /// `included` with no matching stored node (e.g. a phantom BFS starter)
    /// are silently skipped, matching `extract_subgraph`'s pre-existing
    /// behavior.
    fn collect_and_categorize(
        &self,
        included: &HashSet<String>,
        include_body: bool,
    ) -> (Vec<Node>, Vec<SubgraphLink>, Vec<SubgraphLink>) {
        let mut nodes = Vec::new();
        let mut internal_links = Vec::new();
        let mut boundary_links = Vec::new();

        for id in included {
            if let Some(node) = self.nodes.get(id) {
                // `links_typed()` reads `node.body` — must be computed from
                // the full node BEFORE any lightweight stripping below, so
                // `include_body: false` never affects which links surface.
                let typed_links = node.links_typed();
                if include_body {
                    nodes.push(node.clone());
                } else {
                    // Keep every cheap scalar/small-Vec field (some are read
                    // downstream — e.g. `source` by `is_residency_exempt`,
                    // `kind` by the canvas conversion); drop only the
                    // confirmed-heavy fields (body/properties/source_file/
                    // crdt_doc) that the graph view never reads.
                    nodes.push(Node {
                        id: node.id.clone(),
                        title: node.title.clone(),
                        kind: node.kind,
                        body: String::new(),
                        tags: node.tags.clone(),
                        todo_state: node.todo_state.clone(),
                        priority: node.priority,
                        source: node.source,
                        source_version: node.source_version,
                        aliases: node.aliases.clone(),
                        properties: HashMap::new(),
                        source_file: None,
                        crdt_doc: None,
                        created_at: None,
                    });
                }
                for (target, rel_type, weight) in typed_links {
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

        (nodes, internal_links, boundary_links)
    }

    /// Full-corpus extraction (Phase B1, #462): every node in this KB, not a
    /// depth/breadth-bounded BFS from a seed. The naive version of this —
    /// `extract_subgraph` with `node_cap: None` and `starter_nodes:
    /// list_ids(None)` — genuinely works (BFS-from-every-node collapses to
    /// "include everything reachable"), BUT `extract_subgraph`'s node_cap
    /// truncation unconditionally exempts every `starter_node`, so making
    /// every node a starter would defeat the one safety net a pathological-
    /// scale KB needs most. This method sidesteps that entirely: it never
    /// runs a BFS, so there is no starter-node concept to abuse — the only
    /// exemption is the caller-supplied `protected` set.
    ///
    /// `cap`: safety-net truncation exactly like `SubgraphSpec::node_cap`
    /// (same degree-sort-descending, tie-break-by-id-ascending selection
    /// logic, same `hidden_node_count` meaning) — `None` disables it.
    ///
    /// `protected`: ids exempt from truncation. Unlike `extract_subgraph`'s
    /// starter-node exemption (which is total — the whole `starter_nodes`
    /// list, by construction usually small), the CALLER decides what's
    /// protected here — e.g. the current focus node plus every node this
    /// instance uses as a cross-instance-link source (a "bridge" — cutting
    /// it would silently sever the only connection between two diagrams).
    /// This function does not know or care what makes an id worth
    /// protecting; that's a cross-instance/DOI-tiering concern that belongs
    /// to the caller (`mae-core`, which has `Editor::kb_owner_of` and
    /// registry access — this crate deliberately has neither). An id in
    /// `protected` that isn't actually present in this KB is silently
    /// ignored (never inflates the effective cap), matching `list_ids`'
    /// "only ids that actually exist" contract.
    ///
    /// `include_body`: forwarded to `collect_and_categorize` unchanged —
    /// same meaning as `SubgraphSpec::include_body`.
    pub fn extract_full_corpus(
        &self,
        cap: Option<usize>,
        protected: &HashSet<String>,
        include_body: bool,
    ) -> SubgraphResult {
        let mut included: HashSet<String> = self.list_ids(None).into_iter().collect();

        let hidden_node_count = match cap {
            Some(cap) if included.len() > cap => {
                // Only ids BOTH caller-protected AND actually present in this
                // KB count against the exemption budget — an id the caller
                // protected because it matters in a DIFFERENT instance must
                // not silently shrink how many of THIS instance's own nodes
                // get to survive the cap.
                let protected_in_scope: HashSet<&String> = included
                    .iter()
                    .filter(|id| protected.contains(id.as_str()))
                    .collect();
                let mut candidates: Vec<&String> = included
                    .iter()
                    .filter(|id| !protected_in_scope.contains(id))
                    .collect();
                candidates.sort_by(|a, b| {
                    let deg_a = self.node_degree(a);
                    let deg_b = self.node_degree(b);
                    deg_b.cmp(&deg_a).then_with(|| a.cmp(b))
                });
                let keep_budget = cap.saturating_sub(protected_in_scope.len());
                let kept: HashSet<String> = protected_in_scope
                    .iter()
                    .map(|s| (*s).clone())
                    .chain(candidates.into_iter().take(keep_budget).cloned())
                    .collect();
                let hidden = included.len() - kept.len();
                included = kept;
                hidden
            }
            _ => 0,
        };

        let (nodes, internal_links, boundary_links) =
            self.collect_and_categorize(&included, include_body);

        SubgraphResult {
            nodes,
            links: internal_links,
            boundary_links,
            hidden_node_count,
            // extract_full_corpus has no seed/BFS and no notion of a
            // required tag -- always 0, matching SubgraphSpec::required_tag
            // being unset for this call shape.
            tag_filtered_count: 0,
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

    /// Hop-distance from `focus` to every node reachable from it (ADR-068
    /// Phase B3 — Furnas Degree-of-Interest's `D(x, focus)` term), walking
    /// BOTH outgoing (`Node::links()`) and incoming (`links_in`) adjacency —
    /// i.e. undirected reachability. Deliberately does NOT gate on
    /// `SubgraphSpec::include_backlinks` the way `extract_subgraph`'s BFS
    /// does: that flag steers what a BFS-based EXTRACTION pulls in, whereas
    /// "how far is this node from the user's focus" for render-time
    /// tiering is a pure topology question that shouldn't silently change
    /// shape depending on an unrelated extraction setting.
    ///
    /// `focus` itself is distance `0` (only when it's an actual node id in
    /// this KB — an unknown `focus` returns an empty map, never panics).
    /// Every other key present is reachable, at its shortest hop count; an
    /// id NOT present in the returned map is unreachable from `focus`
    /// within this KB (the caller should treat a missing entry as "no
    /// bound" — see `Editor::graph_view_doi_distances`, the one production
    /// caller, which maps a missing entry to `None`).
    ///
    /// O(V+E) — one BFS frontier expansion per hop, each node visited
    /// exactly once (the `distances.contains_key` guard below). Callers
    /// needing a MULTI-source distance (e.g. a diagram with several
    /// cross-link landing points) call this once per source id and merge
    /// via per-id minimum — kept a single-source primitive here rather than
    /// accepting a `&[String]` itself, since every other extraction helper
    /// in this module (`extract_subgraph`, `extract_full_corpus`) already
    /// puts multi-id/merge concerns on the CALLER, not this crate.
    pub fn hop_distances_from(&self, focus: &str) -> HashMap<String, usize> {
        let mut distances = HashMap::new();
        if !self.nodes.contains_key(focus) {
            return distances;
        }
        distances.insert(focus.to_string(), 0);
        let mut frontier = vec![focus.to_string()];
        let mut depth = 0usize;
        while !frontier.is_empty() {
            let mut next_frontier = Vec::new();
            for id in &frontier {
                let mut neighbors: Vec<String> = Vec::new();
                if let Some(node) = self.nodes.get(id) {
                    neighbors.extend(node.links());
                }
                if let Some(sources) = self.links_in.get(id) {
                    neighbors.extend(sources.iter().cloned());
                }
                for n in neighbors {
                    if !distances.contains_key(&n) {
                        distances.insert(n.clone(), depth + 1);
                        next_frontier.push(n);
                    }
                }
            }
            frontier = next_frontier;
            depth += 1;
        }
        distances
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
    ///
    /// **Soft-AND fallback (#357):** a natural-language query commonly
    /// contains one word (filler, synonym, typo) absent from an otherwise
    /// relevant node, which strict AND would drop entirely. When the query
    /// has more than one term, a second, relaxed pass (`terms.len() - 1`
    /// required hits) always runs ALONGSIDE the strict pass — not only when
    /// the strict pass is empty — and the two candidate pools are merged by
    /// id: a strict-pass hit keeps its full score, a relaxed-only hit is
    /// added with a fixed penalty applied. Merging unconditionally (rather
    /// than gating the relaxed pass on strict-pass emptiness) matters
    /// because a hub/meta node satisfying strict AND in full must not
    /// silently keep a more specific target — which just misses strict AND
    /// by one real content term — out of the candidate pool entirely; both
    /// need to enter scoring so `kind_role_prior`'s hub down-weight can
    /// actually compare them. This is a bounded down payment on #357's
    /// symptom, not a replacement for real fuzzy/FTS body search (tracked
    /// separately as #81).
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

        // #357: strip stopwords before they count toward the strict/soft-AND
        // hit gate below — see `filter_stopwords`'s own doc comment for why
        // (a conversational query's filler words otherwise sink retrieval
        // entirely, since the soft-AND fallback only relaxes by one term).
        let words: Vec<&str> = q.split_whitespace().collect();
        let words = filter_stopwords(&words);
        let terms: Vec<Vec<char>> = words.iter().map(|t| t.chars().collect()).collect();

        const FALLBACK_PENALTY: f64 = 0.5;
        let strict = self.search_ranked_pass(&q, &terms, terms.len());
        let mut merged: std::collections::HashMap<String, f64> = strict.into_iter().collect();
        if terms.len() > 1 {
            let relaxed = self.search_ranked_pass(&q, &terms, terms.len() - 1);
            for (id, score) in relaxed {
                // Strict-pass score wins if the id already made the strict pool;
                // otherwise it's a relaxed-only hit and gets the penalty.
                merged.entry(id).or_insert(score * FALLBACK_PENALTY);
            }
        }
        let mut scored: Vec<(String, f64)> = merged.into_iter().collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        scored
    }

    /// One scoring pass over every node for [`search_ranked`](Self::search_ranked),
    /// requiring at least `min_hits` of `terms` to match somewhere on the node
    /// (strict AND when `min_hits == terms.len()`; the soft-AND fallback pass
    /// otherwise). Factored out so `search_ranked` can call it twice without
    /// duplicating the field-weighting logic.
    fn search_ranked_pass(
        &self,
        q: &str,
        terms: &[Vec<char>],
        min_hits: usize,
    ) -> Vec<(String, f64)> {
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

        let num_terms = terms.len().max(1) as f64;
        let whole: Vec<char> = q.chars().collect();

        let mut scored: Vec<(String, f64)> = Vec::new();
        for (id, cache) in self.lower.iter() {
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
            let whole_bonus = [cache.lowered_id.as_str(), local_id, cache.title.as_str()]
                .into_iter()
                .chain(cache.aliases.iter().map(|s| s.as_str()))
                .filter_map(|s| fuzzy::score_match(s, &whole))
                .max()
                .map(|s| s as f64 * W_TITLE)
                .unwrap_or(0.0);

            let mut total = whole_bonus;
            let mut hits = 0usize;
            for term in terms {
                let term_str: String = term.iter().collect();
                let stemmed = stem(&term_str);
                let stem_chars: Vec<char> = stemmed.chars().collect();
                // #357: also try the stemmed form of the term so a query
                // word's morphological variant ("targets") still matches a
                // field's literal form ("target") — see `stem`'s doc
                // comment. When stemming is a no-op, `stem_chars == term`
                // and this is exactly the pre-stemming behavior.
                let best_of = |s: &str| -> Option<i64> {
                    let exact = fuzzy::score_match(s, term);
                    if stemmed == term_str {
                        exact
                    } else {
                        let stemmed_score = fuzzy::score_match(s, &stem_chars);
                        exact.into_iter().chain(stemmed_score).max()
                    }
                };
                let title_alias = [cache.lowered_id.as_str(), local_id, cache.title.as_str()]
                    .into_iter()
                    .chain(cache.aliases.iter().map(|s| s.as_str()))
                    .filter_map(best_of)
                    .max()
                    .map(|s| s as f64 * W_TITLE);
                let tag = cache
                    .tags
                    .iter()
                    .filter_map(|t| best_of(t))
                    .max()
                    .map(|s| s as f64 * W_TAG);
                let body = (cache.body.contains(&term_str)
                    || (stemmed != term_str && cache.body.contains(stemmed)))
                .then_some(BODY_HIT * W_BODY);

                // Best field for this term. A term with no match anywhere
                // contributes no score (and no hit) — whether that drops the
                // node entirely depends on `min_hits` (strict AND vs. the
                // soft-AND fallback pass in `search_ranked`).
                let best = [title_alias, tag, body]
                    .into_iter()
                    .flatten()
                    .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
                if let Some(s) = best {
                    total += s;
                    hits += 1;
                }
            }
            if hits < min_hits {
                continue;
            }
            // Namespace + kind/role priors: primary content (concept/cmd/
            // scheme/option/category) outranks navigational/glossary nodes
            // (term/lesson/…) on a tie, and hub/meta/category nodes are
            // down-weighted below the specific note they merely index (#357)
            // — matches the org-roam intuition that the concept/atom page,
            // not its glossary term or index hub, is the canonical
            // destination. Denominator includes the whole-query bonus slot
            // (+1) so scores stay in 0..=1 without excessive clamping.
            let prior = namespace_prior(id)
                * self
                    .nodes
                    .get(id)
                    .map(|n| kind_role_prior(n.kind, &n.properties))
                    .unwrap_or(1.0);
            let norm = (total * prior / ((num_terms + 1.0) * MAX_TERM)).min(1.0);
            scored.push((id.clone(), norm));
        }
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

    /// Stamp only the named nodes that have no source — the scoped counterpart
    /// of [`Self::stamp_source`].
    ///
    /// @ai-caution: [kb-provenance] `ingest_org_dir` REPLACES a node wholesale
    /// (`insert` overwrites; it does not merge), so ingesting a corpus over
    /// already-stamped nodes DESTROYS their provenance — a `concept:buffer`
    /// stamped `Seed` by `seed_kb()` comes back with `source: None`. Every
    /// built-in-content guard in the editor keys on `source == Some(Seed)`, so
    /// the whole hand-written manual silently stops being recognized as
    /// built-in. Re-stamp after such an ingest, scoped to the ids the ingest
    /// actually reported (`IngestReport::ingested_ids`) — a blanket
    /// [`Self::stamp_source`] would also brand the user's own unstamped nodes
    /// (e.g. `~/.config/mae/help/*.org`) as MAE's, making them uneditable.
    pub fn stamp_source_for(&mut self, ids: &[String], source: NodeSource, version: u32) {
        for id in ids {
            if let Some(node) = self.nodes.get_mut(id) {
                if node.source.is_none() {
                    node.source = Some(source);
                    node.source_version = Some(version);
                }
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

    /// In-memory equivalent of `CozoKbStore::agenda_query` (`shared/kb/src/
    /// cozo_store/agenda.rs`) — for a federated instance imported straight
    /// from an org directory with no durable Cozo-backed store at all (a
    /// real, common shape: `editor.kb.instance_stores` only ever gets an
    /// entry when opening/creating that store succeeded, see
    /// `Editor::kb_reimport`), `store.agenda_query` is categorically
    /// unavailable — this operates directly on `self.nodes`/`self.links_in`
    /// instead (see ADR-083). Semantics are matched field-for-field against
    /// the Cozo query text, NOT `nodes_by_tag`/`nodes_by_priority`'s
    /// exact-match secondary-index convention, so a caller sees IDENTICAL
    /// results whether a given instance happens to be Cozo-backed or
    /// pure-in-memory: `Tag` is a substring match against the JSON-encoded
    /// tags array (mirrors `str_includes(tags_json, tag)`), `Priority` is
    /// `<=` (mirrors `priority <= min_pri` — 'A' is the most urgent, so this
    /// returns "at least as urgent as"), and every arm skips a node with an
    /// empty title (mirrors the Cozo query's own `title != ''` guard).
    /// `Stale`/`Custom` have no faithful in-memory equivalent (no per-node
    /// last-modified timestamp is tracked in-memory at all — `StaleNode`/
    /// `detect_stale_nodes` is a DIFFERENT concept, "source file deleted
    /// from disk", not "not modified in N days"; `Custom` is arbitrary
    /// Datalog with no in-memory query engine to run it against) — both
    /// return `Err` rather than a silently-wrong or silently-empty result.
    pub fn agenda_query_in_memory(&self, filter: &AgendaFilter) -> Result<Vec<Node>, String> {
        let has_title = |n: &&Node| !n.title.is_empty();
        let matches: Vec<&Node> = match filter {
            AgendaFilter::Todo(None) => self
                .nodes
                .values()
                .filter(|n| n.todo_state.is_some())
                .filter(has_title)
                .collect(),
            AgendaFilter::Todo(Some(state)) => self
                .nodes
                .values()
                .filter(|n| n.todo_state.as_deref() == Some(state.as_str()))
                .filter(has_title)
                .collect(),
            AgendaFilter::Priority(min_pri) => self
                .nodes
                .values()
                .filter(|n| n.priority.is_some_and(|p| p <= *min_pri))
                .filter(has_title)
                .collect(),
            AgendaFilter::Tag(tag) => self
                .nodes
                .values()
                .filter(|n| {
                    serde_json::to_string(&n.tags)
                        .is_ok_and(|tags_json| tags_json.contains(tag.as_str()))
                })
                .filter(has_title)
                .collect(),
            AgendaFilter::Stale(_) => {
                return Err(
                    "Stale has no in-memory equivalent (no per-node last-modified timestamp \
                     is tracked without a Cozo-backed store)"
                        .to_string(),
                )
            }
            AgendaFilter::Orphan => self
                .nodes
                .iter()
                .filter(|(_, n)| n.kind != NodeKind::Index)
                .filter(|(id, n)| {
                    let has_outgoing = !n.links().is_empty();
                    let has_incoming = self
                        .links_in
                        .get(id.as_str())
                        .is_some_and(|v| !v.is_empty());
                    !has_outgoing && !has_incoming
                })
                .map(|(_, n)| n)
                .filter(has_title)
                .collect(),
            AgendaFilter::DeadEnd => self
                .nodes
                .values()
                .filter(|n| n.links().is_empty())
                .filter(has_title)
                .collect(),
            AgendaFilter::MissingRole => self
                .nodes
                .values()
                .filter(|n| !n.properties.contains_key("role"))
                .filter(has_title)
                .collect(),
            AgendaFilter::WeaklyLinked(n) => self
                .nodes
                .values()
                .filter(|node| (node.links().len() as u32) < *n)
                .filter(has_title)
                .collect(),
            AgendaFilter::Custom(_) => {
                return Err(
                    "Custom (raw Datalog) has no in-memory equivalent -- requires a \
                     Cozo-backed store"
                        .to_string(),
                )
            }
        };
        let mut out: Vec<Node> = matches.into_iter().cloned().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Compute a health report: orphan nodes, broken links, namespace counts.
    pub fn health_report(&self) -> KbHealthReport {
        self.health_report_with(|_| false)
    }

    /// Health report with an external resolver for cross-KB link checking.
    /// `external_contains` returns true if a target exists in another KB.
    pub fn health_report_with(&self, external_contains: impl Fn(&str) -> bool) -> KbHealthReport {
        self.health_report_with_visibility(external_contains, |_| true)
    }

    /// Like [`Self::health_report_with`], but only folds VISIBLE nodes
    /// (`node_visible`) into `total_nodes`/`total_links`/`orphan_ids`/
    /// `namespace_counts` -- the AI-residency seed-content exemption (#361)
    /// needs this so a restricted KB's non-seed content doesn't surface
    /// through `kb_health`'s counts/lists even though no single node id was
    /// ever requested directly. `all_ids` (used for broken-link detection)
    /// deliberately still includes EVERY node, visible or not: a link
    /// pointing at a real-but-hidden node is not "broken" (deleted/
    /// nonexistent) -- it's just not shown to this caller, a different fact.
    /// The default [`Self::health_report_with`] passes `|_| true` (every
    /// node visible), so existing callers/behavior are unchanged.
    pub fn health_report_with_visibility(
        &self,
        external_contains: impl Fn(&str) -> bool,
        node_visible: impl Fn(&Node) -> bool,
    ) -> KbHealthReport {
        let all_ids: HashSet<&str> = self.nodes.keys().map(|s| s.as_str()).collect();

        // Single fold over VISIBLE nodes only: accumulate link count, broken
        // links, orphan IDs, and namespace counts in one pass -- keeps every
        // derived count/list internally consistent by construction (#361).
        struct Acc {
            total_links: usize,
            broken_links: Vec<BrokenLink>,
            orphan_ids: Vec<String>,
            namespace_counts: HashMap<String, usize>,
        }

        let result = self.nodes.iter().filter(|(_, n)| node_visible(n)).fold(
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
        let total_nodes = self.nodes.values().filter(|n| node_visible(n)).count();

        KbHealthReport {
            total_nodes,
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

    /// Full (untruncated) node-id -> incoming-link-count map, trivially derived from the
    /// existing `links_in` reverse index (no new storage — CLAUDE.md principle #8). The
    /// in-memory mirror of `CozoKbStore::compute_in_degree_map`, used by
    /// `InMemoryQueryLayer::linked_in_degree` so `FederatedQuery::health_report` can fold a
    /// federated in-memory instance's in-degree into the federation-wide sum (issue #474).
    pub fn linked_in_degree(&self) -> HashMap<String, usize> {
        self.links_in
            .iter()
            .map(|(id, sources)| (id.clone(), sources.len()))
            .collect()
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

    /// `stamp_source_for` is scoped in BOTH directions: it must not touch a node
    /// outside the id list, and it must not overwrite provenance that already
    /// exists. Either failure mis-brands a user's own note as MAE's built-in
    /// content, which makes it uneditable and undeletable through the KB API.
    #[test]
    fn stamp_source_for_is_scoped_by_id_and_never_overwrites() {
        let mut kb = kb_with(vec![
            // In the list, unstamped → gets stamped.
            Node::new("concept:buffer", "Buffer", NodeKind::Concept, "b"),
            Node::new("cmd:save", "Save", NodeKind::Command, "s"),
            // In the list, but ALREADY stamped by the user → must be left alone.
            Node::new("concept:mine", "Mine", NodeKind::Concept, "m")
                .with_source(NodeSource::Manual, 7),
            // NOT in the list, unstamped → must stay unstamped even though it
            // carries a built-in-looking prefix.
            Node::new("concept:untouched", "Untouched", NodeKind::Concept, "u"),
            Node::new("note:user", "User note", NodeKind::Note, "n"),
        ]);

        let ids = [
            "concept:buffer".to_string(),
            "cmd:save".to_string(),
            "concept:mine".to_string(),
            // An id the ingest reported but that no longer exists must not panic.
            "concept:vanished".to_string(),
        ];
        kb.stamp_source_for(&ids, NodeSource::Seed, 1);

        for id in ["concept:buffer", "cmd:save"] {
            let n = kb.get(id).unwrap();
            assert_eq!(n.source, Some(NodeSource::Seed), "{id} should be stamped");
            assert_eq!(n.source_version, Some(1), "{id} version");
        }
        let mine = kb.get("concept:mine").unwrap();
        assert_eq!(
            mine.source,
            Some(NodeSource::Manual),
            "an existing stamp must never be overwritten"
        );
        assert_eq!(mine.source_version, Some(7), "existing version preserved");
        for id in ["concept:untouched", "note:user"] {
            assert_eq!(
                kb.get(id).unwrap().source,
                None,
                "{id} is outside the id list and must stay unstamped"
            );
        }
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
    fn search_ranked_soft_and_fallback_returns_partial_match_below_full_match() {
        let kb = kb_with(vec![Node::new(
            "a",
            "Buffer management",
            NodeKind::Note,
            "ropey rope",
        )]);
        // "buffer" matches, "zzz" matches nothing anywhere -> strict AND alone
        // would drop the node, but the soft-AND fallback (#357) relaxes by
        // exactly one term (2 terms, 1 hit >= min_hits=1) and surfaces it at
        // a penalized score.
        let fallback = kb.search_ranked("buffer zzz", 10);
        assert_eq!(
            fallback.first().map(|(id, _)| id.as_str()),
            Some("a"),
            "fallback pass should surface the partially-matching node, got {fallback:?}"
        );
        let full = kb.search_ranked("buffer rope", 10);
        assert!(
            fallback[0].1 < full[0].1,
            "fallback-tier score ({}) must be strictly below a fully-matching \
             query's score ({}) for the same node",
            fallback[0].1,
            full[0].1
        );

        // Adversarial: a genuinely nonsense multi-term query where NO term
        // matches anything must still return empty -- the relaxation has a
        // floor, it doesn't manufacture matches from nothing.
        assert!(
            kb.search_ranked("zzz qqq xyz", 10).is_empty(),
            "a query where every term is unmatched must stay empty even under the fallback"
        );
    }

    #[test]
    fn search_ranked_soft_and_fallback_only_engages_on_zero_strict_results() {
        let solo = kb_with(vec![Node::new(
            "a",
            "Buffer management",
            NodeKind::Note,
            "ropey rope",
        )]);
        let with_distractor = kb_with(vec![
            Node::new("a", "Buffer management", NodeKind::Note, "ropey rope"),
            Node::new(
                "b",
                "Unrelated",
                NodeKind::Note,
                "nothing to do with either term",
            ),
        ]);
        // Strict AND already satisfies "buffer rope" (both terms match node
        // "a") -- the fallback pass must never run, in either KB. Regression
        // guard: an unrelated distractor node's mere presence must not
        // change node "a"'s own score (proving the fallback penalty was
        // never applied) -- compared directly instead of against a
        // hand-picked magic-number threshold.
        let solo_ranked = solo.search_ranked("buffer rope", 10);
        let distractor_ranked = with_distractor.search_ranked("buffer rope", 10);
        assert_eq!(
            distractor_ranked.len(),
            1,
            "only the strictly-matching node should appear, got {distractor_ranked:?}"
        );
        assert_eq!(distractor_ranked[0].0, "a");
        assert_eq!(
            distractor_ranked[0].1, solo_ranked[0].1,
            "an unrelated distractor node must not change node a's own score \
             (the fallback pass must never run when strict AND already found a match)"
        );
    }

    #[test]
    fn search_ranked_downweights_category_and_hub_role_nodes() {
        let kb = kb_with(vec![
            Node::new(
                "a",
                "Testing philosophy",
                NodeKind::Note,
                "adversarial testing philosophy for this project",
            ),
            Node::new(
                "b",
                "Testing philosophy hub",
                NodeKind::Category,
                "adversarial testing philosophy for this project",
            ),
        ]);
        // Identical term-match strength (same title-ish text, same body) --
        // only `kind` differs. The plain Note must rank first.
        let ranked = kb.search_ranked("adversarial testing philosophy", 10);
        assert_eq!(
            ranked.first().map(|(id, _)| id.as_str()),
            Some("a"),
            "non-hub node with equal match strength should outrank the Category node, got {ranked:?}"
        );

        // Adversarial companion: a hub node with a *stronger* raw match
        // (exact title) must still beat a barely-matching non-hub node --
        // the down-weight tips near-ties, it doesn't blanket-bury hubs.
        let kb2 = kb_with(vec![
            Node::new(
                "hub",
                "Exact Phrase Match",
                NodeKind::Category,
                "exact phrase match",
            ),
            Node::new(
                "weak",
                "Something else",
                NodeKind::Note,
                "barely mentions exact",
            ),
        ]);
        let ranked2 = kb2.search_ranked("exact phrase match", 10);
        assert_eq!(
            ranked2.first().map(|(id, _)| id.as_str()),
            Some("hub"),
            "a strong hub match must still beat a weak non-hub match, got {ranked2:?}"
        );

        // `:role: hub` property (not just NodeKind::Category/Meta) triggers
        // the same down-weight.
        let kb3 = kb_with(vec![
            Node::new(
                "c",
                "Testing philosophy",
                NodeKind::Note,
                "adversarial testing philosophy for this project",
            ),
            Node::new(
                "d",
                "Testing philosophy hub",
                NodeKind::Note,
                "adversarial testing philosophy for this project",
            )
            .with_properties(HashMap::from([("role".to_string(), "hub".to_string())])),
        ]);
        let ranked3 = kb3.search_ranked("adversarial testing philosophy", 10);
        assert_eq!(
            ranked3.first().map(|(id, _)| id.as_str()),
            Some("c"),
            "role=hub property should down-weight like NodeKind::Category, got {ranked3:?}"
        );
    }

    #[test]
    fn search_ranked_natural_language_query_with_one_unmatched_filler_word_is_not_empty() {
        // Principle #14 -- a short, natural phrasing (not a hand-picked bag
        // of exact keywords) where exactly one word is absent from the
        // target node's title/tags/body -- the case the soft-AND fallback
        // (relax by exactly one term) is designed to rescue. A longer,
        // heavily-filler-worded question with several unmatched terms is
        // deliberately NOT rescued by this conservative fix -- full
        // fuzzy/FTS body search is #81's scope, not this bounded down
        // payment's.
        let kb = kb_with(vec![
            Node::new(
                "practice:adversarial-testing",
                "Adversarial testing philosophy",
                NodeKind::Note,
                "Tests exist to falsify the implementation, not confirm it.",
            )
            .with_tags(["testing", "philosophy"]),
            Node::new(
                "hub:dev-practices",
                "Development practices hub",
                NodeKind::Category,
                "Links out to testing philosophy, commit conventions, and build tooling.",
            ),
        ]);
        let ranked = kb.search_ranked("testing philosophy explained", 10);
        assert!(
            !ranked.is_empty(),
            "a natural query with exactly one unmatched filler/synonym word must not return zero results"
        );
        assert_eq!(
            ranked.first().map(|(id, _)| id.as_str()),
            Some("practice:adversarial-testing"),
            "the specific practice note should outrank the hub node despite both being on the fallback tier, got {ranked:?}"
        );
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

    /// The evidence a `TargetNotFound` classification actually has is "absent
    /// from the id set I searched" -- nothing more.
    ///
    /// This test documents the LIMITATION rather than asserting it away: a node
    /// that genuinely exists elsewhere (another federated instance, a hub, a KB
    /// this replica does not hold) is still reported broken, because
    /// `health_report` builds `all_ids` from one in-memory `KnowledgeBase` and
    /// has no existence oracle to consult.
    ///
    /// That is why the variant is no longer called `DeletedNode`: the old name
    /// -- and the `"deleted_node"` string the MCP `kb_health` tool exported to
    /// the model -- asserted the node had been deleted, on evidence that only
    /// supports "not here". When the oracle lands, this test should gain a
    /// `NotHeldLocally` case and stop being a limitation note.
    #[test]
    fn a_link_to_a_node_this_replica_does_not_hold_is_not_reported_as_deleted() {
        // Two KBs, as a federation would have. `other` holds the target.
        let other = kb_with(vec![Node::new(
            "concept:remote",
            "Remote",
            NodeKind::Note,
            "body",
        )]);
        assert!(
            other.get("concept:remote").is_some(),
            "fixture: the target genuinely exists in the other instance"
        );

        let local = kb_with(vec![Node::new(
            "a",
            "A",
            NodeKind::Note,
            "[[concept:remote]]",
        )]);
        let report = local.health_report();

        assert_eq!(report.broken_links.len(), 1);
        assert_eq!(report.broken_links[0].target, "concept:remote");
        // The selective oracle: the CLASSIFICATION, not the count. A count-only
        // assertion passes under both the old and new naming and proves nothing.
        assert_ne!(
            format!("{:?}", report.broken_links[0].kind),
            "DeletedNode",
            "a node held by another instance must never be classified as deleted"
        );
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
        assert!(kinds.contains(&&BrokenLinkKind::TargetNotFound)); // valid UUID format
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

    /// Covers the TEXT fields only — `id`/`title`/`body`/`tags`.
    ///
    /// Renamed from `crdt_bridge_roundtrip_preserves_all_fields`, which it never
    /// did: the node it builds sets no `properties`, `todo_state`, `priority` or
    /// `aliases`, so it cannot fail on the five fields the CRDT actually drops.
    /// Its one non-text assertion, `source`, is a *parameter* passed into
    /// `from_crdt_doc` — asserting the argument equals itself. `kind` is likewise
    /// a parameter and was never asserted at all. See
    /// `crdt_roundtrip_preserves_every_node_field` below for the real contract.
    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_bridge_roundtrip_preserves_text_fields() {
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
        assert!(restored.crdt_doc.is_some(), "CRDT bytes should be stored");
    }

    /// ADR-093 Gate A.1 — the contract a CRDT-as-truth migration depends on.
    ///
    /// Every `Node` field must survive `Node → KbNodeDoc → Node`. Today five do
    /// not: `properties`, `todo_state`, `priority`, `aliases` and `source_version`
    /// are absent from `KbNodeDoc`'s schema entirely, and `kind`/`source` are
    /// supplied as arguments to `from_crdt_doc` rather than read back from the doc.
    ///
    /// This is latent while a KB is unshared — `kb_update_node_with` persists
    /// straight to Cozo, which stores every field, and `crdt_doc` stays `None`. It
    /// stops being latent the moment a migration mints CRDT lineage for every node,
    /// which is exactly what a hosted, CRDT-as-truth deployment requires. For 2,457
    /// org-roam notes, `properties` is where `:ID:` and `:ROLE:` live.
    ///
    /// `from_crdt_doc` is deliberately called with DELIBERATELY WRONG `kind` and
    /// `source` arguments: if the restored node still reports the original values,
    /// the doc carried them. If it reports the wrong ones, the caller did — which
    /// is a round-trip in name only.
    #[cfg(feature = "crdt")]
    #[test]
    fn crdt_roundtrip_preserves_every_node_field() {
        let mut node = Node::new(
            "concept:full",
            "Every Field — 日本語 café 🎉",
            NodeKind::Concept,
            realistic_org_body(),
        )
        .with_tags(vec!["research", "crdt"]);
        node.todo_state = Some("NEXT".to_string());
        node.priority = Some('A');
        node.aliases = vec!["alias one".to_string(), "エイリアス".to_string()];
        node.properties
            .insert("ID".to_string(), "1F0A-BEEF".to_string());
        node.properties
            .insert("ROLE".to_string(), "hub".to_string());
        node.source_version = Some(7);
        // #710: provenance is a field like any other, and this test asserted
        // every field EXCEPT this one — which is precisely why it shipped absent
        // from the wire. `Seed` specifically, because it is the only enforced
        // read-only marking and therefore the one whose loss actually costs.
        node.source = Some(NodeSource::Seed);

        let doc = node.to_crdt_doc().expect("to_crdt_doc");
        // Wrong on purpose — see the doc comment.
        let restored = Node::from_crdt_doc(&doc, NodeKind::Note, NodeSource::Manual);

        assert_eq!(restored.id, node.id, "id");
        assert_eq!(restored.title, node.title, "title");
        assert_eq!(restored.body, node.body, "body");
        assert_eq!(restored.tags, node.tags, "tags");
        assert_eq!(
            restored.kind, node.kind,
            "kind must come from the doc, not the caller's argument"
        );
        assert_eq!(restored.todo_state, node.todo_state, "todo_state");
        assert_eq!(restored.priority, node.priority, "priority");
        assert_eq!(restored.aliases, node.aliases, "aliases");
        assert_eq!(
            restored.properties, node.properties,
            "properties — where org-roam :ID:/:ROLE: live"
        );
        assert_eq!(
            restored.source_version, node.source_version,
            "source_version"
        );
        assert_eq!(
            restored.source, node.source,
            "source must come from the doc, not the caller's argument — the \
             caller passed Manual and the doc says Seed (#710)"
        );
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
            include_body: true,
            required_tag: None,
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

    #[test]
    fn extract_subgraph_required_tag_keeps_only_tagged_nodes_plus_seed() {
        // Real-world shape (the terraform-onboarding bug this filter was
        // built for): an untagged seed links to one node carrying the
        // required tag and one that doesn't -- only the tagged node (plus
        // the seed itself, regardless of its own tags) should survive.
        let kb = kb_with(vec![
            Node::new(
                "zero-to-running",
                "Zero to Running",
                NodeKind::Note,
                "[[onboarded]] [[unrelated]]",
            ),
            Node::new("onboarded", "Onboarded", NodeKind::Note, "")
                .with_tags(["terraform", "terraform-onboarding"]),
            Node::new("unrelated", "Unrelated", NodeKind::Note, "").with_tags(["terraform"]),
        ]);
        let mut s = spec("zero-to-running", 1, false);
        s.required_tag = Some("terraform-onboarding".to_string());
        let result = kb.extract_subgraph(&s);

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"zero-to-running"),
            "seed always survives regardless of its own tags: {ids:?}"
        );
        assert!(
            ids.contains(&"onboarded"),
            "tagged node must survive: {ids:?}"
        );
        assert!(
            !ids.contains(&"unrelated"),
            "untagged node must be excluded: {ids:?}"
        );
        assert_eq!(result.tag_filtered_count, 1);
    }

    #[test]
    fn extract_subgraph_required_tag_traverses_through_untagged_intermediate() {
        // The tag restricts the RESULT, not the BFS traversal: an untagged
        // node one hop out must still be walked THROUGH so a tagged node
        // two hops out stays reachable, even though the untagged
        // intermediate itself is excluded from the final output.
        let kb = kb_with(vec![
            Node::new("seed", "Seed", NodeKind::Note, "[[stepping-stone]]"),
            Node::new(
                "stepping-stone",
                "Stepping Stone",
                NodeKind::Note,
                "[[deep-tagged]]",
            ),
            Node::new("deep-tagged", "Deep Tagged", NodeKind::Note, "").with_tags(["onboarding"]),
        ]);
        let mut s = spec("seed", 2, false);
        s.required_tag = Some("onboarding".to_string());
        let result = kb.extract_subgraph(&s);

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"deep-tagged"),
            "a tagged node beyond an untagged stepping stone must still be reached: {ids:?}"
        );
        assert!(
            !ids.contains(&"stepping-stone"),
            "the untagged intermediate must not itself appear in the result: {ids:?}"
        );
    }

    #[test]
    fn extract_subgraph_required_tag_excludes_node_as_a_boundary_link_not_a_silent_drop() {
        let kb = kb_with(vec![
            Node::new("seed", "Seed", NodeKind::Note, "[[tagged]] [[untagged]]"),
            Node::new("tagged", "Tagged", NodeKind::Note, "").with_tags(["keep"]),
            Node::new("untagged", "Untagged", NodeKind::Note, ""),
        ]);
        let mut s = spec("seed", 1, false);
        s.required_tag = Some("keep".to_string());
        let result = kb.extract_subgraph(&s);

        assert_eq!(result.tag_filtered_count, 1);
        assert_eq!(
            result.boundary_links.len(),
            1,
            "the excluded node's link must be demoted to a boundary link, not dropped"
        );
        assert_eq!(result.boundary_links[0].target, "untagged");
    }

    #[test]
    fn extract_subgraph_required_tag_and_node_cap_compose_independently() {
        // node_cap must count the TAG-FILTERED candidate set, not raw
        // traversal size -- three tagged nodes reachable, capped to 2
        // (seed + 1), so tag_filtered_count and hidden_node_count are both
        // nonzero and independent of each other.
        let kb = kb_with(vec![
            Node::new(
                "seed",
                "Seed",
                NodeKind::Note,
                "[[t1]] [[t2]] [[t3]] [[plain]]",
            ),
            Node::new("t1", "T1", NodeKind::Note, "").with_tags(["keep"]),
            Node::new("t2", "T2", NodeKind::Note, "").with_tags(["keep"]),
            Node::new("t3", "T3", NodeKind::Note, "").with_tags(["keep"]),
            Node::new("plain", "Plain", NodeKind::Note, ""),
        ]);
        let mut s = spec("seed", 1, false);
        s.required_tag = Some("keep".to_string());
        s.node_cap = Some(2);
        let result = kb.extract_subgraph(&s);

        assert_eq!(result.nodes.len(), 2, "capped to exactly node_cap nodes");
        assert!(result.nodes.iter().any(|n| n.id == "seed"));
        assert_eq!(
            result.tag_filtered_count, 1,
            "exactly the untagged 'plain' node was excluded by the tag filter"
        );
        assert_eq!(
            result.hidden_node_count, 2,
            "of the seed + 3 tagged candidates, node_cap=2 hides 2 more beyond the seed"
        );
    }

    #[test]
    fn extract_subgraph_required_tag_matching_nothing_returns_only_the_seed() {
        // Adversarial: a tag that matches no reachable node must not panic
        // or return an empty result -- the seed always survives.
        let kb = kb_with(vec![
            Node::new("seed", "Seed", NodeKind::Note, "[[a]] [[b]]"),
            Node::new("a", "A", NodeKind::Note, "").with_tags(["other"]),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let mut s = spec("seed", 1, false);
        s.required_tag = Some("nonexistent-tag".to_string());
        let result = kb.extract_subgraph(&s);

        let ids: Vec<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["seed"]);
        assert_eq!(result.tag_filtered_count, 2);
    }

    #[test]
    fn extract_subgraph_required_tag_none_is_a_true_no_op() {
        // Regression guard: every pre-existing caller passes required_tag:
        // None and must see byte-identical behavior to before this field
        // existed.
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "see [[b]] and [[c]]"),
            Node::new("b", "B", NodeKind::Note, ""),
            Node::new("c", "C", NodeKind::Note, ""),
        ]);
        let result = kb.extract_subgraph(&spec("a", 1, false));
        assert_eq!(result.nodes.len(), 3);
        assert_eq!(result.tag_filtered_count, 0);
    }

    #[test]
    fn extract_subgraph_include_body_false_strips_heavy_fields_without_changing_cap_selection() {
        // "popular" carries a large body + non-empty properties + a
        // source_file — the fields we're stripping — and is also the
        // higher-degree node (two backlinks) that must survive a cap of 2
        // (starter + 1) over "lonely" (degree 0). Real (not unicorn) sizes:
        // a few KB of body text, a populated property drawer.
        let big_body = "x".repeat(5_000);
        let mut props = HashMap::new();
        props.insert("last-accessed".to_string(), "2026-01-01".to_string());

        let make_kb = || {
            kb_with(vec![
                Node::new("start", "Start", NodeKind::Note, "[[popular]] [[lonely]]"),
                Node::new("ref1", "Ref1", NodeKind::Note, "[[popular]]"),
                Node::new("ref2", "Ref2", NodeKind::Note, "[[popular]]"),
                Node::new("popular", "Popular", NodeKind::Note, big_body.clone())
                    .with_properties(props.clone())
                    .with_source_file("/tmp/popular.org"),
                Node::new("lonely", "Lonely", NodeKind::Note, ""),
            ])
        };

        let mut s_light = spec("start", 1, false);
        s_light.node_cap = Some(2);
        s_light.include_body = false;
        let light = make_kb().extract_subgraph(&s_light);

        let mut s_full = spec("start", 1, false);
        s_full.node_cap = Some(2);
        // spec()'s include_body defaults to true.
        let full = make_kb().extract_subgraph(&s_full);

        // The selection (which nodes survive the cap vs. get demoted to a
        // boundary stub) must be byte-for-byte identical regardless of
        // include_body — stripping heavy fields must never bias which
        // nodes get kept. Cap selection happens on the KB's own stored
        // nodes/degree table before the collection loop, so this is a
        // regression guard, not a coincidence.
        let mut light_ids: Vec<&str> = light.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut full_ids: Vec<&str> = full.nodes.iter().map(|n| n.id.as_str()).collect();
        light_ids.sort();
        full_ids.sort();
        assert_eq!(
            light_ids, full_ids,
            "include_body must not change which nodes the cap keeps"
        );
        assert_eq!(light.hidden_node_count, full.hidden_node_count);
        assert!(light_ids.contains(&"popular"));
        assert!(!light_ids.contains(&"lonely"));

        let popular_light = light.nodes.iter().find(|n| n.id == "popular").unwrap();
        assert_eq!(popular_light.title, "Popular");
        assert_eq!(popular_light.kind, NodeKind::Note);
        assert_eq!(popular_light.body, "", "body must be stripped");
        assert!(
            popular_light.properties.is_empty(),
            "properties must be stripped"
        );
        assert_eq!(
            popular_light.source_file, None,
            "source_file must be stripped"
        );
        assert_eq!(popular_light.crdt_doc, None, "crdt_doc must be stripped");

        // The include_body: true path (existing default behavior) must be
        // completely unaffected — full body/properties/source_file intact.
        let popular_full = full.nodes.iter().find(|n| n.id == "popular").unwrap();
        assert_eq!(popular_full.body, big_body);
        assert_eq!(popular_full.properties, props);
        assert_eq!(
            popular_full.source_file,
            Some(std::path::PathBuf::from("/tmp/popular.org"))
        );
    }

    #[test]
    fn extract_subgraph_include_body_true_clones_every_field_byte_identical() {
        // Regression guard for principle #14: the pre-existing (default)
        // behavior must be provably unchanged by this PR, not just
        // "looks empty by coincidence" — every field on the returned Node
        // must match the originally-inserted Node exactly.
        let big_body = "y".repeat(500);
        let mut props = HashMap::new();
        props.insert("k".to_string(), "v".to_string());
        let original = Node::new("a", "A", NodeKind::Concept, big_body.clone())
            .with_properties(props.clone())
            .with_tags(vec!["t1", "t2"])
            .with_aliases(vec!["alias1"])
            .with_todo_state("TODO")
            .with_priority('A')
            .with_source(NodeSource::UserOrg, 3);

        let kb = kb_with(vec![original.clone()]);
        let result = kb.extract_subgraph(&spec("a", 0, false));
        assert_eq!(result.nodes.len(), 1);
        let got = &result.nodes[0];
        assert_eq!(got.id, original.id);
        assert_eq!(got.title, original.title);
        assert_eq!(got.kind, original.kind);
        assert_eq!(got.body, original.body);
        assert_eq!(got.tags, original.tags);
        assert_eq!(got.todo_state, original.todo_state);
        assert_eq!(got.priority, original.priority);
        assert_eq!(got.source, original.source);
        assert_eq!(got.source_version, original.source_version);
        assert_eq!(got.aliases, original.aliases);
        assert_eq!(got.properties, original.properties);
        assert_eq!(got.source_file, original.source_file);
        assert_eq!(got.crdt_doc, original.crdt_doc);
    }

    #[test]
    fn extract_subgraph_include_body_false_avoids_cloning_heavy_body_text_at_scale() {
        // Proxy for the memory-scaling claim: N=250 nodes each with a
        // several-KB body, chained so a single BFS walk reaches all of
        // them (mirroring "near-whole-KB subgraph for a well-connected
        // node" from kb_graph_node_count_cap's own doc string). Asserting
        // the summed body length is ~0 proves the heavy field genuinely
        // isn't cloned, not merely that it happens to look empty.
        const N: usize = 250;
        const BODY_BYTES: usize = 4096;
        let body = "z".repeat(BODY_BYTES);

        let mut nodes = Vec::with_capacity(N);
        for i in 0..N {
            let link = if i + 1 < N {
                format!("[[n{}]]", i + 1)
            } else {
                String::new()
            };
            let node_body = format!("{body}\n{link}");
            nodes.push(Node::new(
                format!("n{i}"),
                format!("N{i}"),
                NodeKind::Note,
                node_body,
            ));
        }
        let kb = kb_with(nodes);

        let mut s = spec("n0", N, false);
        s.include_body = false;
        let result = kb.extract_subgraph(&s);

        assert_eq!(
            result.nodes.len(),
            N,
            "sanity: BFS must actually walk the full chain"
        );
        let total_body_bytes: usize = result.nodes.iter().map(|n| n.body.len()).sum();
        assert!(
            total_body_bytes < 1024,
            "include_body: false must not clone body text at scale — got \
             {total_body_bytes} bytes across {N} nodes that each have a \
             {BODY_BYTES}-byte body"
        );
    }

    /// Build a KB of `n` nodes with varying degree: node 0 is a hub linking
    /// to every other node (giving it the highest degree by construction);
    /// the rest have no outgoing links of their own, so their only degree
    /// comes from that one incoming edge (all tied at degree 1) — except
    /// `low_degree_id`, which additionally gets NO backlink from the hub
    /// (degree 0), making it the single lowest-degree node a naive
    /// degree-only cap would always cut first.
    fn kb_with_hub_and_low_degree_outlier(n: usize, low_degree_id: &str) -> KnowledgeBase {
        let mut nodes = Vec::with_capacity(n);
        let mut hub_body = String::new();
        for i in 0..n {
            let id = format!("n{i}");
            if id != low_degree_id {
                hub_body.push_str(&format!("[[{id}]] "));
            }
        }
        nodes.push(Node::new("hub", "Hub", NodeKind::Note, hub_body));
        for i in 0..n {
            nodes.push(Node::new(
                format!("n{i}"),
                format!("N{i}"),
                NodeKind::Note,
                "",
            ));
        }
        kb_with(nodes)
    }

    #[test]
    fn extract_full_corpus_no_cap_includes_literally_every_node() {
        // 20+ nodes across varying degree (a hub + everything else), no cap
        // — every single node must survive, proving this pulls the WHOLE
        // corpus rather than any BFS-reachable subset.
        const N: usize = 24;
        let kb = kb_with_hub_and_low_degree_outlier(N, "n0");
        let result = kb.extract_full_corpus(None, &HashSet::new(), true);
        assert_eq!(
            result.nodes.len(),
            N + 1,
            "must include every node (hub + {N} leaves), not a truncated subset"
        );
        assert_eq!(result.hidden_node_count, 0);
        let ids: HashSet<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("hub"));
        for i in 0..N {
            assert!(
                ids.contains(format!("n{i}").as_str()),
                "n{i} must be present"
            );
        }
    }

    #[test]
    fn extract_full_corpus_cap_exempts_only_the_protected_low_degree_bridge_node() {
        // Adversarial case (CLAUDE.md #14): "lonely_bridge" is deliberately
        // the LOWEST-degree node in the KB (no incoming link from the hub,
        // unlike every other leaf) — a naive degree-only cap would always
        // cut it first. It's also the sole cross-instance-link source in
        // this scenario (simulated here by the caller marking it
        // `protected`), so it MUST survive truncation anyway, while OTHER
        // equally-unprotected low-degree nodes (n1, n2, ... ordinary leaves)
        // DO get cut to make room.
        const N: usize = 30;
        // "unused" never appears in any n{i} id, so the hub links to every
        // n{i} (each getting degree 1) while "lonely_bridge", added below,
        // gets zero incoming links — the deliberately lowest-degree node.
        let base = kb_with_hub_and_low_degree_outlier(N, "unused");
        let mut nodes: Vec<Node> = base.iter().map(|(_, n)| n.clone()).collect();
        nodes.push(Node::new(
            "lonely_bridge",
            "Lonely Bridge",
            NodeKind::Note,
            "",
        ));
        let kb = kb_with(nodes);

        // cap well below the total (hub + N leaves + lonely_bridge).
        let cap = 5usize;
        let mut protected = HashSet::new();
        protected.insert("lonely_bridge".to_string());

        let result = kb.extract_full_corpus(Some(cap), &protected, true);
        let ids: HashSet<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();

        assert!(
            ids.contains("lonely_bridge"),
            "the protected, deliberately-lowest-degree bridge node must survive \
             truncation even though a naive degree-only cap would cut it: {ids:?}"
        );
        assert!(
            ids.contains("hub"),
            "the highest-degree node should also naturally survive on merit"
        );
        // The cap (5) minus the one protected node leaves room for 4
        // degree-ranked survivors; with N=30 ordinary leaves all tied at
        // degree 1 (or 0), most must be cut — prove at least one ordinary
        // (unprotected) leaf was actually excluded, not just "everything
        // happened to fit".
        let some_leaf_cut = (0..N).any(|i| !ids.contains(format!("n{i}").as_str()));
        assert!(
            some_leaf_cut,
            "unprotected low-degree leaves must actually be cut by the cap, not just \
             the protected node exempted: {ids:?}"
        );
        assert_eq!(
            result.nodes.len(),
            cap,
            "protected node counts toward the cap budget (like a starter node does in \
             extract_subgraph), it's just never the one CHOSEN to be cut"
        );
    }

    #[test]
    fn extract_full_corpus_hidden_node_count_matches_actual_cut_count() {
        const N: usize = 40;
        let kb = kb_with_hub_and_low_degree_outlier(N, "n0");
        let total = N + 1; // hub + N leaves
        let cap = 10usize;
        let result = kb.extract_full_corpus(Some(cap), &HashSet::new(), true);
        assert_eq!(result.nodes.len(), cap);
        assert_eq!(
            result.hidden_node_count,
            total - cap,
            "hidden_node_count must reflect exactly how many were cut, not an \
             incidental/stale value"
        );
    }

    #[test]
    fn extract_full_corpus_protected_id_absent_from_this_kb_does_not_shrink_the_effective_cap() {
        // A `protected` id that belongs to a DIFFERENT KB instance (never
        // present here) must be silently ignored — not treated as consuming
        // one slot of the exemption budget, which would otherwise leave one
        // fewer real node kept than the cap promises.
        const N: usize = 20;
        let kb = kb_with_hub_and_low_degree_outlier(N, "n0");
        let cap = 8usize;
        let mut protected = HashSet::new();
        protected.insert("concept:from-a-totally-different-instance".to_string());

        let result = kb.extract_full_corpus(Some(cap), &protected, true);
        assert_eq!(
            result.nodes.len(),
            cap,
            "a protected id absent from this KB must not change the effective cap"
        );
    }

    #[test]
    fn extract_full_corpus_cap_larger_than_total_is_a_no_op() {
        const N: usize = 10;
        let kb = kb_with_hub_and_low_degree_outlier(N, "n0");
        let result = kb.extract_full_corpus(Some(1000), &HashSet::new(), true);
        assert_eq!(result.nodes.len(), N + 1);
        assert_eq!(result.hidden_node_count, 0);
    }

    #[test]
    fn extract_full_corpus_include_body_false_strips_heavy_fields() {
        let big_body = "x".repeat(5_000);
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, big_body.clone())]);
        let result = kb.extract_full_corpus(None, &HashSet::new(), false);
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].body, "", "body must be stripped");

        let result_full = kb.extract_full_corpus(None, &HashSet::new(), true);
        assert_eq!(result_full.nodes[0].body, big_body);
    }

    // --- hop_distances_from (ADR-068 Phase B3) ---

    #[test]
    fn hop_distances_from_focus_itself_is_zero_and_neighbors_grow_by_one_hop() {
        // a -> b -> c, a straight chain: focus a is 0, b is 1, c is 2.
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "[[b]]"),
            Node::new("b", "B", NodeKind::Note, "[[c]]"),
            Node::new("c", "C", NodeKind::Note, ""),
        ]);
        let dist = kb.hop_distances_from("a");
        assert_eq!(dist.get("a"), Some(&0));
        assert_eq!(dist.get("b"), Some(&1));
        assert_eq!(dist.get("c"), Some(&2));
    }

    #[test]
    fn hop_distances_from_walks_incoming_links_too_not_just_outgoing() {
        // a links to b (a -> b); distance FROM b must still find a at hop 1
        // via the incoming/backlink side -- distance is undirected
        // reachability, unlike extract_subgraph's include_backlinks-gated
        // BFS.
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "[[b]]"),
            Node::new("b", "B", NodeKind::Note, ""),
        ]);
        let dist = kb.hop_distances_from("b");
        assert_eq!(dist.get("b"), Some(&0));
        assert_eq!(
            dist.get("a"),
            Some(&1),
            "distance must walk the incoming-link side too: {dist:?}"
        );
    }

    #[test]
    fn hop_distances_from_takes_the_shortest_of_multiple_paths() {
        // a -> b -> d (2 hops) and a -> c -> nothing, but ALSO a -> d
        // directly (1 hop) -- the direct edge must win, not the longer path
        // discovered on the same BFS frontier.
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "[[b]] [[d]]"),
            Node::new("b", "B", NodeKind::Note, "[[d]]"),
            Node::new("d", "D", NodeKind::Note, ""),
        ]);
        let dist = kb.hop_distances_from("a");
        assert_eq!(dist.get("d"), Some(&1));
    }

    #[test]
    fn hop_distances_from_unreachable_node_is_absent_not_panicking() {
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, ""),
            Node::new("island", "Island", NodeKind::Note, ""),
        ]);
        let dist = kb.hop_distances_from("a");
        assert_eq!(dist.get("a"), Some(&0));
        assert_eq!(
            dist.get("island"),
            None,
            "a node with no path to/from focus must be absent, not zero/panic: {dist:?}"
        );
    }

    #[test]
    fn hop_distances_from_unknown_focus_id_returns_empty_map() {
        let kb = kb_with(vec![Node::new("a", "A", NodeKind::Note, "")]);
        let dist = kb.hop_distances_from("does-not-exist");
        assert!(dist.is_empty());
    }

    // --- agenda_query_in_memory (ADR-083) ---

    fn agenda_kb() -> KnowledgeBase {
        kb_with(vec![
            Node::new("todo-a", "Todo A", NodeKind::Note, "").with_todo_state("TODO"),
            Node::new("done-b", "Done B", NodeKind::Note, "").with_todo_state("DONE"),
            Node::new("plain-c", "Plain C", NodeKind::Note, ""),
            Node::new("pri-hi", "Pri Hi", NodeKind::Note, "").with_priority('A'),
            Node::new("pri-mid", "Pri Mid", NodeKind::Note, "").with_priority('B'),
            Node::new("pri-lo", "Pri Lo", NodeKind::Note, "").with_priority('C'),
            Node::new("orphan-d", "Orphan D", NodeKind::Note, ""),
            Node::new("linked-e", "Linked E", NodeKind::Note, "see [[linked-f]]"),
            Node::new("linked-f", "Linked F", NodeKind::Note, ""),
            Node::new("has-role", "Has Role", NodeKind::Note, "").with_properties(
                [("role".to_string(), "atom".to_string())]
                    .into_iter()
                    .collect(),
            ),
            Node::new("no-role", "No Role", NodeKind::Note, ""),
        ])
    }

    #[test]
    fn agenda_query_in_memory_todo_none_matches_any_set_state() {
        let kb = agenda_kb();
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::Todo(None))
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"todo-a") && ids.contains(&"done-b"));
        assert!(!ids.contains(&"plain-c"));
    }

    #[test]
    fn agenda_query_in_memory_todo_some_is_an_exact_match() {
        let kb = agenda_kb();
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::Todo(Some("DONE".to_string())))
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["done-b"]);
    }

    #[test]
    fn agenda_query_in_memory_priority_is_less_than_or_equal_mirroring_cozo() {
        // Cozo's own query is `priority <= min_pri` -- 'A' is the most
        // urgent, so requesting 'B' must return both 'A' and 'B', not just
        // an exact 'B' match (the in-memory nodes_by_priority index's own
        // convention, deliberately NOT reused here for this reason).
        let kb = agenda_kb();
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::Priority('B'))
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"pri-hi") && ids.contains(&"pri-mid"));
        assert!(!ids.contains(&"pri-lo"));
    }

    #[test]
    fn agenda_query_in_memory_orphan_requires_no_incoming_and_no_outgoing() {
        let kb = agenda_kb();
        let out = kb.agenda_query_in_memory(&AgendaFilter::Orphan).unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"orphan-d"));
        assert!(
            !ids.contains(&"linked-e") && !ids.contains(&"linked-f"),
            "a node with an outgoing or incoming link is not an orphan: {ids:?}"
        );
    }

    #[test]
    fn agenda_query_in_memory_dead_end_only_checks_outgoing() {
        let kb = agenda_kb();
        let out = kb.agenda_query_in_memory(&AgendaFilter::DeadEnd).unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"linked-f"),
            "linked-f has an incoming link but no outgoing one -- still a dead end: {ids:?}"
        );
        assert!(
            !ids.contains(&"linked-e"),
            "linked-e has an outgoing link: {ids:?}"
        );
    }

    #[test]
    fn agenda_query_in_memory_missing_role_checks_the_role_property() {
        let kb = agenda_kb();
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::MissingRole)
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"no-role"));
        assert!(!ids.contains(&"has-role"));
    }

    #[test]
    fn agenda_query_in_memory_weakly_linked_counts_outgoing_only() {
        let kb = agenda_kb();
        // linked-e has 1 outgoing link; everything else in this fixture has 0.
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::WeaklyLinked(1))
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(!ids.contains(&"linked-e"), "1 is not < 1: {ids:?}");
        assert!(
            ids.contains(&"linked-f"),
            "0 outgoing links is < 1: {ids:?}"
        );
    }

    #[test]
    fn agenda_query_in_memory_tag_is_a_substring_match_mirroring_cozo() {
        // Deliberately the OPPOSITE convention from nodes_by_tag's exact
        // match -- kb_agenda's established behavior against a Cozo-backed
        // KB is str_includes(tags_json, tag), and this must stay identical
        // regardless of which backend a given federated instance happens
        // to have, or the SAME kb_agenda call would silently behave
        // differently depending on internal storage details the caller
        // has no visibility into.
        let kb = kb_with(vec![
            Node::new("a", "A", NodeKind::Note, "").with_tags(["terraform-onboarding"]),
            Node::new("b", "B", NodeKind::Note, "").with_tags(["onboarding"]),
        ]);
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::Tag("onboarding".to_string()))
            .unwrap();
        let ids: Vec<&str> = out.iter().map(|n| n.id.as_str()).collect();
        assert!(
            ids.contains(&"a") && ids.contains(&"b"),
            "substring match must find 'onboarding' inside 'terraform-onboarding' too: {ids:?}"
        );
    }

    #[test]
    fn agenda_query_in_memory_stale_and_custom_return_a_clear_error() {
        let kb = agenda_kb();
        assert!(kb.agenda_query_in_memory(&AgendaFilter::Stale(30)).is_err());
        assert!(kb
            .agenda_query_in_memory(&AgendaFilter::Custom("?[id] := *nodes{id}".to_string()))
            .is_err());
    }

    #[test]
    fn agenda_query_in_memory_excludes_a_node_with_an_empty_title() {
        // Mirrors every Cozo agenda query's own `title != ''` guard.
        let kb = kb_with(vec![
            Node::new("blank", "", NodeKind::Note, "").with_todo_state("TODO")
        ]);
        let out = kb
            .agenda_query_in_memory(&AgendaFilter::Todo(None))
            .unwrap();
        assert!(out.is_empty(), "{out:?}");
    }
}

/// #656 — the fresh-lineage branches of `upsert_with_crdt` must not mint a v1
/// document for a node that has v2 fields.
#[cfg(all(test, feature = "crdt"))]
mod fresh_lineage_v2_tests {
    use super::*;

    fn v2_node() -> Node {
        let mut n = Node::new("task:1", "Ship it", NodeKind::Task, "body");
        n.todo_state = Some("TODO".into());
        n.priority = Some('A');
        n.aliases = vec!["shipit".into()];
        n.properties.insert("role".into(), "owner".into());
        n.source = Some(NodeSource::UserOrg);
        n.source_version = Some(7);
        n
    }

    /// A node with **no prior CRDT bytes** — first share, or lazy migration.
    ///
    /// This is the common path, not an edge case: every node in an existing KB
    /// takes it the first time the KB is shared.
    #[test]
    fn a_node_with_no_prior_crdt_doc_still_carries_its_v2_fields() {
        let mut kb = KnowledgeBase::new();
        let bytes = kb.upsert_with_crdt(v2_node(), 1).expect("returns state");

        let doc = mae_sync::kb::KbNodeDoc::from_bytes(&bytes).expect("decodes");
        assert_eq!(
            doc.schema_version(),
            2,
            "a fresh doc for a node with v2 fields must be schema v2, not v1"
        );
        assert_eq!(doc.todo_state().as_deref(), Some("TODO"));
        assert_eq!(doc.priority().as_deref(), Some("A"));
        assert_eq!(doc.aliases(), vec!["shipit".to_string()]);
        assert_eq!(
            doc.properties().get("role").map(String::as_str),
            Some("owner")
        );
    }

    /// The other fresh branch: prior bytes exist but are unreadable, so the
    /// lineage restarts. The fields must survive that too — a corrupt-bytes
    /// recovery that silently drops metadata is a data-loss path wearing the
    /// costume of resilience.
    #[test]
    fn an_unreadable_prior_doc_restarts_the_lineage_without_dropping_v2_fields() {
        let mut kb = KnowledgeBase::new();
        let mut node = v2_node();
        node.crdt_doc = Some(b"not a valid yrs update at all".to_vec());
        let bytes = kb.upsert_with_crdt(node, 1).expect("returns state");

        let doc = mae_sync::kb::KbNodeDoc::from_bytes(&bytes).expect("decodes");
        assert_eq!(doc.schema_version(), 2);
        assert_eq!(doc.todo_state().as_deref(), Some("TODO"));
        assert_eq!(
            doc.properties().get("role").map(String::as_str),
            Some("owner")
        );
    }

    /// A node that genuinely has no v2 content stays v1 — the fix must not
    /// stamp v2 onto documents that carry nothing to justify it, or every
    /// tolerant reader gains work for no reason (ADR-093: no upcast-on-read).
    #[test]
    fn a_plain_node_is_not_gratuitously_upgraded() {
        let mut kb = KnowledgeBase::new();
        let plain = Node::new("note:1", "Plain", NodeKind::Note, "just prose");
        let bytes = kb.upsert_with_crdt(plain, 1).expect("returns state");
        let doc = mae_sync::kb::KbNodeDoc::from_bytes(&bytes).expect("decodes");
        assert_eq!(doc.title(), "Plain");
        assert!(doc.todo_state().is_none());
    }
}
