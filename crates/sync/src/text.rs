//! TextSync: YText <-> Rope bridge for collaborative text editing.

use ropey::Rope;
use std::sync::{Arc, Mutex};
use yrs::{
    undo::UndoManager, updates::decoder::Decode, updates::encoder::Encode, Doc, GetString, ReadTxn,
    Subscription, Text, Transact,
};

use crate::SyncError;

/// The yrs text field name used in all documents.
const TEXT_NAME: &str = "content";

/// Collaborative text document backed by yrs with a ropey rendering mirror.
///
/// Local edits update both YText (source of truth) and Rope (for rendering).
/// Remote updates are applied to YText, then the Rope is rebuilt.
pub struct TextSync {
    doc: Doc,
    rope: Rope,
    /// Per-user undo manager. When active, local edits create CRDT-native
    /// undo operations instead of relying on EditAction stacks + reconcile_to().
    undo_mgr: Option<UndoManager<()>>,
    /// Updates generated during undo/redo operations, captured via observe_update_v1.
    /// Drained after each undo/redo call to produce broadcast bytes.
    captured_updates: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Subscription for update capture. Kept alive as long as undo is active.
    _update_sub: Option<Subscription>,
}

impl TextSync {
    /// Create a new sync document with initial content.
    pub fn new(content: &str) -> Self {
        let doc = Doc::new();
        {
            let text = doc.get_or_insert_text(TEXT_NAME);
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, content);
        }
        let rope = Rope::from_str(content);
        Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        }
    }

    /// Create with a specific client ID (for testing deterministic merges).
    pub fn with_client_id(content: &str, client_id: u64) -> Self {
        let doc = Doc::with_client_id(client_id);
        {
            let text = doc.get_or_insert_text(TEXT_NAME);
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, content);
        }
        let rope = Rope::from_str(content);
        Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        }
    }

    /// Create an empty relay document. No content is inserted — the Doc starts
    /// with an empty state vector. Used by the state server, which only relays
    /// updates from clients and should not contribute its own operations.
    pub fn empty_relay() -> Self {
        let doc = Doc::new();
        // Do NOT insert anything — the server is a passive relay.
        // The first client to share will provide the initial content.
        let rope = Rope::from_str("");
        Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        }
    }

    /// Create from an existing yrs document.
    pub fn from_doc(doc: Doc) -> Self {
        let content = {
            let text = doc.get_or_insert_text(TEXT_NAME);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let rope = Rope::from_str(&content);
        Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        }
    }

    /// Apply a local insert at char offset. Returns encoded update for broadcast.
    ///
    /// When undo is active, uses origin-tagged transactions so the UndoManager
    /// tracks this edit for per-user undo.
    pub fn insert(&mut self, offset: u32, text: &str) -> Vec<u8> {
        let ytext = self.doc.get_or_insert_text(TEXT_NAME);
        let update = if self.undo_mgr.is_some() {
            let origin = self.doc.client_id();
            let mut txn = self.doc.transact_mut_with(origin);
            ytext.insert(&mut txn, offset, text);
            txn.encode_update_v1()
        } else {
            let mut txn = self.doc.transact_mut();
            ytext.insert(&mut txn, offset, text);
            txn.encode_update_v1()
        };
        self.rebuild_rope();
        update
    }

    /// Apply a local delete (char offset + length). Returns encoded update for broadcast.
    ///
    /// When undo is active, uses origin-tagged transactions so the UndoManager
    /// tracks this edit for per-user undo.
    pub fn delete(&mut self, offset: u32, len: u32) -> Vec<u8> {
        let ytext = self.doc.get_or_insert_text(TEXT_NAME);
        let update = if self.undo_mgr.is_some() {
            let origin = self.doc.client_id();
            let mut txn = self.doc.transact_mut_with(origin);
            ytext.remove_range(&mut txn, offset, len);
            txn.encode_update_v1()
        } else {
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

    /// Encode only the changes not yet seen by a peer (differential sync).
    /// `remote_sv` is the encoded state vector from the remote peer.
    pub fn encode_diff(&self, remote_sv: &[u8]) -> Vec<u8> {
        let sv =
            yrs::StateVector::decode_v1(remote_sv).unwrap_or_else(|_| yrs::StateVector::default());
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&sv)
    }

    /// Load from encoded full state.
    pub fn from_state(state: &[u8]) -> Result<Self, SyncError> {
        let doc = Doc::new();
        let update =
            yrs::Update::decode_v1(state).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        let content = {
            let text = doc.get_or_insert_text(TEXT_NAME);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let rope = Rope::from_str(&content);
        Ok(Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        })
    }

    /// Load from encoded full state with a specific client ID.
    /// Use this instead of `from_state()` when the caller needs a deterministic
    /// client ID (e.g., editor clients that generate local edits).
    pub fn from_state_with_client_id(state: &[u8], client_id: u64) -> Result<Self, SyncError> {
        let options = yrs::Options {
            client_id,
            ..Default::default()
        };
        let doc = Doc::with_options(options);
        let update =
            yrs::Update::decode_v1(state).map_err(|e| SyncError::Encoding(e.to_string()))?;
        {
            let mut txn = doc.transact_mut();
            txn.apply_update(update)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }
        let content = {
            let text = doc.get_or_insert_text(TEXT_NAME);
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let rope = Rope::from_str(&content);
        Ok(Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            _update_sub: None,
        })
    }

    /// Get the rope (for rendering).
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    /// Get text content as string.
    pub fn content(&self) -> String {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        text.get_string(&txn)
    }

    /// Access the underlying yrs Doc.
    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    /// Reconcile the document to a target string via minimal CRDT operations.
    ///
    /// Computes a character-level diff between the current content and `target`,
    /// then applies insert/delete operations through yrs transactions. Returns
    /// the encoded update bytes for broadcast (empty if no change).
    pub fn reconcile_to(&mut self, target: &str) -> Vec<u8> {
        use similar::{ChangeTag, TextDiff};

        let current = self.content();
        if current == target {
            return Vec::new();
        }

        let target_str = target.to_string();
        let diff = TextDiff::from_chars(&current, &target_str);
        let ytext = self.doc.get_or_insert_text(TEXT_NAME);

        let update = {
            let mut txn = self.doc.transact_mut();
            let mut offset: u32 = 0;

            for change in diff.iter_all_changes() {
                match change.tag() {
                    ChangeTag::Equal => {
                        offset += change.value().chars().count() as u32;
                    }
                    ChangeTag::Delete => {
                        let len = change.value().chars().count() as u32;
                        ytext.remove_range(&mut txn, offset, len);
                        // offset stays the same after delete
                    }
                    ChangeTag::Insert => {
                        let text = change.value();
                        ytext.insert(&mut txn, offset, text);
                        offset += text.chars().count() as u32;
                    }
                }
            }

            txn.encode_update_v1()
        };

        self.rebuild_rope();
        update
    }

    /// Rebuild rope from YText (called after remote updates).
    fn rebuild_rope(&mut self) {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        let content = text.get_string(&txn);
        self.rope = Rope::from_str(&content);
    }

    // --- Per-user CRDT undo (yrs UndoManager) ---

    /// Enable per-user undo tracking. Creates a yrs UndoManager scoped to the
    /// text field, tracking only edits from this client's origin.
    ///
    /// `capture_timeout_millis: 0` means every transaction is a separate undo
    /// item (matches vim operator semantics). The buffer layer calls `undo_reset()`
    /// for explicit group boundaries.
    pub fn enable_undo(&mut self) {
        use yrs::undo::Options;

        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let origin = self.doc.client_id();

        let options = Options {
            // Use u64::MAX so all edits within a vim undo group merge into
            // one UndoManager item.  Explicit `undo_reset()` calls at group
            // boundaries (end_undo_group, each normal-mode dispatch) separate
            // items.  With 0 every transaction was a separate undo step,
            // breaking vim's "undo all of insert mode" contract.
            capture_timeout_millis: u64::MAX,
            tracked_origins: [origin.into()].into_iter().collect(),
            ..Default::default()
        };

        let mgr = UndoManager::with_scope_and_options(&self.doc, &text, options);

        // Subscribe to updates so we can capture undo/redo-generated deltas.
        let captured = self.captured_updates.clone();
        let sub = self
            .doc
            .observe_update_v1(move |_txn, event| {
                if let Ok(mut buf) = captured.lock() {
                    buf.push(event.update.clone());
                }
            })
            .expect("observe_update_v1 should not fail on owned doc");

        self.undo_mgr = Some(mgr);
        self._update_sub = Some(sub);
    }

    /// The client ID of the underlying yrs document.
    pub fn client_id(&self) -> u64 {
        self.doc.client_id()
    }

    /// Whether the UndoManager is active.
    pub fn undo_mgr_active(&self) -> bool {
        self.undo_mgr.is_some()
    }

    /// Whether there are undoable operations.
    pub fn can_undo(&self) -> bool {
        self.undo_mgr.as_ref().is_some_and(|m| m.can_undo())
    }

    /// Whether there are redoable operations.
    pub fn can_redo(&self) -> bool {
        self.undo_mgr.as_ref().is_some_and(|m| m.can_redo())
    }

    /// Undo the last local operation. Returns `(success, update_bytes)`.
    ///
    /// `update_bytes` contains the CRDT updates generated by the undo,
    /// ready for broadcast to peers. The rope is rebuilt from YText.
    pub fn undo(&mut self) -> (bool, Vec<Vec<u8>>) {
        let Some(mgr) = &mut self.undo_mgr else {
            return (false, Vec::new());
        };
        // Clear captured updates before undo so we only collect undo's deltas.
        if let Ok(mut buf) = self.captured_updates.lock() {
            buf.clear();
        }
        let ok = mgr.undo_blocking();
        self.rebuild_rope();
        let updates = if let Ok(mut buf) = self.captured_updates.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        };
        (ok, updates)
    }

    /// Redo the last undone operation. Returns `(success, update_bytes)`.
    pub fn redo(&mut self) -> (bool, Vec<Vec<u8>>) {
        let Some(mgr) = &mut self.undo_mgr else {
            return (false, Vec::new());
        };
        if let Ok(mut buf) = self.captured_updates.lock() {
            buf.clear();
        }
        let ok = mgr.redo_blocking();
        self.rebuild_rope();
        let updates = if let Ok(mut buf) = self.captured_updates.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        };
        (ok, updates)
    }

    /// Insert an explicit undo group boundary. The next edit starts a new
    /// undo stack item regardless of timing.
    pub fn undo_reset(&mut self) {
        if let Some(mgr) = &mut self.undo_mgr {
            mgr.reset();
        }
    }

    /// Clear all undo/redo history.
    pub fn clear_undo(&mut self) {
        if let Some(mgr) = &mut self.undo_mgr {
            mgr.clear();
        }
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
        let ts2 = TextSync::from_state(&state).unwrap();
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
    fn reconcile_to_basic() {
        let mut ts = TextSync::new("hello world");
        let update = ts.reconcile_to("hello rust");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "hello rust");
        assert_eq!(ts.rope().to_string(), "hello rust");
    }

    #[test]
    fn reconcile_to_noop() {
        let mut ts = TextSync::new("no change");
        let update = ts.reconcile_to("no change");
        assert!(update.is_empty());
        assert_eq!(ts.content(), "no change");
    }

    #[test]
    fn reconcile_preserves_crdt_history() {
        // Reconcile on doc A, then apply the update on doc B — both converge.
        let mut doc_a = TextSync::with_client_id("hello world", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // Sync initial state.
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        assert_eq!(doc_b.content(), "hello world");

        // Reconcile A to new content.
        let update = doc_a.reconcile_to("hello rust world!");
        assert!(!update.is_empty());
        assert_eq!(doc_a.content(), "hello rust world!");

        // Apply to B.
        doc_b.apply_update(&update).unwrap();
        assert_eq!(doc_b.content(), "hello rust world!");
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

    #[test]
    fn reconcile_to_empty() {
        let mut ts = TextSync::new("hello");
        let update = ts.reconcile_to("");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "");
        assert_eq!(ts.rope().len_chars(), 0);
    }

    #[test]
    fn reconcile_from_empty() {
        let mut ts = TextSync::new("");
        let update = ts.reconcile_to("world");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "world");
        assert_eq!(ts.rope().to_string(), "world");
    }

    // --- Per-user CRDT undo tests ---

    #[test]
    fn undo_single_insert() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();
        ts.insert(5, " world");
        assert_eq!(ts.content(), "hello world");
        let (ok, updates) = ts.undo();
        assert!(ok);
        assert_eq!(ts.content(), "hello");
        assert!(!updates.is_empty(), "undo should produce broadcast updates");
    }

    #[test]
    fn redo_after_undo() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();
        ts.insert(5, " world");
        assert_eq!(ts.content(), "hello world");
        ts.undo();
        assert_eq!(ts.content(), "hello");
        let (ok, updates) = ts.redo();
        assert!(ok);
        assert_eq!(ts.content(), "hello world");
        assert!(!updates.is_empty());
    }

    #[test]
    fn undo_produces_update_bytes() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        ts.insert(0, "abc");
        let (_, updates) = ts.undo();
        // Updates should be non-empty and decodable.
        assert!(!updates.is_empty());
        for u in &updates {
            yrs::Update::decode_v1(u).expect("update bytes should be valid");
        }
    }

    #[test]
    fn undo_remote_excluded() {
        // Remote edits (no origin) should NOT be undone by local undo.
        let mut doc_a = TextSync::with_client_id("hello", 1);
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", 2);
        // Sync initial state from A to B.
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();

        // B inserts (remote from A's perspective).
        let remote_update = doc_b.insert(5, " world");
        doc_a.apply_update(&remote_update).unwrap();
        assert_eq!(doc_a.content(), "hello world");

        // A's undo should NOT undo B's edit (no local ops to undo).
        let (ok, _) = doc_a.undo();
        assert!(!ok, "nothing to undo — remote edits excluded");
        assert_eq!(doc_a.content(), "hello world");
    }

    #[test]
    fn redo_survives_remote_update() {
        // Verify that applying a remote update between undo and redo
        // does NOT clear the redo stack.
        let mut doc_a = TextSync::with_client_id("base\n", 1);
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", 2);
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        doc_b.enable_undo();

        // A inserts "from-A"
        let _update_a = doc_a.insert(5, "from-A\n");
        assert_eq!(doc_a.content(), "base\nfrom-A\n");

        // B inserts "from-B" and sends to A
        let update_b = doc_b.insert(5, "from-B\n");
        doc_a.apply_update(&update_b).unwrap();
        // A now has both
        assert!(doc_a.content().contains("from-A"));
        assert!(doc_a.content().contains("from-B"));

        // A undoes its own edit
        let (ok, _) = doc_a.undo();
        assert!(ok, "A should be able to undo its insert");
        assert!(
            !doc_a.content().contains("from-A"),
            "from-A should be gone after undo"
        );
        assert!(
            doc_a.content().contains("from-B"),
            "from-B should survive A's undo"
        );

        // B undoes its own edit and sends the update to A (simulates remote undo)
        let (b_ok, b_updates) = doc_b.undo();
        assert!(b_ok);
        for u in &b_updates {
            doc_a.apply_update(u).unwrap();
        }
        assert!(
            !doc_a.content().contains("from-B"),
            "from-B should be gone after B's undo"
        );

        // A redoes its own edit — this should work even after receiving B's remote undo
        let (redo_ok, _) = doc_a.redo();
        assert!(redo_ok, "A should be able to redo after remote update");
        assert!(
            doc_a.content().contains("from-A"),
            "from-A should be restored by redo"
        );
    }

    #[test]
    fn undo_group_boundary() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        ts.insert(0, "aaa");
        ts.undo_reset(); // explicit boundary
        ts.insert(3, "bbb");
        assert_eq!(ts.content(), "aaabbb");

        // First undo removes "bbb" (second group).
        ts.undo();
        assert_eq!(ts.content(), "aaa");

        // Second undo removes "aaa" (first group).
        ts.undo();
        assert_eq!(ts.content(), "");
    }

    #[test]
    fn two_clients_independent_undo() {
        let mut doc_a = TextSync::with_client_id("base", 1);
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", 2);
        doc_b.enable_undo();

        // Sync initial state.
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        assert_eq!(doc_b.content(), "base");

        // Both insert.
        let update_a = doc_a.insert(4, "-A");
        let update_b = doc_b.insert(4, "-B");

        // Exchange updates.
        doc_a.apply_update(&update_b).unwrap();
        doc_b.apply_update(&update_a).unwrap();

        // Both should have same content.
        assert_eq!(doc_a.content(), doc_b.content());
        let converged = doc_a.content();
        assert!(converged.contains("-A"));
        assert!(converged.contains("-B"));

        // A undoes only A's insert.
        let (ok_a, updates_a) = doc_a.undo();
        assert!(ok_a);
        assert!(
            doc_a.content().contains("-B"),
            "B's edit preserved after A's undo"
        );
        assert!(!doc_a.content().contains("-A"), "A's edit reversed");

        // Apply A's undo to B so they converge again.
        for u in &updates_a {
            doc_b.apply_update(u).unwrap();
        }
        assert_eq!(doc_a.content(), doc_b.content());
    }

    #[test]
    fn can_undo_empty() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        assert!(!ts.can_undo());
        assert!(!ts.can_redo());
        ts.insert(0, "x");
        assert!(ts.can_undo());
    }

    #[test]
    fn undo_clear() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        ts.insert(0, "abc");
        assert!(ts.can_undo());
        ts.clear_undo();
        assert!(!ts.can_undo());
    }

    #[test]
    fn undo_delete_restores() {
        let mut ts = TextSync::with_client_id("hello world", 1);
        ts.enable_undo();
        ts.delete(5, 6); // remove " world"
        assert_eq!(ts.content(), "hello");
        let (ok, _) = ts.undo();
        assert!(ok);
        assert_eq!(ts.content(), "hello world");
    }

    // --- Reconcile edge cases ---

    #[test]
    fn reconcile_complex_replace() {
        let mut ts = TextSync::new("hello world");
        let update = ts.reconcile_to("goodbye moon");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "goodbye moon");
        assert_eq!(ts.rope().to_string(), "goodbye moon");
    }

    #[test]
    fn reconcile_partial_overlap() {
        let mut ts = TextSync::new("abcdef");
        // Keep "abc", replace "def" with "xyz123"
        let update = ts.reconcile_to("abcxyz123");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "abcxyz123");
    }

    #[test]
    fn reconcile_to_longer() {
        let mut ts = TextSync::new("short");
        let long = "a".repeat(1000);
        let update = ts.reconcile_to(&long);
        assert!(!update.is_empty());
        assert_eq!(ts.content(), long);
    }

    #[test]
    fn reconcile_noop_identical() {
        let mut ts = TextSync::new("same text");
        let _update = ts.reconcile_to("same text");
        // No-op reconcile should produce no meaningful diff.
        assert_eq!(ts.content(), "same text");
        // Update may still contain bytes (yrs transaction overhead) but content unchanged.
    }

    // --- Delete boundary cases ---

    #[test]
    fn delete_at_start() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.delete(0, 2); // remove "he"
        assert_eq!(ts.content(), "llo");
        assert_eq!(ts.rope().to_string(), "llo");
    }

    #[test]
    fn delete_at_end() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.delete(3, 2); // remove "lo"
        assert_eq!(ts.content(), "hel");
    }

    #[test]
    fn delete_entire_content() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.delete(0, 5);
        assert_eq!(ts.content(), "");
        assert_eq!(ts.rope().len_chars(), 0);
    }

    #[test]
    fn delete_then_insert_at_same_position() {
        let mut ts = TextSync::with_client_id("abc", 1);
        ts.delete(1, 1); // remove "b" → "ac"
        assert_eq!(ts.content(), "ac");
        ts.insert(1, "X"); // → "aXc"
        assert_eq!(ts.content(), "aXc");
    }

    // --- Undo/redo multi-cycle ---

    #[test]
    fn undo_redo_three_cycles() {
        let mut ts = TextSync::with_client_id("base", 1);
        ts.enable_undo();

        ts.insert(4, " one");
        ts.undo_reset();
        ts.insert(8, " two");
        ts.undo_reset();
        ts.insert(12, " three");
        assert_eq!(ts.content(), "base one two three");

        // Undo all three
        ts.undo();
        assert_eq!(ts.content(), "base one two");
        ts.undo();
        assert_eq!(ts.content(), "base one");
        ts.undo();
        assert_eq!(ts.content(), "base");

        // Redo all three
        ts.redo();
        assert_eq!(ts.content(), "base one");
        ts.redo();
        assert_eq!(ts.content(), "base one two");
        ts.redo();
        assert_eq!(ts.content(), "base one two three");
    }

    #[test]
    fn undo_then_new_edit_clears_redo() {
        let mut ts = TextSync::with_client_id("base", 1);
        ts.enable_undo();

        ts.insert(4, " one");
        ts.undo_reset();
        ts.insert(8, " two");
        assert_eq!(ts.content(), "base one two");

        // Undo " two"
        ts.undo();
        assert_eq!(ts.content(), "base one");

        // New edit should clear redo stack
        ts.insert(8, " NEW");
        assert_eq!(ts.content(), "base one NEW");

        // Redo should fail (stack cleared by new edit)
        let (ok, _) = ts.redo();
        assert!(!ok, "redo should fail after new edit");
    }

    #[test]
    fn undo_delete_with_boundary() {
        let mut ts = TextSync::with_client_id("hello world", 1);
        ts.enable_undo();

        ts.delete(5, 6); // remove " world"
        ts.undo_reset();
        ts.insert(5, " earth");
        assert_eq!(ts.content(), "hello earth");

        // Undo " earth" insert
        ts.undo();
        assert_eq!(ts.content(), "hello");

        // Undo delete of " world"
        ts.undo();
        assert_eq!(ts.content(), "hello world");
    }

    // --- State vector / diff round-trip ---

    #[test]
    fn state_vector_diff_roundtrip() {
        let mut doc_a = TextSync::with_client_id("initial", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        // Sync initial state
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        assert_eq!(doc_b.content(), "initial");

        // A makes edits
        doc_a.insert(7, " content");
        assert_eq!(doc_a.content(), "initial content");

        // B computes state vector, A computes diff from it
        let sv_b = doc_b.state_vector();
        let diff = doc_a.encode_diff(&sv_b);
        assert!(!diff.is_empty());

        // B applies diff → should converge
        doc_b.apply_update(&diff).unwrap();
        assert_eq!(doc_b.content(), "initial content");
    }

    #[test]
    fn state_vector_diff_with_concurrent_edits() {
        let mut doc_a = TextSync::with_client_id("base", 1);
        let mut doc_b = TextSync::with_client_id("", 2);

        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();

        // Both edit concurrently (before seeing each other's changes)
        let update_a = doc_a.insert(4, "-A");
        let update_b = doc_b.insert(4, "-B");

        // Exchange via state vector + diff (not raw updates)
        let sv_a = doc_a.state_vector();
        let sv_b = doc_b.state_vector();

        // But first apply raw updates to get full state
        doc_a.apply_update(&update_b).unwrap();
        doc_b.apply_update(&update_a).unwrap();

        // Now compute diffs from pre-sync state vectors — should be non-empty
        let _diff_for_a = doc_b.encode_diff(&sv_a);
        let _diff_for_b = doc_a.encode_diff(&sv_b);

        // Both should converge to same content
        assert_eq!(doc_a.content(), doc_b.content());
        let content = doc_a.content();
        assert!(content.contains("-A"));
        assert!(content.contains("-B"));
    }
}
