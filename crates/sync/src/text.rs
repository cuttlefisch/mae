//! TextSync: YText <-> Rope bridge for collaborative text editing.

use ropey::Rope;
use yrs::{
    updates::decoder::Decode, updates::encoder::Encode, Doc, GetString, ReadTxn, Text, Transact,
};

use crate::SyncError;

/// Collaborative text document backed by yrs with a ropey rendering mirror.
///
/// Local edits update both YText (source of truth) and Rope (for rendering).
/// Remote updates are applied to YText, then the Rope is rebuilt.
pub struct TextSync {
    doc: Doc,
    text_name: String,
    rope: Rope,
}

impl TextSync {
    /// Create a new sync document with initial content.
    pub fn new(content: &str) -> Self {
        let doc = Doc::new();
        {
            let text = doc.get_or_insert_text("content");
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, content);
        }
        let rope = Rope::from_str(content);
        Self {
            doc,
            text_name: "content".to_string(),
            rope,
        }
    }

    /// Create with a specific client ID (for testing deterministic merges).
    pub fn with_client_id(content: &str, client_id: u64) -> Self {
        let doc = Doc::with_client_id(client_id);
        {
            let text = doc.get_or_insert_text("content");
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, content);
        }
        let rope = Rope::from_str(content);
        Self {
            doc,
            text_name: "content".to_string(),
            rope,
        }
    }

    /// Create from an existing yrs document.
    pub fn from_doc(doc: Doc, text_name: &str) -> Self {
        let content = {
            let text = doc.get_or_insert_text(text_name);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let rope = Rope::from_str(&content);
        Self {
            doc,
            text_name: text_name.to_string(),
            rope,
        }
    }

    /// Apply a local insert at char offset. Returns encoded update for broadcast.
    pub fn insert(&mut self, offset: u32, text: &str) -> Vec<u8> {
        let ytext = self.doc.get_or_insert_text(&*self.text_name);
        let update = {
            let mut txn = self.doc.transact_mut();
            ytext.insert(&mut txn, offset, text);
            txn.encode_update_v1()
        };
        self.rebuild_rope();
        update
    }

    /// Apply a local delete (char offset + length). Returns encoded update for broadcast.
    pub fn delete(&mut self, offset: u32, len: u32) -> Vec<u8> {
        let ytext = self.doc.get_or_insert_text(&*self.text_name);
        let update = {
            let mut txn = self.doc.transact_mut();
            ytext.remove_range(&mut txn, offset, len);
            txn.encode_update_v1()
        };
        self.rebuild_rope();
        update
    }

    /// Apply a remote update from another client.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        self.rebuild_rope();
        Ok(())
    }

    /// Get the current state vector (for sync protocol).
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Encode the full document state (for persistence or new client sync).
    pub fn encode_state(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// Load from encoded full state.
    pub fn from_state(state: &[u8], text_name: &str) -> Result<Self, SyncError> {
        let doc = Doc::new();
        let update =
            yrs::Update::decode_v1(state).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        let content = {
            let text = doc.get_or_insert_text(text_name);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let rope = Rope::from_str(&content);
        Ok(Self {
            doc,
            text_name: text_name.to_string(),
            rope,
        })
    }

    /// Get the rope (for rendering).
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Get text content as string.
    pub fn content(&self) -> String {
        let text = self.doc.get_or_insert_text(&*self.text_name);
        let txn = self.doc.transact();
        text.get_string(&txn)
    }

    /// Access the underlying yrs Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Rebuild rope from YText (called after remote updates).
    fn rebuild_rope(&mut self) {
        let text = self.doc.get_or_insert_text(&*self.text_name);
        let txn = self.doc.transact();
        let content = text.get_string(&txn);
        self.rope = Rope::from_str(&content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_doc() {
        let ts = TextSync::new("");
        assert_eq!(ts.content(), "");
        assert_eq!(ts.rope().len_chars(), 0);
    }

    #[test]
    fn new_with_content() {
        let ts = TextSync::new("hello\nworld");
        assert_eq!(ts.content(), "hello\nworld");
        assert_eq!(ts.rope().len_lines(), 2);
    }

    #[test]
    fn insert_updates_both() {
        let mut ts = TextSync::new("hello");
        ts.insert(5, " world");
        assert_eq!(ts.content(), "hello world");
        assert_eq!(ts.rope().to_string(), "hello world");
    }

    #[test]
    fn delete_updates_both() {
        let mut ts = TextSync::new("hello world");
        ts.delete(5, 6);
        assert_eq!(ts.content(), "hello");
        assert_eq!(ts.rope().to_string(), "hello");
    }

    #[test]
    fn apply_remote_update() {
        let mut doc_a = TextSync::with_client_id("hello", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // Sync initial state from A to B
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        assert_eq!(doc_b.content(), "hello");

        // A inserts, sends update to B
        let update = doc_a.insert(5, " world");
        doc_b.apply_update(&update).unwrap();
        assert_eq!(doc_b.content(), "hello world");
    }

    #[test]
    fn two_clients_converge() {
        let mut doc_a = TextSync::with_client_id("hello", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // Sync initial state from A to B
        let state_a = doc_a.encode_state();
        doc_b.apply_update(&state_a).unwrap();
        assert_eq!(doc_b.content(), "hello");

        // Both insert at different positions concurrently
        let update_a = doc_a.insert(0, "A:");
        let update_b = doc_b.insert(5, "!");

        // Exchange updates
        doc_a.apply_update(&update_b).unwrap();
        doc_b.apply_update(&update_a).unwrap();

        // Both should converge to same content
        assert_eq!(doc_a.content(), doc_b.content());
        let content = doc_a.content();
        assert!(content.contains("A:"));
        assert!(content.contains("!"));
        assert!(content.contains("hello"));
    }

    #[test]
    fn concurrent_inserts_same_position() {
        let mut doc_a = TextSync::with_client_id("", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // Both insert at position 0
        let update_a = doc_a.insert(0, "AAA");
        let update_b = doc_b.insert(0, "BBB");

        // Exchange
        doc_a.apply_update(&update_b).unwrap();
        doc_b.apply_update(&update_a).unwrap();

        // Must converge (order determined by client ID)
        assert_eq!(doc_a.content(), doc_b.content());
        // Content should contain both insertions
        let content = doc_a.content();
        assert!(content.contains("AAA"));
        assert!(content.contains("BBB"));
    }

    #[test]
    fn large_document_roundtrip() {
        let lines: String = (0..10_000)
            .map(|i| format!("Line {i}: some content here\n"))
            .collect();
        let ts = TextSync::new(&lines);

        let state = ts.encode_state();
        let ts2 = TextSync::from_state(&state, "content").unwrap();
        assert_eq!(ts.content(), ts2.content());
        assert_eq!(ts.rope().len_lines(), ts2.rope().len_lines());
    }

    #[test]
    fn state_vector_diff() {
        let mut doc_a = TextSync::with_client_id("hello", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // B starts with A's initial state
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();

        // A makes more edits
        doc_a.insert(5, " world");
        doc_a.insert(11, "!");

        // B requests diff based on its state vector
        let sv_b = doc_b.state_vector();
        let sv = yrs::StateVector::decode_v1(&sv_b).unwrap();
        let txn = doc_a.doc().transact();
        let diff = txn.encode_state_as_update_v1(&sv);

        // Apply diff to B
        doc_b.apply_update(&diff).unwrap();
        assert_eq!(doc_b.content(), "hello world!");
    }

    #[test]
    fn stress_convergence() {
        use rand::Rng;

        // Create doc 0 with content, rest empty — then sync
        let mut docs: Vec<TextSync> = Vec::new();
        docs.push(TextSync::with_client_id("start", 1));
        for i in 1..5u64 {
            docs.push(TextSync::with_client_id("", i + 1));
        }

        // Sync initial state from doc 0 to all others
        let state = docs[0].encode_state();
        for doc in docs.iter_mut().skip(1) {
            doc.apply_update(&state).unwrap();
        }

        let mut rng = rand::thread_rng();
        let mut pending_updates: Vec<Vec<(usize, Vec<u8>)>> = vec![Vec::new(); 5];

        // Each doc does 200 random operations
        for _ in 0..200 {
            for i in 0..5 {
                let len = docs[i].content().len() as u32;
                if len == 0 || rng.gen_bool(0.6) {
                    // Insert
                    let pos = if len == 0 { 0 } else { rng.gen_range(0..len) };
                    let ch = (b'a' + rng.gen_range(0..26u8)) as char;
                    let update = docs[i].insert(pos, &ch.to_string());
                    pending_updates[i].push((i, update));
                } else {
                    // Delete
                    let pos = rng.gen_range(0..len);
                    let update = docs[i].delete(pos, 1);
                    pending_updates[i].push((i, update));
                }
            }
        }

        // Exchange all updates between all docs
        for (i, batch) in pending_updates.iter_mut().enumerate() {
            let updates = std::mem::take(batch);
            for (_, update) in &updates {
                for (j, doc) in docs.iter_mut().enumerate() {
                    if j != i {
                        doc.apply_update(update).unwrap();
                    }
                }
            }
        }

        // All docs must converge
        let expected = docs[0].content();
        for (i, doc) in docs.iter().enumerate().skip(1) {
            assert_eq!(doc.content(), expected, "Doc {i} diverged from doc 0");
        }
    }
}
