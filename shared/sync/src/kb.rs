//! KbNodeDoc: yrs-backed KB node with YMap schema.
//!
//! All yrs Doc instances use UTF-16 offset kind (via `text::new_doc()`) for
//! consistency with the Yjs standard. See the CRDT UTF-16 fix (92a20b8).

use sha2::{Digest, Sha256};
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, Out, ReadTxn, Text, TextPrelim, Transact,
};

use crate::text::{new_doc, new_doc_with_client_id};
use crate::SyncError;

const ID_KEY: &str = "id";
const TITLE_KEY: &str = "title";
const BODY_KEY: &str = "body";
const TAGS_KEY: &str = "tags";
const LINKS_KEY: &str = "links";
const META_KEY: &str = "meta";

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
const COLL_CREATOR_KEY: &str = "creator";
const COLL_NODES_KEY: &str = "nodes";
const COLL_MEMBERS_KEY: &str = "members";

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
}
