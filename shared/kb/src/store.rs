//! KbStore trait — database-agnostic KB persistence interface.
//!
//! Implementation: `CozoKbStore` (v0.12.0+, CozoDB with SQLite storage).
//!
//! The in-memory `KnowledgeBase` remains the hot cache; `KbStore` is the
//! durable persistence layer that backs it.

use crate::Node;

/// A link between two KB nodes.
#[derive(Debug, Clone)]
pub struct Link {
    pub src: String,
    pub dst: String,
    pub rel_type: String,
    pub display: Option<String>,
    /// Edge weight (0.0–1.0). Default 1.0 for human-authored links.
    pub weight: f64,
    /// Confidence score (0.0–1.0). AI-generated links carry lower confidence.
    pub confidence: f64,
}

/// A search hit from FTS.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub score: f64,
}

/// A pending CRDT update queued for sync.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub rowid: i64,
    pub kb_id: String,
    pub node_id: String,
    pub update_bytes: Vec<u8>,
}

/// Subgraph result from neighborhood queries.
#[derive(Debug, Clone)]
pub struct SubGraph {
    pub nodes: Vec<(String, String)>,         // (id, title)
    pub edges: Vec<(String, String, String)>, // (src, dst, rel_type)
}

/// A member of a meta-node (ordered reference to another node).
#[derive(Debug, Clone)]
pub struct MetaMember {
    pub member_id: String,
    pub position: i32,
    /// Role: "content" (include body), "reference" (link only), "transclusion" (inline).
    pub role: String,
}

/// A paragraph-level block within a node.
#[derive(Debug, Clone)]
pub struct Block {
    pub block_idx: usize,
    pub content: String,
    /// Block type: "paragraph", "heading", "code", "list".
    pub block_type: String,
}

/// Filter for agenda queries.
#[derive(Debug, Clone)]
pub enum AgendaFilter {
    /// Nodes with a specific todo state (or any todo state if None).
    Todo(Option<String>),
    /// Nodes with priority >= the given char.
    Priority(char),
    /// Nodes with a specific tag.
    Tag(String),
    /// Nodes not updated in N days.
    Stale(u32),
    /// Nodes with no incoming or outgoing links.
    Orphan,
    /// Nodes with no outgoing links.
    DeadEnd,
    /// Nodes with no `:role:` property set (missing molecular-note classification).
    MissingRole,
    /// Nodes with fewer than N outgoing typed links.
    WeaklyLinked(u32),
    /// Raw Datalog query (CozoDB only).
    Custom(String),
}

/// A version snapshot of a node.
///
/// Each version carries a SHA-256 content checksum for tamper evidence.
/// On restore, the checksum is recomputed and verified before applying.
/// This supports SOC II audit trail requirements.
#[derive(Debug, Clone)]
pub struct NodeVersion {
    pub version: i64,
    pub title: String,
    pub body: String,
    pub tags_json: String,
    pub todo_state: String,
    pub priority: String,
    pub change_summary: String,
    pub author: String,
    pub created_at: i64,
    /// SHA-256 hex digest of `title|body|tags_json|todo_state|priority`.
    /// Used for tamper detection on restore.
    pub content_hash: String,
}

impl NodeVersion {
    /// Compute a SHA-256 content hash for this version's fields.
    ///
    /// The canonical form is `title|body|tags_json|todo_state|priority`
    /// hashed with SHA-256 and returned as a 64-char hex digest.
    /// This provides SOC II–compliant tamper evidence for audit trails.
    pub fn compute_hash(
        title: &str,
        body: &str,
        tags_json: &str,
        todo_state: &str,
        priority: &str,
    ) -> String {
        use sha2::{Digest, Sha256};
        let canonical = format!("{title}|{body}|{tags_json}|{todo_state}|{priority}");
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    /// Verify this version's content_hash matches its fields.
    pub fn verify_integrity(&self) -> bool {
        let expected = Self::compute_hash(
            &self.title,
            &self.body,
            &self.tags_json,
            &self.todo_state,
            &self.priority,
        );
        self.content_hash == expected
    }
}

/// Error when a version's content hash doesn't match (tamper detected).
#[derive(Debug)]
pub struct IntegrityError {
    pub node_id: String,
    pub version: i64,
    pub expected_hash: String,
    pub actual_hash: String,
}

impl std::fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "integrity check failed for {}@v{}: expected {}, got {}",
            self.node_id, self.version, self.expected_hash, self.actual_hash
        )
    }
}

/// A vector search result with distance score.
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub id: String,
    /// Distance from query vector (lower = more similar for cosine).
    pub distance: f64,
}

/// Classification of why a link is broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokenLinkReason {
    /// Target node was deleted or never existed.
    DeletedNode,
    /// Target ID is malformed.
    MalformedId,
}

/// A broken link with source context.
#[derive(Debug, Clone)]
pub struct BrokenLinkInfo {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    pub reason: BrokenLinkReason,
}

/// Structured KB health report (CozoDB-sourced).
#[derive(Debug, Clone)]
pub struct HealthReport {
    pub total_nodes: usize,
    pub total_links: usize,
    pub namespace_counts: std::collections::HashMap<String, usize>,
    pub by_kind: std::collections::HashMap<String, usize>,
    pub by_rel_type: std::collections::HashMap<String, usize>,
    pub orphan_ids: Vec<String>,
    pub broken_links: Vec<BrokenLinkInfo>,
    pub hub_nodes: Vec<(String, usize)>,
}

/// Error type for KbStore operations.
#[derive(Debug)]
pub enum KbStoreError {
    /// Operation not supported by this backend (e.g., graph algorithms on SQLite).
    NotSupported(String),
    /// Underlying storage error.
    Storage(String),
    /// Node not found.
    NotFound(String),
}

impl std::fmt::Display for KbStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported(msg) => write!(f, "not supported: {msg}"),
            Self::Storage(msg) => write!(f, "storage error: {msg}"),
            Self::NotFound(id) => write!(f, "node not found: {id}"),
        }
    }
}

impl std::error::Error for KbStoreError {}

/// Database-agnostic KB persistence interface.
///
/// Implementation: `CozoKbStore` (CozoDB with SQLite storage engine).
///
/// The trait is designed to be object-safe (`dyn KbStore`) so backends can
/// be swapped at runtime based on configuration.
pub trait KbStore: Send + Sync {
    // --- Node CRUD ---

    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError>;
    fn update_node(&self, node: &Node) -> Result<(), KbStoreError>;
    fn delete_node(&self, id: &str) -> Result<(), KbStoreError>;
    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError>;
    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError>;

    // --- Search ---

    fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError>;

    // --- Links ---

    fn add_link(&self, src: &str, dst: &str, display: Option<&str>) -> Result<(), KbStoreError>;
    fn remove_link(&self, src: &str, dst: &str) -> Result<(), KbStoreError>;
    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;
    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;

    // --- CRDT ---

    fn get_crdt_doc(&self, id: &str) -> Result<Option<Vec<u8>>, KbStoreError>;
    fn update_crdt_doc(&self, id: &str, doc: &[u8]) -> Result<(), KbStoreError>;

    // --- Offline queue ---

    fn push_pending_update(
        &self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> Result<(), KbStoreError>;
    fn drain_pending_updates(&self) -> Result<Vec<PendingUpdate>, KbStoreError>;
    fn ack_pending_update(&self, id: i64) -> Result<(), KbStoreError>;

    /// Count of durably-queued (un-acked) pending CRDT updates. ADR-020 observability:
    /// lets the editor answer "do I have unsynced/offline edits?" — the in-memory
    /// queue is empty when store-backed (B-16 single-source emit), so the durable
    /// queue is the source of truth. Default impl reuses the non-destructive drain;
    /// override with a `count(...)` query if it becomes hot.
    fn count_pending_updates(&self) -> Result<usize, KbStoreError> {
        Ok(self.drain_pending_updates()?.len())
    }

    // --- Bulk operations ---

    fn load_all(&self) -> Result<Vec<Node>, KbStoreError>;
    fn save_all(&self, nodes: &[&Node]) -> Result<(), KbStoreError>;

    // --- Graph queries (optional, default NotSupported) ---

    /// Add a typed relationship between two nodes.
    fn add_typed_link(
        &self,
        _src: &str,
        _dst: &str,
        _rel_type: &str,
        _weight: f64,
    ) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "typed links require CozoDB backend".into(),
        ))
    }

    /// Query links by relationship type.
    fn links_typed(&self, _id: &str, _rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "typed link queries require CozoDB backend".into(),
        ))
    }

    /// Return all known relationship type names.
    fn known_rel_types(&self) -> Result<std::collections::HashSet<String>, KbStoreError> {
        Ok(std::collections::HashSet::new())
    }

    /// Find shortest path between two nodes (BFS).
    fn shortest_path(&self, _from: &str, _to: &str) -> Result<Vec<String>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "shortest path requires CozoDB backend".into(),
        ))
    }

    /// BFS neighborhood subgraph around a node up to `depth` hops.
    fn neighborhood(&self, _id: &str, _depth: u32) -> Result<SubGraph, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "neighborhood queries require CozoDB backend".into(),
        ))
    }

    /// Execute a raw backend query (Datalog for CozoDB, SQL for SQLite).
    fn raw_query(&self, _script: &str) -> Result<(Vec<String>, Vec<Vec<String>>), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "raw queries require CozoDB backend".into(),
        ))
    }

    // --- Meta-nodes (Phase D) ---

    /// Get ordered members of a meta-node.
    fn meta_members(&self, _meta_id: &str) -> Result<Vec<MetaMember>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "meta-nodes require CozoDB backend".into(),
        ))
    }

    /// Add a member to a meta-node at the given position.
    fn add_meta_member(
        &self,
        _meta_id: &str,
        _member_id: &str,
        _position: i32,
        _role: &str,
    ) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "meta-nodes require CozoDB backend".into(),
        ))
    }

    /// Remove a member from a meta-node.
    fn remove_meta_member(&self, _meta_id: &str, _member_id: &str) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "meta-nodes require CozoDB backend".into(),
        ))
    }

    /// Recompose a meta-node's body from its members.
    fn compose_meta_body(&self, _meta_id: &str) -> Result<String, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "meta-nodes require CozoDB backend".into(),
        ))
    }

    // --- Block addressing (Phase D) ---

    /// Get all blocks for a node (paragraph-level sub-nodes).
    fn get_blocks(&self, _parent_id: &str) -> Result<Vec<Block>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "blocks require CozoDB backend".into(),
        ))
    }

    /// Set blocks for a node (replaces existing).
    fn set_blocks(&self, _parent_id: &str, _blocks: &[Block]) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "blocks require CozoDB backend".into(),
        ))
    }

    /// Get a single block by parent ID and index.
    fn get_block(&self, _parent_id: &str, _idx: usize) -> Result<Option<Block>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "blocks require CozoDB backend".into(),
        ))
    }

    // --- Agenda queries (Phase E) ---

    /// Run an agenda query with the given filter.
    fn agenda_query(&self, _filter: &AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "agenda queries require CozoDB backend".into(),
        ))
    }

    // --- Node versioning (Phase H) ---

    /// Get version history for a node.
    fn node_history(&self, _id: &str, _limit: usize) -> Result<Vec<NodeVersion>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "versioning requires CozoDB backend".into(),
        ))
    }

    /// Get a node at a specific version.
    fn node_at_version(&self, _id: &str, _version: i64) -> Result<Option<Node>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "versioning requires CozoDB backend".into(),
        ))
    }

    /// Restore a node to a previous version.
    fn restore_version(&self, _id: &str, _version: i64) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "versioning requires CozoDB backend".into(),
        ))
    }

    // --- Embeddings / Vector search (Phase G) ---

    /// Store an embedding vector for a node.
    fn store_embedding(&self, _id: &str, _model: &str, _vec: &[f32]) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "embeddings require CozoDB backend".into(),
        ))
    }

    /// Search for nearest neighbors by vector similarity (HNSW).
    fn vector_search(&self, _vec: &[f32], _k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "vector search requires CozoDB backend".into(),
        ))
    }

    /// GraphRAG query: vector-nearest neighbors expanded by 1 hop of graph links.
    /// Returns node IDs with scores (vector hits get their distance score,
    /// graph-expanded nodes get score 0.0).
    fn graphrag_search(&self, _vec: &[f32], _k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "GraphRAG requires CozoDB backend".into(),
        ))
    }

    // --- Health ---

    /// Compute a structured health report from the backing store.
    fn health_report(&self) -> Result<HealthReport, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "health report requires CozoDB backend".into(),
        ))
    }

    /// Return (id, title) pairs for all nodes, optionally filtered by prefix.
    fn id_title_pairs(&self, _prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "id_title_pairs requires CozoDB backend".into(),
        ))
    }

    // --- Lifecycle ---

    fn backend_name(&self) -> &str;
    fn db_path(&self) -> &std::path::Path;
}
