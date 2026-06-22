//! KbNodeDoc: yrs-backed KB node with YMap schema.
//!
//! All yrs Doc instances use UTF-16 offset kind (via `text::new_doc()`) for
//! consistency with the Yjs standard. See the CRDT UTF-16 fix (92a20b8).

use sha2::{Digest, Sha256};
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, MapRef, Out, ReadTxn, Text, TextPrelim, Transact,
};

use crate::text::{new_doc, new_doc_with_client_id};
use crate::SyncError;

const ID_KEY: &str = "id";
const TITLE_KEY: &str = "title";
const BODY_KEY: &str = "body";
const TAGS_KEY: &str = "tags";
const LINKS_KEY: &str = "links";
const META_KEY: &str = "meta";

/// True if `candidate_sv` carries operations not covered by `base_sv` — i.e.
/// some client's clock in `candidate_sv` exceeds what `base_sv` has seen.
///
/// Both args are v1-encoded yrs state vectors. This is the format-independent
/// primitive behind ADR-022 reconcile decisions: "is the remote ahead of me?"
/// (`sv_has_ops_beyond(remote_sv, my_sv)`) and "am I ahead of the remote?"
/// (`sv_has_ops_beyond(my_sv, remote_sv)`). Avoids the trap that an `encode_diff`
/// against a fully-covering SV is still a non-empty (`[0,0]`) byte sequence.
pub fn sv_has_ops_beyond(candidate_sv: &[u8], base_sv: &[u8]) -> Result<bool, SyncError> {
    let candidate = yrs::StateVector::decode_v1(candidate_sv)
        .map_err(|e| SyncError::Encoding(e.to_string()))?;
    let base =
        yrs::StateVector::decode_v1(base_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, &clock) in candidate.iter() {
        if base.get(client) < clock {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True if two v1-encoded state vectors share **no** client id — i.e. the two
/// documents were constructed on entirely independent lineages.
///
/// This is the order-independent ADR-022 divergence signal: any healthy collab
/// pair shares at least the owner's lineage client (the joiner adopted it on
/// first join), so a *disjoint* client set means the nodes were built from
/// scratch with the same id but incompatible lineages (the B-14 condition) —
/// regardless of which side happens to win the YMap last-writer-wins.
pub fn sv_clients_disjoint(a_sv: &[u8], b_sv: &[u8]) -> Result<bool, SyncError> {
    let a = yrs::StateVector::decode_v1(a_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    let b = yrs::StateVector::decode_v1(b_sv).map_err(|e| SyncError::Encoding(e.to_string()))?;
    for (client, _) in a.iter() {
        if b.contains_client(client) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Materialized content from a KbNodeDoc — all fields extracted for FTS rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedNode {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
}

/// A KB node represented as a yrs document.
///
/// Schema:
/// - Root YMap "node" contains: id (String), title (YText), body (YText),
///   tags (YArray<String>), links (YArray<String>), meta (YMap<String, String>)
///
/// All Doc instances use UTF-16 offset kind for cross-client consistency.
pub struct KbNodeDoc {
    doc: Doc,
}

impl KbNodeDoc {
    /// Create a new KB node document with UTF-16 offset kind.
    pub fn new(id: &str, title: &str, body: &str, tags: &[String]) -> Self {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_map("node");
            let mut txn = doc.transact_mut();

            root.insert(&mut txn, ID_KEY, id);
            root.insert(&mut txn, TITLE_KEY, TextPrelim::new(title));
            root.insert(&mut txn, BODY_KEY, TextPrelim::new(body));

            let tags_arr = root.insert(&mut txn, TAGS_KEY, ArrayPrelim::default());
            for tag in tags {
                tags_arr.push_back(&mut txn, tag.as_str());
            }

            root.insert(&mut txn, LINKS_KEY, ArrayPrelim::default());
            root.insert(&mut txn, META_KEY, MapPrelim::default());
        }
        Self { doc }
    }

    /// Create a new KB node document with a specific client ID for collaborative use.
    pub fn new_with_client_id(
        id: &str,
        title: &str,
        body: &str,
        tags: &[String],
        client_id: u64,
    ) -> Self {
        let doc = new_doc_with_client_id(client_id);
        {
            let root = doc.get_or_insert_map("node");
            let mut txn = doc.transact_mut();

            root.insert(&mut txn, ID_KEY, id);
            root.insert(&mut txn, TITLE_KEY, TextPrelim::new(title));
            root.insert(&mut txn, BODY_KEY, TextPrelim::new(body));

            let tags_arr = root.insert(&mut txn, TAGS_KEY, ArrayPrelim::default());
            for tag in tags {
                tags_arr.push_back(&mut txn, tag.as_str());
            }

            root.insert(&mut txn, LINKS_KEY, ArrayPrelim::default());
            root.insert(&mut txn, META_KEY, MapPrelim::default());
        }
        Self { doc }
    }

    /// Load from encoded bytes with UTF-16 offset kind.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SyncError> {
        let doc = new_doc();
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Load from encoded bytes with a specific client ID for joining a collaborative KB.
    pub fn from_bytes_with_client_id(bytes: &[u8], client_id: u64) -> Result<Self, SyncError> {
        let doc = new_doc_with_client_id(client_id);
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Encode full state for persistence.
    pub fn encode(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// Alias for `encode()` — naming consistency with TextSync.
    pub fn encode_state(&self) -> Vec<u8> {
        self.encode()
    }

    /// Compute an incremental diff against a remote state vector.
    ///
    /// Returns only the updates the remote doesn't have yet. More efficient
    /// than sending the full state when the remote is only slightly behind.
    pub fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, SyncError> {
        let sv = yrs::StateVector::decode_v1(remote_sv)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        let txn = self.doc.transact();
        Ok(txn.encode_state_as_update_v1(&sv))
    }

    /// Get the node ID.
    pub fn id(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        root.get(&txn, ID_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Get title.
    pub fn title(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, TITLE_KEY) {
            Some(Out::YText(text)) => text.get_string(&txn),
            _ => String::new(),
        }
    }

    /// Set title. Returns encoded update.
    pub fn set_title(&mut self, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, TITLE_KEY) {
            let len = text.get_string(&txn).encode_utf16().count() as u32;
            if len > 0 {
                text.remove_range(&mut txn, 0, len);
            }
            text.insert(&mut txn, 0, title);
        }
        txn.encode_update_v1()
    }

    /// Get body.
    pub fn body(&self) -> String {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, BODY_KEY) {
            Some(Out::YText(text)) => text.get_string(&txn),
            _ => String::new(),
        }
    }

    /// Set body. Returns encoded update.
    pub fn set_body(&mut self, body: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YText(text)) = root.get(&txn, BODY_KEY) {
            let len = text.get_string(&txn).encode_utf16().count() as u32;
            if len > 0 {
                text.remove_range(&mut txn, 0, len);
            }
            text.insert(&mut txn, 0, body);
        }
        txn.encode_update_v1()
    }

    /// Get tags.
    pub fn tags(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, TAGS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Add a tag. Returns encoded update.
    pub fn add_tag(&mut self, tag: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            arr.push_back(&mut txn, tag);
        }
        txn.encode_update_v1()
    }

    /// Remove a tag by value. Returns encoded update.
    pub fn remove_tag(&mut self, tag: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, TAGS_KEY) {
            let idx = arr.iter(&txn).position(|v| v.to_string(&txn) == tag);
            if let Some(idx) = idx {
                arr.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// Get links.
    pub fn links(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, LINKS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Add a link. Returns encoded update.
    pub fn add_link(&mut self, target: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, LINKS_KEY) {
            arr.push_back(&mut txn, target);
        }
        txn.encode_update_v1()
    }

    /// Remove a link by target. Returns encoded update.
    pub fn remove_link(&mut self, target: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(arr)) = root.get(&txn, LINKS_KEY) {
            let idx = arr.iter(&txn).position(|v| v.to_string(&txn) == target);
            if let Some(idx) = idx {
                arr.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// Set a metadata key-value pair. Returns encoded update.
    pub fn set_meta(&mut self, key: &str, value: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map("node");
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(meta)) = root.get(&txn, META_KEY) {
            meta.insert(&mut txn, key, value);
        }
        txn.encode_update_v1()
    }

    /// Get a metadata value by key.
    pub fn get_meta(&self, key: &str) -> Option<String> {
        let root = self.doc.get_or_insert_map("node");
        let txn = self.doc.transact();
        match root.get(&txn, META_KEY) {
            Some(Out::YMap(meta)) => meta.get(&txn, key).map(|v| v.to_string(&txn)),
            _ => None,
        }
    }

    /// Apply a remote update. Returns whether content actually changed
    /// (detected via SHA-256 hash comparison, since yrs state vectors are
    /// monotonically increasing even for undo operations).
    pub fn apply_update(&mut self, update: &[u8]) -> Result<bool, SyncError> {
        let hash_before = self.content_hash();
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        drop(txn);
        let hash_after = self.content_hash();
        Ok(hash_before != hash_after)
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// True if this document holds operations not yet covered by `remote_sv`
    /// — i.e. `encode_diff(remote_sv)` would carry real (non-no-op) content.
    ///
    /// Format-independent: a yrs v1 update against a fully-covering state vector
    /// still encodes to a small non-empty byte sequence (`[0, 0]`), so checking
    /// `encode_diff(..).is_empty()` is wrong. We instead compare state vectors
    /// per client: we are "ahead" iff some client's local clock exceeds what the
    /// remote has seen. Used by ADR-022 reconcile to decide whether a local-ahead
    /// push is actually needed.
    pub fn has_ops_beyond(&self, remote_sv: &[u8]) -> Result<bool, SyncError> {
        sv_has_ops_beyond(&self.state_vector(), remote_sv)
    }

    /// Extract all fields into a `MaterializedNode` for FTS5 rebuild.
    pub fn materialize(&self) -> MaterializedNode {
        MaterializedNode {
            id: self.id(),
            title: self.title(),
            body: self.body(),
            tags: self.tags(),
            links: self.links(),
        }
    }

    /// SHA-256 content hash for change detection.
    ///
    /// Covers title + body + tags (not links/meta, which are structural).
    /// Used to detect actual content changes since yrs state vectors grow
    /// monotonically even on undo.
    pub fn content_hash(&self) -> String {
        let mat = self.materialize();
        let mut hasher = Sha256::new();
        hasher.update(mat.title.as_bytes());
        hasher.update(b"\0");
        hasher.update(mat.body.as_bytes());
        hasher.update(b"\0");
        for tag in &mat.tags {
            hasher.update(tag.as_bytes());
            hasher.update(b"\0");
        }
        format!("{:x}", hasher.finalize())
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

// --- KbCollectionDoc: manifest of nodes in a shared KB ---

const COLLECTION_MAP: &str = "collection";
const COLL_NAME_KEY: &str = "name";
const COLL_CREATOR_KEY: &str = "creator"; // legacy display label (never authoritative)
const COLL_NODES_KEY: &str = "nodes";
const COLL_MEMBERS_KEY: &str = "members"; // legacy YArray<label>, read-only after ADR-018
                                          // ADR-018 identity-anchored schema (v2):
const COLL_SCHEMA_KEY: &str = "schema";
const COLL_OWNER_KEY: &str = "owner"; // owner principal (key fingerprint)
const COLL_MEMBER_ROLES_KEY: &str = "member_roles"; // YMap<fingerprint -> {role,label}>
const COLL_POLICY_KEY: &str = "join_policy"; // restrictive|invite|permissive
const COLL_PENDING_KEY: &str = "pending"; // YMap<fingerprint -> {label,requested_at}>
const MEMBER_ROLE_KEY: &str = "role";
const MEMBER_LABEL_KEY: &str = "label";
const PENDING_AT_KEY: &str = "requested_at";

/// Collection schema version. v2 = ADR-018 (principal-anchored owner/roles/policy).
pub const SCHEMA_VERSION: u32 = 2;

/// A KB role. Hierarchical RBAC (NIST INCITS 359): `owner ⊇ editor ⊇ viewer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Editor => "editor",
            Role::Viewer => "viewer",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "editor" => Some(Role::Editor),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    fn rank(self) -> u8 {
        match self {
            Role::Viewer => 0,
            Role::Editor => 1,
            Role::Owner => 2,
        }
    }
    /// Role inheritance: a senior role holds all junior permissions.
    pub fn includes(self, other: Role) -> bool {
        self.rank() >= other.rank()
    }
}

/// Per-KB join policy (maps to Drive sharing modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JoinPolicy {
    /// Default-deny: only owner + explicitly added members.
    Restrictive,
    /// Non-members' joins become pending → owner approves (default).
    #[default]
    Invite,
    /// Any authenticated peer auto-joins as viewer.
    Permissive,
}

impl JoinPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            JoinPolicy::Restrictive => "restrictive",
            JoinPolicy::Invite => "invite",
            JoinPolicy::Permissive => "permissive",
        }
    }
    pub fn parse(s: &str) -> Option<JoinPolicy> {
        match s {
            "restrictive" => Some(JoinPolicy::Restrictive),
            "invite" => Some(JoinPolicy::Invite),
            "permissive" => Some(JoinPolicy::Permissive),
            _ => None,
        }
    }
}

/// A KB member: the principal (key fingerprint), its role, and a display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub fingerprint: String,
    pub role: Role,
    pub label: String,
}

/// A pending join request (invite policy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRequest {
    pub fingerprint: String,
    pub label: String,
    pub requested_at: String,
}

/// A KB collection manifest represented as a yrs document.
///
/// Schema:
/// - Root YMap "collection" contains:
///   - name (String): KB display name
///   - creator (String): creator's user name
///   - nodes (YMap<node_id -> String(title)>): node manifest
///   - members (YArray<String>): member user names
///
/// The collection doc is stored on the server as `kbc:{kb_id}`.
pub struct KbCollectionDoc {
    doc: Doc,
}

impl KbCollectionDoc {
    /// Create a new collection document.
    pub fn new(name: &str, creator: &str) -> Self {
        let doc = new_doc();
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Create a new collection document with a specific client ID.
    pub fn new_with_client_id(name: &str, creator: &str, client_id: u64) -> Self {
        let doc = new_doc_with_client_id(client_id);
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_CREATOR_KEY, creator);
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            let members = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            members.push_back(&mut txn, creator);
        }
        Self { doc }
    }

    /// Load from encoded bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SyncError> {
        let doc = new_doc();
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Encode full state.
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Apply a remote update.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        Ok(())
    }

    /// Get KB name.
    pub fn name(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_NAME_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Get creator name.
    pub fn creator(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_CREATOR_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Add a node to the collection manifest. Returns encoded update.
    pub fn add_node(&mut self, node_id: &str, title: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.insert(&mut txn, node_id, title);
        }
        txn.encode_update_v1()
    }

    /// Remove a node from the collection manifest. Returns encoded update.
    pub fn remove_node(&mut self, node_id: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(nodes)) = root.get(&txn, COLL_NODES_KEY) {
            nodes.remove(&mut txn, node_id);
        }
        txn.encode_update_v1()
    }

    /// List all nodes in the collection: (node_id, title) pairs.
    pub fn list_nodes(&self) -> Vec<(String, String)> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes
                .iter(&txn)
                .map(|(k, v)| (k.to_string(), v.to_string(&txn)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Get number of nodes in the collection.
    pub fn node_count(&self) -> u32 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_NODES_KEY) {
            Some(Out::YMap(nodes)) => nodes.len(&txn),
            _ => 0,
        }
    }

    /// Re-stamp the authoritative creator and ensure they are a member.
    /// Used by the daemon to bind a shared collection to the AUTHENTICATED peer
    /// identity (ADR-017 strict binding), overriding the client-supplied creator.
    /// Returns the encoded update.
    pub fn set_creator(&mut self, creator: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_CREATOR_KEY, creator);
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == creator);
            if !already {
                members.push_back(&mut txn, creator);
            }
        }
        txn.encode_update_v1()
    }

    /// Add a member to the collection. Returns encoded update.
    pub fn add_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            // Check for duplicates
            let already = members.iter(&txn).any(|v| v.to_string(&txn) == user_name);
            if !already {
                members.push_back(&mut txn, user_name);
            }
        }
        txn.encode_update_v1()
    }

    /// Remove a member from the collection. Returns encoded update.
    pub fn remove_member(&mut self, user_name: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YArray(members)) = root.get(&txn, COLL_MEMBERS_KEY) {
            let idx = members
                .iter(&txn)
                .position(|v| v.to_string(&txn) == user_name);
            if let Some(idx) = idx {
                members.remove(&mut txn, idx as u32);
            }
        }
        txn.encode_update_v1()
    }

    /// List all members.
    pub fn members(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_MEMBERS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    // --- ADR-018: identity-anchored owner / roles / join-policy / pending ---

    /// Create a v2 collection owned by `owner_principal` (a key fingerprint), with
    /// `owner_label` for display. Seeds schema=2, owner, the owner member entry
    /// (role=owner), join_policy=invite, an empty pending map, and legacy
    /// `creator`/`members` for back-compat reads. An empty owner principal is
    /// tolerated (the daemon stamps the real owner from the verified cert).
    pub fn new_owned(name: &str, owner_principal: &str, owner_label: &str) -> Self {
        Self::new_owned_with(
            name,
            owner_principal,
            owner_label,
            None,
            JoinPolicy::default(),
        )
    }

    /// Like `new_owned` but with an explicit client id and join policy.
    pub fn new_owned_with(
        name: &str,
        owner_principal: &str,
        owner_label: &str,
        client_id: Option<u64>,
        policy: JoinPolicy,
    ) -> Self {
        let doc = match client_id {
            Some(id) => new_doc_with_client_id(id),
            None => new_doc(),
        };
        {
            let root = doc.get_or_insert_map(COLLECTION_MAP);
            let mut txn = doc.transact_mut();
            root.insert(&mut txn, COLL_NAME_KEY, name);
            root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
            root.insert(&mut txn, COLL_OWNER_KEY, owner_principal);
            root.insert(&mut txn, COLL_CREATOR_KEY, owner_label); // legacy display
            root.insert(&mut txn, COLL_NODES_KEY, MapPrelim::default());
            root.insert(&mut txn, COLL_POLICY_KEY, policy.as_str());
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
            let m = root.insert(&mut txn, COLL_MEMBER_ROLES_KEY, MapPrelim::default());
            if !owner_principal.is_empty() {
                let entry = m.insert(&mut txn, owner_principal, MapPrelim::default());
                entry.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
                entry.insert(&mut txn, MEMBER_LABEL_KEY, owner_label);
            }
            // legacy members array (read-only after migration)
            let legacy = root.insert(&mut txn, COLL_MEMBERS_KEY, ArrayPrelim::default());
            legacy.push_back(&mut txn, owner_label);
        }
        Self { doc }
    }

    /// Schema version (0 = legacy v1, absent the schema key).
    pub fn schema_version(&self) -> u32 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_SCHEMA_KEY)
            .map(|v| v.to_string(&txn).parse::<u32>().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Owner principal (key fingerprint). Empty if unset.
    pub fn owner(&self) -> String {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_OWNER_KEY)
            .map(|v| v.to_string(&txn))
            .unwrap_or_default()
    }

    /// Owner display label (legacy `creator` field).
    pub fn owner_label(&self) -> String {
        self.creator()
    }

    /// The role of `principal` (key fingerprint), if it is a member.
    pub fn role_of(&self, principal: &str) -> Option<Role> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                return entry
                    .get(&txn, MEMBER_ROLE_KEY)
                    .map(|r| r.to_string(&txn))
                    .and_then(|s| Role::parse(&s));
            }
        }
        None
    }

    /// All members with their roles (the ReBAC tuple set for this KB).
    pub fn member_roles(&self) -> Vec<Member> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let mut out = Vec::new();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            for (fp, v) in m.iter(&txn) {
                if let Out::YMap(entry) = v {
                    let role = entry
                        .get(&txn, MEMBER_ROLE_KEY)
                        .map(|r| r.to_string(&txn))
                        .and_then(|s| Role::parse(&s))
                        .unwrap_or(Role::Viewer);
                    let label = entry
                        .get(&txn, MEMBER_LABEL_KEY)
                        .map(|l| l.to_string(&txn))
                        .unwrap_or_default();
                    out.push(Member {
                        fingerprint: fp.to_string(),
                        role,
                        label,
                    });
                }
            }
        }
        out
    }

    /// The KB join policy (default invite).
    pub fn join_policy(&self) -> JoinPolicy {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_POLICY_KEY)
            .map(|v| v.to_string(&txn))
            .and_then(|s| JoinPolicy::parse(&s))
            .unwrap_or_default()
    }

    /// Pending join requests (invite policy).
    pub fn pending(&self) -> Vec<PendingRequest> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        let mut out = Vec::new();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            for (fp, v) in p.iter(&txn) {
                if let Out::YMap(req) = v {
                    let label = req
                        .get(&txn, MEMBER_LABEL_KEY)
                        .map(|l| l.to_string(&txn))
                        .unwrap_or_default();
                    let requested_at = req
                        .get(&txn, PENDING_AT_KEY)
                        .map(|t| t.to_string(&txn))
                        .unwrap_or_default();
                    out.push(PendingRequest {
                        fingerprint: fp.to_string(),
                        label,
                        requested_at,
                    });
                }
            }
        }
        out
    }

    /// Helper: get-or-create the `member_roles` YMap within an open txn.
    fn member_roles_map(root: &MapRef, txn: &mut yrs::TransactionMut) -> MapRef {
        match root.get(txn, COLL_MEMBER_ROLES_KEY) {
            Some(Out::YMap(m)) => m,
            _ => root.insert(txn, COLL_MEMBER_ROLES_KEY, MapPrelim::default()),
        }
    }

    /// Bind the authoritative owner = `principal` (key fingerprint), display
    /// `label`. Idempotent; ensures schema=2 + default policy + the owner member
    /// entry. The daemon calls this on kb/share to bind the verified cert identity.
    pub fn set_owner(&mut self, principal: &str, label: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_OWNER_KEY, principal);
        root.insert(&mut txn, COLL_CREATOR_KEY, label);
        if root.get(&txn, COLL_SCHEMA_KEY).is_none() {
            root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
        }
        if root.get(&txn, COLL_POLICY_KEY).is_none() {
            root.insert(&mut txn, COLL_POLICY_KEY, JoinPolicy::default().as_str());
        }
        if root.get(&txn, COLL_PENDING_KEY).is_none() {
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
        }
        let m = Self::member_roles_map(&root, &mut txn);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label);
        txn.encode_update_v1()
    }

    /// Insert or update a member's role (keyed by principal; CRDT-safe LWW).
    pub fn upsert_member(&mut self, principal: &str, label: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let m = Self::member_roles_map(&root, &mut txn);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label);
        txn.encode_update_v1()
    }

    /// Update only the role of an existing member (no-op if absent).
    pub fn set_role(&mut self, principal: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            if let Some(Out::YMap(entry)) = m.get(&txn, principal) {
                entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
            }
        }
        txn.encode_update_v1()
    }

    /// Remove a member by principal.
    pub fn remove_principal(&mut self, principal: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(m)) = root.get(&txn, COLL_MEMBER_ROLES_KEY) {
            m.remove(&mut txn, principal);
        }
        txn.encode_update_v1()
    }

    /// Set the KB join policy.
    pub fn set_join_policy(&mut self, policy: JoinPolicy) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_POLICY_KEY, policy.as_str());
        txn.encode_update_v1()
    }

    /// Record a pending join request (idempotent re-request).
    pub fn add_pending(&mut self, principal: &str, label: &str, requested_at: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let p = match root.get(&txn, COLL_PENDING_KEY) {
            Some(Out::YMap(p)) => p,
            _ => root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default()),
        };
        let req = p.insert(&mut txn, principal, MapPrelim::default());
        req.insert(&mut txn, MEMBER_LABEL_KEY, label);
        req.insert(&mut txn, PENDING_AT_KEY, requested_at);
        txn.encode_update_v1()
    }

    /// Remove a pending request.
    pub fn remove_pending(&mut self, principal: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            p.remove(&mut txn, principal);
        }
        txn.encode_update_v1()
    }

    /// Approve a pending principal as `role` — removes pending + adds the member
    /// in a SINGLE transaction (atomic, no transient half-state on peers).
    pub fn approve(&mut self, principal: &str, role: Role) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        let mut label = String::new();
        if let Some(Out::YMap(p)) = root.get(&txn, COLL_PENDING_KEY) {
            if let Some(Out::YMap(req)) = p.get(&txn, principal) {
                label = req
                    .get(&txn, MEMBER_LABEL_KEY)
                    .map(|l| l.to_string(&txn))
                    .unwrap_or_default();
            }
            p.remove(&mut txn, principal);
        }
        let m = Self::member_roles_map(&root, &mut txn);
        let entry = m.insert(&mut txn, principal, MapPrelim::default());
        entry.insert(&mut txn, MEMBER_ROLE_KEY, role.as_str());
        entry.insert(&mut txn, MEMBER_LABEL_KEY, label.as_str());
        txn.encode_update_v1()
    }

    /// Legacy v1 members (the read-only `members` YArray of labels), for migration.
    pub fn legacy_members(&self) -> Vec<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        match root.get(&txn, COLL_MEMBERS_KEY) {
            Some(Out::YArray(arr)) => arr.iter(&txn).map(|v| v.to_string(&txn)).collect(),
            _ => Vec::new(),
        }
    }

    /// Migrate a legacy v1 collection (label `creator` + `members` YArray) to the
    /// v2 identity-anchored schema. Idempotent: returns `None` if already v2.
    ///
    /// `resolver(label) -> Some((fingerprint, label))` maps a legacy label to its
    /// key principal (e.g. via the daemon's authorized_keys). A label that doesn't
    /// resolve becomes a transitional `legacy:<label>` principal — preserved for
    /// audit, but a real key peer won't match it, so the owner should re-add it by
    /// fingerprint (or simply re-share, which `set_owner` re-binds). The legacy
    /// `members` YArray is left intact (read-only); v2 data lives under new keys.
    pub fn migrate_if_legacy<F>(&mut self, resolver: F) -> Option<Vec<u8>>
    where
        F: Fn(&str) -> Option<(String, String)>,
    {
        if self.schema_version() >= 2 {
            return None;
        }
        let creator_label = self.creator();
        let legacy = self.legacy_members();
        // Resolve a label → (principal, label); fall back to legacy:<label>.
        let resolve = |label: &str| -> (String, String) {
            resolver(label).unwrap_or_else(|| (format!("legacy:{label}"), label.to_string()))
        };
        let (owner_principal, owner_label) = resolve(&creator_label);
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_SCHEMA_KEY, SCHEMA_VERSION as i64);
        root.insert(&mut txn, COLL_OWNER_KEY, owner_principal.as_str());
        if root.get(&txn, COLL_POLICY_KEY).is_none() {
            root.insert(&mut txn, COLL_POLICY_KEY, JoinPolicy::default().as_str());
        }
        if root.get(&txn, COLL_PENDING_KEY).is_none() {
            root.insert(&mut txn, COLL_PENDING_KEY, MapPrelim::default());
        }
        let m = Self::member_roles_map(&root, &mut txn);
        // Owner entry first.
        {
            let e = m.insert(&mut txn, owner_principal.as_str(), MapPrelim::default());
            e.insert(&mut txn, MEMBER_ROLE_KEY, Role::Owner.as_str());
            e.insert(&mut txn, MEMBER_LABEL_KEY, owner_label.as_str());
        }
        for label in legacy {
            let (principal, disp) = resolve(&label);
            if principal == owner_principal {
                continue; // already the owner
            }
            let e = m.insert(&mut txn, principal.as_str(), MapPrelim::default());
            e.insert(&mut txn, MEMBER_ROLE_KEY, Role::Editor.as_str());
            e.insert(&mut txn, MEMBER_LABEL_KEY, disp.as_str());
        }
        Some(txn.encode_update_v1())
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- KbNodeDoc tests ---

    #[test]
    fn new_node_schema() {
        let node = KbNodeDoc::new(
            "concept:test",
            "Test Node",
            "Some body text",
            &["tag1".to_string(), "tag2".to_string()],
        );
        assert_eq!(node.id(), "concept:test");
        assert_eq!(node.title(), "Test Node");
        assert_eq!(node.body(), "Some body text");
        assert_eq!(node.tags(), vec!["tag1", "tag2"]);
        assert!(node.links().is_empty());
    }

    #[test]
    fn set_title_generates_update() {
        let mut node = KbNodeDoc::new("n1", "Old Title", "", &[]);
        let update = node.set_title("New Title");
        assert!(!update.is_empty());
        assert_eq!(node.title(), "New Title");
    }

    #[test]
    fn set_body_generates_update() {
        let mut node = KbNodeDoc::new("n1", "T", "old body", &[]);
        let update = node.set_body("new body content");
        assert!(!update.is_empty());
        assert_eq!(node.body(), "new body content");
    }

    #[test]
    fn tag_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &["a".to_string()]);
        assert_eq!(node.tags(), vec!["a"]);

        node.add_tag("b");
        assert_eq!(node.tags(), vec!["a", "b"]);

        node.remove_tag("a");
        assert_eq!(node.tags(), vec!["b"]);
    }

    #[test]
    fn two_clients_merge_body() {
        let mut node_a = KbNodeDoc::new("n1", "T", "hello", &[]);
        let state = node_a.encode();

        let mut node_b = KbNodeDoc::from_bytes(&state).unwrap();
        assert_eq!(node_b.body(), "hello");

        // Both edit body (set_body replaces, so last-write-wins semantics)
        let update_a = node_a.set_body("from A");
        let update_b = node_b.set_body("from B");

        node_a.apply_update(&update_b).unwrap();
        node_b.apply_update(&update_a).unwrap();

        // Both converge to the same result
        assert_eq!(node_a.body(), node_b.body());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let node = KbNodeDoc::new(
            "concept:arch",
            "Architecture",
            "The system uses...",
            &["core".to_string(), "design".to_string()],
        );
        let bytes = node.encode();

        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.id(), "concept:arch");
        assert_eq!(restored.title(), "Architecture");
        assert_eq!(restored.body(), "The system uses...");
        assert_eq!(restored.tags(), vec!["core", "design"]);
    }

    // --- UTF-16 offset tests ---

    #[test]
    fn utf16_offset_cjk_roundtrip() {
        let node = KbNodeDoc::new("n1", "CJK", "", &[]);
        // CJK characters are multi-byte in UTF-8 but single code unit in UTF-16 (BMP)
        let mut n = KbNodeDoc::from_bytes(&node.encode()).unwrap();
        n.set_body("Hello 世界 and more text after");
        let bytes = n.encode();
        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.body(), "Hello 世界 and more text after");
    }

    #[test]
    fn utf16_offset_emoji_roundtrip() {
        // Emoji above BMP (U+1F600) are 2 UTF-16 code units (surrogate pairs)
        let mut node = KbNodeDoc::new("n1", "Emoji Test 😀", "Body with 🎉 emoji", &[]);
        node.set_title("Updated 🌍 title");
        let bytes = node.encode();
        let restored = KbNodeDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.title(), "Updated 🌍 title");
        assert_eq!(restored.body(), "Body with 🎉 emoji");
    }

    #[test]
    fn utf16_two_client_cjk_merge() {
        let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "你好", &[], 1);
        let state = node_a.encode();
        let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let update_a = node_a.set_body("你好世界");
        let update_b = node_b.set_body("你好朋友");

        node_a.apply_update(&update_b).unwrap();
        node_b.apply_update(&update_a).unwrap();

        assert_eq!(node_a.body(), node_b.body());
    }

    // --- Client ID tests ---

    #[test]
    fn new_with_client_id_preserves_identity() {
        let node = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 42);
        assert_eq!(node.id(), "n1");
        assert_eq!(node.title(), "T");
        // Verify client_id is set on the yrs Doc
        assert_eq!(node.doc().client_id().get(), 42);
    }

    #[test]
    fn from_bytes_with_client_id_preserves_identity() {
        let original = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 10);
        let bytes = original.encode();
        let restored = KbNodeDoc::from_bytes_with_client_id(&bytes, 20).unwrap();
        assert_eq!(restored.id(), "n1");
        assert_eq!(restored.doc().client_id().get(), 20);
    }

    // --- encode_diff tests ---

    #[test]
    fn encode_diff_produces_valid_update() {
        let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
        let sv_before = node.state_vector();
        node.set_body("hello world");
        let diff = node.encode_diff(&sv_before).unwrap();
        assert!(!diff.is_empty());

        // Apply the diff to a copy from before the change
        let mut old = KbNodeDoc::from_bytes(&{
            let orig = KbNodeDoc::new("n1", "T", "hello", &[]);
            orig.encode()
        })
        .unwrap();
        old.apply_update(&diff).unwrap();
        // After applying diff, old should have "hello world"
        // (The diff contains the set_body which replaces the entire text)
        assert!(old.body().contains("hello"));
    }

    // --- materialize tests ---

    #[test]
    fn materialize_extracts_all_fields() {
        let mut node = KbNodeDoc::new(
            "concept:test",
            "Test",
            "Body",
            &["tag1".to_string(), "tag2".to_string()],
        );
        node.add_link("concept:other");
        let mat = node.materialize();
        assert_eq!(mat.id, "concept:test");
        assert_eq!(mat.title, "Test");
        assert_eq!(mat.body, "Body");
        assert_eq!(mat.tags, vec!["tag1", "tag2"]);
        assert_eq!(mat.links, vec!["concept:other"]);
    }

    // --- content_hash tests ---

    #[test]
    fn content_hash_changes_on_edit() {
        let mut node = KbNodeDoc::new("n1", "T", "hello", &[]);
        let hash1 = node.content_hash();
        node.set_body("world");
        let hash2 = node.content_hash();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_stable_for_same_content() {
        let node1 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
        let node2 = KbNodeDoc::new("n1", "T", "hello", &["a".to_string()]);
        assert_eq!(node1.content_hash(), node2.content_hash());
    }

    // --- apply_update returns changed flag ---

    #[test]
    fn apply_update_returns_changed_flag() {
        let mut node_a = KbNodeDoc::new_with_client_id("n1", "T", "hello", &[], 1);
        let state = node_a.encode();
        let mut node_b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let update = node_b.set_body("changed");
        let changed = node_a.apply_update(&update).unwrap();
        assert!(changed, "content changed, flag should be true");

        // Apply same update again — no content change
        // (yrs deduplicates, so the flag should be false)
        let update2 = node_b.set_body("changed"); // no-op — same content
        let changed2 = node_a.apply_update(&update2).unwrap();
        // The body is still "changed" so hash should match
        assert!(!changed2, "same content, flag should be false");
    }

    // --- 3-client convergence ---

    #[test]
    fn three_client_concurrent_edits_converge() {
        let mut a = KbNodeDoc::new_with_client_id("n1", "T", "base", &[], 1);
        let state = a.encode();
        let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();
        let mut c = KbNodeDoc::from_bytes_with_client_id(&state, 3).unwrap();

        // All three concurrently edit different fields
        let u_a = a.set_title("Title from A");
        let u_b = b.add_tag("tag-from-b");
        let u_c = c.add_link("link-from-c");

        // Apply all updates to all clients
        a.apply_update(&u_b).unwrap();
        a.apply_update(&u_c).unwrap();
        b.apply_update(&u_a).unwrap();
        b.apply_update(&u_c).unwrap();
        c.apply_update(&u_a).unwrap();
        c.apply_update(&u_b).unwrap();

        // All three should converge
        assert_eq!(a.title(), b.title());
        assert_eq!(b.title(), c.title());
        assert_eq!(a.title(), "Title from A");
        assert_eq!(a.tags(), b.tags());
        assert_eq!(b.tags(), c.tags());
        assert!(a.tags().contains(&"tag-from-b".to_string()));
        assert_eq!(a.links(), b.links());
        assert_eq!(b.links(), c.links());
        assert!(a.links().contains(&"link-from-c".to_string()));
    }

    // --- Multi-field concurrent edits ---

    #[test]
    fn concurrent_title_and_body_edits() {
        let mut a = KbNodeDoc::new_with_client_id("n1", "T", "B", &[], 1);
        let state = a.encode();
        let mut b = KbNodeDoc::from_bytes_with_client_id(&state, 2).unwrap();

        let u_a = a.set_title("New Title");
        let u_b = b.set_body("New Body");

        a.apply_update(&u_b).unwrap();
        b.apply_update(&u_a).unwrap();

        assert_eq!(a.title(), "New Title");
        assert_eq!(a.body(), "New Body");
        assert_eq!(a.title(), b.title());
        assert_eq!(a.body(), b.body());
    }

    // --- Link and meta operations ---

    #[test]
    fn link_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &[]);
        node.add_link("target1");
        node.add_link("target2");
        assert_eq!(node.links(), vec!["target1", "target2"]);

        node.remove_link("target1");
        assert_eq!(node.links(), vec!["target2"]);
    }

    #[test]
    fn meta_operations() {
        let mut node = KbNodeDoc::new("n1", "T", "", &[]);
        node.set_meta("author", "alice");
        node.set_meta("version", "2");
        assert_eq!(node.get_meta("author"), Some("alice".to_string()));
        assert_eq!(node.get_meta("version"), Some("2".to_string()));
        assert_eq!(node.get_meta("missing"), None);
    }

    // --- KbCollectionDoc tests ---

    #[test]
    fn collection_basic_creation() {
        let coll = KbCollectionDoc::new("Research Notes", "alice");
        assert_eq!(coll.name(), "Research Notes");
        assert_eq!(coll.creator(), "alice");
        assert_eq!(coll.members(), vec!["alice"]);
        assert_eq!(coll.node_count(), 0);
    }

    #[test]
    fn collection_add_remove_nodes() {
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_node("concept:buffer", "Buffer");
        coll.add_node("concept:window", "Window");
        assert_eq!(coll.node_count(), 2);

        let nodes = coll.list_nodes();
        assert!(nodes.iter().any(|(id, _)| id == "concept:buffer"));
        assert!(nodes.iter().any(|(id, _)| id == "concept:window"));

        coll.remove_node("concept:buffer");
        assert_eq!(coll.node_count(), 1);
    }

    #[test]
    fn collection_members() {
        let mut coll = KbCollectionDoc::new("Test", "alice");
        coll.add_member("bob");
        coll.add_member("bob"); // duplicate — should not be added
        assert_eq!(coll.members(), vec!["alice", "bob"]);

        coll.remove_member("alice");
        assert_eq!(coll.members(), vec!["bob"]);
    }

    #[test]
    fn collection_set_creator_restamps_and_seeds_member() {
        // A collection built with a client-claimed creator...
        let mut coll = KbCollectionDoc::new("Test", "client-name");
        assert_eq!(coll.creator(), "client-name");
        // ...is re-stamped to the authenticated identity, which becomes a member.
        coll.set_creator("alice");
        assert_eq!(coll.creator(), "alice", "creator overridden");
        assert!(
            coll.members().contains(&"alice".to_string()),
            "creator seeded as member"
        );
        // Idempotent: no duplicate member on re-stamp.
        coll.set_creator("alice");
        assert_eq!(
            coll.members().iter().filter(|m| *m == "alice").count(),
            1,
            "no duplicate member"
        );
    }

    #[test]
    fn collection_encode_decode_roundtrip() {
        let mut coll = KbCollectionDoc::new("KB1", "alice");
        coll.add_node("n1", "Node One");
        coll.add_member("bob");

        let bytes = coll.encode_state();
        let restored = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(restored.name(), "KB1");
        assert_eq!(restored.creator(), "alice");
        assert_eq!(restored.node_count(), 1);
        assert_eq!(restored.members().len(), 2);
    }

    #[test]
    fn collection_two_client_merge() {
        let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
        let state = coll_a.encode_state();
        let mut coll_b = KbCollectionDoc::from_bytes(&state).unwrap();

        let u_a = coll_a.add_node("n1", "From A");
        let u_b = coll_b.add_node("n2", "From B");

        coll_a.apply_update(&u_b).unwrap();
        coll_b.apply_update(&u_a).unwrap();

        assert_eq!(coll_a.node_count(), 2);
        assert_eq!(coll_b.node_count(), 2);

        let nodes_a = coll_a.list_nodes();
        let nodes_b = coll_b.list_nodes();
        assert_eq!(nodes_a.len(), nodes_b.len());
    }

    // --- ADR-018 v2 schema: owner / roles / policy / pending ---

    #[test]
    fn role_hierarchy_includes() {
        assert!(Role::Owner.includes(Role::Editor));
        assert!(Role::Owner.includes(Role::Viewer));
        assert!(Role::Editor.includes(Role::Viewer));
        assert!(!Role::Viewer.includes(Role::Editor));
        assert!(!Role::Editor.includes(Role::Owner));
    }

    #[test]
    fn collection_v2_new_owned_seeds_owner_role_policy() {
        let coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "SHA256:owner");
        assert_eq!(coll.owner_label(), "alice");
        assert_eq!(coll.role_of("SHA256:owner"), Some(Role::Owner));
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        assert!(coll.pending().is_empty());
        assert_eq!(coll.member_roles().len(), 1);
    }

    #[test]
    fn collection_v2_roles_and_upsert() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Editor);
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        coll.set_role("SHA256:bob", Role::Viewer);
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Viewer));
        coll.remove_principal("SHA256:bob");
        assert_eq!(coll.role_of("SHA256:bob"), None);
    }

    #[test]
    fn collection_v2_join_policy() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        coll.set_join_policy(JoinPolicy::Restrictive);
        assert_eq!(coll.join_policy(), JoinPolicy::Restrictive);
    }

    #[test]
    fn collection_v2_pending_then_approve_atomic() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        coll.add_pending("SHA256:bob", "bob", "2026-06-16T00:00:00Z");
        assert_eq!(coll.pending().len(), 1);
        assert_eq!(coll.role_of("SHA256:bob"), None);
        coll.approve("SHA256:bob", Role::Editor);
        assert!(coll.pending().is_empty(), "approve clears pending");
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        let m = coll
            .member_roles()
            .into_iter()
            .find(|m| m.fingerprint == "SHA256:bob")
            .unwrap();
        assert_eq!(m.label, "bob", "approve carries the pending label");
    }

    #[test]
    fn collection_v2_two_client_member_merge_converges() {
        let mut a =
            KbCollectionDoc::new_owned_with("KB", "SHA256:o", "alice", Some(1), JoinPolicy::Invite);
        let state = a.encode_state();
        let mut b = KbCollectionDoc::from_bytes(&state).unwrap();
        let ua = a.upsert_member("SHA256:bob", "bob", Role::Editor);
        let ub = b.upsert_member("SHA256:carol", "carol", Role::Viewer);
        a.apply_update(&ub).unwrap();
        b.apply_update(&ua).unwrap();
        for c in [&a, &b] {
            assert_eq!(c.role_of("SHA256:bob"), Some(Role::Editor));
            assert_eq!(c.role_of("SHA256:carol"), Some(Role::Viewer));
        }
        assert_eq!(a.member_roles().len(), b.member_roles().len());
    }

    #[test]
    fn collection_v2_roundtrip_preserves_schema() {
        let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
        coll.upsert_member("SHA256:bob", "bob", Role::Viewer);
        coll.set_join_policy(JoinPolicy::Permissive);
        coll.add_pending("SHA256:eve", "eve", "t");
        let bytes = coll.encode_state();
        let r = KbCollectionDoc::from_bytes(&bytes).unwrap();
        assert_eq!(r.schema_version(), 2);
        assert_eq!(r.owner(), "SHA256:o");
        assert_eq!(r.role_of("SHA256:bob"), Some(Role::Viewer));
        assert_eq!(r.join_policy(), JoinPolicy::Permissive);
        assert_eq!(r.pending().len(), 1);
    }

    #[test]
    fn migrate_v1_resolves_labels_to_principals() {
        // Build a legacy v1 collection (label creator + members YArray).
        let mut coll = KbCollectionDoc::new("KB", "alice");
        coll.add_member("bob");
        assert_eq!(coll.schema_version(), 0, "legacy = no schema key");
        // resolver maps known labels to fingerprints.
        let resolver = |label: &str| match label {
            "alice" => Some(("SHA256:alice".to_string(), "alice".to_string())),
            "bob" => Some(("SHA256:bob".to_string(), "bob".to_string())),
            _ => None,
        };
        let update = coll.migrate_if_legacy(resolver).expect("migrated");
        assert!(!update.is_empty());
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "SHA256:alice");
        assert_eq!(coll.role_of("SHA256:alice"), Some(Role::Owner));
        assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
        assert_eq!(coll.join_policy(), JoinPolicy::Invite);
        // idempotent
        assert!(coll.migrate_if_legacy(resolver).is_none());
    }

    #[test]
    fn migrate_v1_unresolved_label_falls_back_to_legacy_principal() {
        let mut coll = KbCollectionDoc::new("KB", "alice");
        coll.add_member("ghost");
        // resolver knows nobody → legacy:<label> principals.
        coll.migrate_if_legacy(|_| None).expect("migrated");
        assert_eq!(coll.schema_version(), 2);
        assert_eq!(coll.owner(), "legacy:alice");
        assert_eq!(coll.role_of("legacy:alice"), Some(Role::Owner));
        assert_eq!(coll.role_of("legacy:ghost"), Some(Role::Editor));
    }
}
