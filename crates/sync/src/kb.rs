//! KbNodeDoc: yrs-backed KB node with YMap schema.

use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Array, ArrayPrelim, Doc, GetString, Map,
    MapPrelim, Out, ReadTxn, Text, TextPrelim, Transact,
};

use crate::SyncError;

const ID_KEY: &str = "id";
const TITLE_KEY: &str = "title";
const BODY_KEY: &str = "body";
const TAGS_KEY: &str = "tags";
const LINKS_KEY: &str = "links";
const META_KEY: &str = "meta";

/// A KB node represented as a yrs document.
///
/// Schema:
/// - Root YMap "node" contains: id (String), title (YText), body (YText),
///   tags (YArray<String>), links (YArray<String>), meta (YMap<String, String>)
pub struct KbNodeDoc {
    doc: Doc,
}

impl KbNodeDoc {
    /// Create a new KB node document.
    pub fn new(id: &str, title: &str, body: &str, tags: &[String]) -> Self {
        let doc = Doc::new();
        {
            let root = doc.get_or_insert_map("node");
            let mut txn = doc.transact_mut();

            root.insert(&mut txn, ID_KEY, id);

            let title_text = root.insert(&mut txn, TITLE_KEY, TextPrelim::new(title));
            let _ = title_text;

            let body_text = root.insert(&mut txn, BODY_KEY, TextPrelim::new(body));
            let _ = body_text;

            let tags_arr = root.insert(&mut txn, TAGS_KEY, ArrayPrelim::default());
            for tag in tags {
                tags_arr.push_back(&mut txn, tag.as_str());
            }

            let _links = root.insert(&mut txn, LINKS_KEY, ArrayPrelim::default());
            let _meta = root.insert(&mut txn, META_KEY, MapPrelim::default());
        }
        Self { doc }
    }

    /// Load from encoded bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SyncError> {
        let doc = Doc::new();
        let update =
            yrs::Update::decode_v1(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        Ok(Self { doc })
    }

    /// Encode for persistence.
    pub fn encode(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
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
            let len = text.get_string(&txn).len() as u32;
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
            let len = text.get_string(&txn).len() as u32;
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

    /// Apply a remote update.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update)
            .map_err(|e| SyncError::Encoding(e.to_string()))?;
        Ok(())
    }

    /// State vector for sync.
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Access the underlying Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
