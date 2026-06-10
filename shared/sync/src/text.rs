//! TextSync: YText <-> Rope bridge for collaborative text editing.

use ropey::Rope;
use std::sync::{Arc, Mutex};
use yrs::{
    block::ClientID, doc::OffsetKind, undo::UndoManager, updates::decoder::Decode,
    updates::encoder::Encode, Doc, GetString, ReadTxn, Subscription, Text, Transact,
};

use crate::SyncError;

/// The yrs text field name used in all documents.
const TEXT_NAME: &str = "content";

/// Create a yrs Doc configured with UTF-16 offset kind (the Yjs standard).
///
/// All Doc instances MUST use this to ensure offset consistency. Using the
/// default `OffsetKind::Bytes` causes char↔yrs offset mismatches for non-ASCII text.
pub(crate) fn new_doc() -> Doc {
    Doc::with_options(yrs::Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

/// Create a yrs Doc with a specific client ID and UTF-16 offset kind.
pub(crate) fn new_doc_with_client_id(client_id: u64) -> Doc {
    Doc::with_options(yrs::Options {
        client_id: ClientID::new(client_id),
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

/// Maximum number of undo stack items before old items are evicted.
/// Matches Emacs's `undo-limit` philosophy — generous but bounded.
const DEFAULT_UNDO_LIMIT: usize = 1000;

/// Cursor position metadata stored on each undo StackItem.
/// Captures the cursor offset at the time the undo group was created,
/// so undo/redo can restore precise cursor position.
#[derive(Debug, Clone, Default)]
pub struct CursorMeta {
    /// Character offset of the cursor when this undo item was created.
    pub cursor_offset: u32,
}

/// Result from an undo or redo operation.
pub struct UndoResult {
    /// Whether the operation succeeded (had something to undo/redo).
    pub success: bool,
    /// CRDT update bytes for broadcast to peers.
    pub updates: Vec<Vec<u8>>,
    /// Cursor offset to restore (from the undo stack item's metadata).
    pub cursor_offset: Option<u32>,
}

/// Collaborative text document backed by yrs with a ropey rendering mirror.
///
/// Local edits update both YText (source of truth) and Rope (for rendering).
/// Remote updates are applied to YText, then the Rope is rebuilt.
pub struct TextSync {
    doc: Doc,
    rope: Rope,
    /// Per-user undo manager with cursor metadata on each stack item.
    undo_mgr: Option<UndoManager<CursorMeta>>,
    /// Updates generated during undo/redo operations, captured via observe_update_v1.
    /// Drained after each undo/redo call to produce broadcast bytes.
    captured_updates: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Current cursor offset, shared with observe_item_added callback.
    /// Updated by the buffer layer before each edit via `set_cursor_offset()`.
    cursor_offset: Arc<Mutex<u32>>,
    /// Cursor offset restored by observe_item_popped callback during undo/redo.
    restored_cursor: Arc<Mutex<Option<u32>>>,
    /// Subscription for update capture. Kept alive as long as undo is active.
    _update_sub: Option<Subscription>,
    /// Subscriptions for undo item observers. Kept alive as long as undo is active.
    _undo_subs: Vec<Subscription>,
    /// Maximum undo stack depth. Old items are silently dropped when exceeded.
    undo_limit: usize,
}

impl TextSync {
    /// Create a new sync document with initial content.
    pub fn new(content: &str) -> Self {
        let doc = new_doc();
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
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// Create with a specific client ID (for testing deterministic merges).
    pub fn with_client_id(content: &str, client_id: u64) -> Self {
        let doc = new_doc_with_client_id(client_id);
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
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// Create an empty relay document. No content is inserted — the Doc starts
    /// with an empty state vector. Used by the state server, which only relays
    /// updates from clients and should not contribute its own operations.
    pub fn empty_relay() -> Self {
        let doc = new_doc();
        // Do NOT insert anything — the server is a passive relay.
        // The first client to share will provide the initial content.
        let rope = Rope::from_str("");
        Self {
            doc,
            rope,
            undo_mgr: None,
            captured_updates: Arc::new(Mutex::new(Vec::new())),
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
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
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        }
    }

    /// Apply a local insert at char offset. Returns encoded update for broadcast.
    ///
    /// When undo is active, uses origin-tagged transactions so the UndoManager
    /// tracks this edit for per-user undo.
    ///
    /// Note: yrs is configured with `OffsetKind::Utf16` (the Yjs standard),
    /// so we convert char offsets to UTF-16 code unit offsets via ropey's
    /// O(log n) `char_to_utf16_cu()` before calling yrs.
    pub fn insert(&mut self, offset: u32, text: &str) -> Vec<u8> {
        let utf16_offset = self.char_to_utf16_offset(offset);
        let ytext = self.doc.get_or_insert_text(TEXT_NAME);
        let update = if self.undo_mgr.is_some() {
            let origin = self.doc.client_id();
            let mut txn = self.doc.transact_mut_with(origin);
            ytext.insert(&mut txn, utf16_offset, text);
            txn.encode_update_v1()
        } else {
            let mut txn = self.doc.transact_mut();
            ytext.insert(&mut txn, utf16_offset, text);
            txn.encode_update_v1()
        };
        self.rebuild_rope();
        update
    }

    /// Apply a local delete (char offset + char length). Returns encoded update for broadcast.
    ///
    /// When undo is active, uses origin-tagged transactions so the UndoManager
    /// tracks this edit for per-user undo.
    ///
    /// Note: yrs is configured with `OffsetKind::Utf16` (the Yjs standard),
    /// so we convert char offset/length to UTF-16 code unit offset/length.
    pub fn delete(&mut self, offset: u32, len: u32) -> Vec<u8> {
        let utf16_offset = self.char_to_utf16_offset(offset);
        let utf16_len = self.char_len_to_utf16_len(offset, len);
        let ytext = self.doc.get_or_insert_text(TEXT_NAME);
        let update = if self.undo_mgr.is_some() {
            let origin = self.doc.client_id();
            let mut txn = self.doc.transact_mut_with(origin);
            ytext.remove_range(&mut txn, utf16_offset, utf16_len);
            txn.encode_update_v1()
        } else {
            let mut txn = self.doc.transact_mut();
            ytext.remove_range(&mut txn, utf16_offset, utf16_len);
            txn.encode_update_v1()
        };
        self.rebuild_rope();
        update
    }

    /// Apply a remote update from another client.
    pub fn apply_update(&mut self, update: &[u8]) -> Result<(), SyncError> {
        let update_decoded =
            yrs::Update::decode_v1(update).map_err(|e| SyncError::Encoding(e.to_string()))?;

        // Diagnostic: log update contents and state vector before apply.
        let update_sv = update_decoded.state_vector();
        for (&client_id, &clock) in update_sv.iter() {
            tracing::info!(
                client_id = client_id.get(),
                clock,
                "  update contains ops from"
            );
        }
        let sv_before = {
            let txn = self.doc.transact();
            txn.state_vector()
        };
        let content_before = self.content();

        // Check for overlap: update's client_ids already in our state vector.
        for (&client_id, &update_clock) in update_sv.iter() {
            let local_clock = sv_before.get(&client_id);
            if local_clock > 0 {
                tracing::warn!(
                    client_id = client_id.get(),
                    update_clock,
                    local_clock,
                    "OVERLAP: update client already in local state vector"
                );
            }
        }

        {
            let mut txn = self.doc.transact_mut();
            txn.apply_update(update_decoded)
                .map_err(|e| SyncError::Encoding(e.to_string()))?;
        }

        // Diagnostic: log state vector after apply.
        let sv_after = {
            let txn = self.doc.transact();
            txn.state_vector()
        };

        self.rebuild_rope();
        let content_after = self.content();
        let content_changed = content_before != content_after;
        // Heuristic: if SV didn't advance and content didn't change,
        // likely the update items are stuck in yrs pending queue.
        let sv_unchanged = sv_before == sv_after;

        if content_changed {
            tracing::info!(
                local_client_id = self.doc.client_id().get(),
                update_len = update.len(),
                content_len_before = content_before.len(),
                content_len_after = content_after.len(),
                "TextSync::apply_update — content CHANGED"
            );
        } else {
            tracing::warn!(
                local_client_id = self.doc.client_id().get(),
                update_len = update.len(),
                sv_unchanged,
                sv_before_entries = sv_before.len(),
                sv_after_entries = sv_after.len(),
                "TextSync::apply_update — content UNCHANGED (no-op)"
            );
            for (&client_id, &clock) in sv_before.iter() {
                tracing::warn!(client_id = client_id.get(), clock, "  sv_before entry");
            }
            for (&client_id, &clock) in sv_after.iter() {
                tracing::warn!(client_id = client_id.get(), clock, "  sv_after entry");
            }
        }

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
        let doc = new_doc();
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
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
        })
    }

    /// Load from encoded full state with a specific client ID.
    /// Use this instead of `from_state()` when the caller needs a deterministic
    /// client ID (e.g., editor clients that generate local edits).
    pub fn from_state_with_client_id(state: &[u8], client_id: u64) -> Result<Self, SyncError> {
        let doc = new_doc_with_client_id(client_id);
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
            cursor_offset: Arc::new(Mutex::new(0)),
            restored_cursor: Arc::new(Mutex::new(None)),
            _update_sub: None,
            _undo_subs: Vec::new(),
            undo_limit: DEFAULT_UNDO_LIMIT,
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
    ///
    /// Note: yrs uses byte offsets (`OffsetKind::Bytes`), so we track byte
    /// offsets alongside char offsets throughout the diff application.
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
            let mut utf16_offset: u32 = 0;

            for change in diff.iter_all_changes() {
                let utf16_len: u32 = change.value().chars().map(|c| c.len_utf16() as u32).sum();
                match change.tag() {
                    ChangeTag::Equal => {
                        utf16_offset += utf16_len;
                    }
                    ChangeTag::Delete => {
                        ytext.remove_range(&mut txn, utf16_offset, utf16_len);
                    }
                    ChangeTag::Insert => {
                        let text = change.value();
                        ytext.insert(&mut txn, utf16_offset, text);
                        utf16_offset += utf16_len;
                    }
                }
            }

            txn.encode_update_v1()
        };

        self.rebuild_rope();
        update
    }

    /// Convert a char offset (Unicode scalar values) to UTF-16 code unit offset.
    ///
    /// yrs is configured with `OffsetKind::Utf16` (the Yjs standard), so we must
    /// convert Rust char offsets to UTF-16 code unit counts. Uses ropey's native
    /// `char_to_utf16_cu()` which is O(log n) via B-tree metadata — same
    /// performance as byte offset conversion.
    fn char_to_utf16_offset(&self, char_offset: u32) -> u32 {
        let clamped = (char_offset as usize).min(self.rope.len_chars());
        self.rope.char_to_utf16_cu(clamped) as u32
    }

    /// Convert a char-length span starting at `char_offset` to UTF-16 code unit length.
    fn char_len_to_utf16_len(&self, char_offset: u32, char_len: u32) -> u32 {
        let start = (char_offset as usize).min(self.rope.len_chars());
        let end = (start + char_len as usize).min(self.rope.len_chars());
        let utf16_start = self.rope.char_to_utf16_cu(start);
        let utf16_end = self.rope.char_to_utf16_cu(end);
        (utf16_end - utf16_start) as u32
    }

    /// Rebuild rope from YText (called after remote updates).
    fn rebuild_rope(&mut self) {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        let content = text.get_string(&txn);
        self.rope = Rope::from_str(&content);
    }

    // --- Per-user CRDT undo (yrs UndoManager) ---

    /// Set the current cursor offset. Called by the buffer layer before edits
    /// so the UndoManager can capture cursor position on each undo stack item.
    pub fn set_cursor_offset(&self, offset: u32) {
        if let Ok(mut cur) = self.cursor_offset.lock() {
            *cur = offset;
        }
    }

    /// Enable per-user undo tracking. Creates a yrs UndoManager scoped to the
    /// text field, tracking only edits from this client's origin.
    ///
    /// Uses `capture_timeout_millis: u64::MAX` so all edits within a vim undo
    /// group merge into one UndoManager item. Explicit `undo_reset()` calls at
    /// group boundaries (end_undo_group, each normal-mode dispatch) separate items.
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

        let mut mgr: UndoManager<CursorMeta> = UndoManager::with_options(options);
        mgr.expand_scope(&self.doc, &text);

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

        // Save cursor offset into undo stack item metadata when a new item is created.
        let cursor_for_add = self.cursor_offset.clone();
        let add_sub = mgr.observe_item_added(move |_txn, event| {
            if let Ok(cur) = cursor_for_add.lock() {
                event.meta_mut().cursor_offset = *cur;
            }
        });

        // Restore cursor offset from stack item metadata when an item is popped (undo/redo).
        let restored = self.restored_cursor.clone();
        let pop_sub = mgr.observe_item_popped(move |_txn, event| {
            if let Ok(mut r) = restored.lock() {
                *r = Some(event.meta().cursor_offset);
            }
        });

        self.undo_mgr = Some(mgr);
        self._update_sub = Some(sub);
        self._undo_subs = vec![add_sub, pop_sub];
    }

    /// The client ID of the underlying yrs document.
    pub fn client_id(&self) -> u64 {
        self.doc.client_id().get()
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

    /// Undo the last local operation. Returns an `UndoResult` with success flag,
    /// update bytes for broadcast, and the cursor offset to restore.
    pub fn undo(&mut self) -> UndoResult {
        let Some(mgr) = &mut self.undo_mgr else {
            return UndoResult {
                success: false,
                updates: Vec::new(),
                cursor_offset: None,
            };
        };
        // Clear state before undo.
        if let Ok(mut buf) = self.captured_updates.lock() {
            buf.clear();
        }
        if let Ok(mut r) = self.restored_cursor.lock() {
            *r = None;
        }
        let ok = mgr.undo_blocking();
        self.rebuild_rope();
        let updates = if let Ok(mut buf) = self.captured_updates.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        };
        let cursor_offset = self.restored_cursor.lock().ok().and_then(|r| *r);
        UndoResult {
            success: ok,
            updates,
            cursor_offset,
        }
    }

    /// Redo the last undone operation. Returns an `UndoResult` with success flag,
    /// update bytes for broadcast, and the cursor offset to restore.
    pub fn redo(&mut self) -> UndoResult {
        let Some(mgr) = &mut self.undo_mgr else {
            return UndoResult {
                success: false,
                updates: Vec::new(),
                cursor_offset: None,
            };
        };
        if let Ok(mut buf) = self.captured_updates.lock() {
            buf.clear();
        }
        if let Ok(mut r) = self.restored_cursor.lock() {
            *r = None;
        }
        let ok = mgr.redo_blocking();
        self.rebuild_rope();
        let updates = if let Ok(mut buf) = self.captured_updates.lock() {
            std::mem::take(&mut *buf)
        } else {
            Vec::new()
        };
        let cursor_offset = self.restored_cursor.lock().ok().and_then(|r| *r);
        UndoResult {
            success: ok,
            updates,
            cursor_offset,
        }
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
            mgr.clear_all();
        }
    }

    /// Set the maximum undo stack depth. Default is 1000.
    ///
    /// Note: yrs's UndoManager does not expose stack trimming via its public API,
    /// so this limit is currently advisory. It is stored for future enforcement
    /// when yrs adds support. In practice, StackItems are lightweight (IdSet pairs)
    /// so 1000 items is well within memory budget.
    pub fn set_undo_limit(&mut self, limit: usize) {
        self.undo_limit = limit;
    }

    /// Get the current undo stack depth limit.
    pub fn undo_limit(&self) -> usize {
        self.undo_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a realistic 32-bit client_id from (pid, buffer_index),
    /// mirroring `compute_client_id` in collab_bridge.rs.
    /// Uses FNV-1a to stay within the yrs v1 wire format's safe range.
    fn test_client_id(pid: u32, buf_idx: u32) -> u64 {
        let mut h: u32 = 0x811c_9dc5;
        for b in pid.to_le_bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        for b in buf_idx.to_le_bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        if h == 0 {
            1
        } else {
            h as u64
        }
    }

    // Realistic client_ids for a two-editor collab scenario.
    // PID 4_089_813 buf 2 = sharer, PID 4_089_541 buf 2 = joiner.
    fn sharer_id() -> u64 {
        test_client_id(4_089_813, 2)
    }
    fn joiner_id() -> u64 {
        test_client_id(4_089_541, 2)
    }

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
        let mut doc_a = TextSync::with_client_id("hello", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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
        let mut doc_a = TextSync::with_client_id("hello", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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
        let mut doc_a = TextSync::with_client_id("", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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
        let mut doc_a = TextSync::with_client_id("hello", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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

        let mut rng = rand::rng();
        let mut pending_updates: Vec<Vec<(usize, Vec<u8>)>> = vec![Vec::new(); 5];

        // Each doc does 200 random operations
        for _ in 0..200 {
            for i in 0..5 {
                let len = docs[i].content().len() as u32;
                if len == 0 || rng.random_bool(0.6) {
                    // Insert
                    let pos = if len == 0 {
                        0
                    } else {
                        rng.random_range(0..len)
                    };
                    let ch = (b'a' + rng.random_range(0..26u8)) as char;
                    let update = docs[i].insert(pos, &ch.to_string());
                    pending_updates[i].push((i, update));
                } else {
                    // Delete
                    let pos = rng.random_range(0..len);
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
        let result = ts.undo();
        assert!(result.success);
        assert_eq!(ts.content(), "hello");
        assert!(
            !result.updates.is_empty(),
            "undo should produce broadcast updates"
        );
    }

    #[test]
    fn redo_after_undo() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();
        ts.insert(5, " world");
        assert_eq!(ts.content(), "hello world");
        ts.undo();
        assert_eq!(ts.content(), "hello");
        let result = ts.redo();
        assert!(result.success);
        assert_eq!(ts.content(), "hello world");
        assert!(!result.updates.is_empty());
    }

    #[test]
    fn undo_produces_update_bytes() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        ts.insert(0, "abc");
        let result = ts.undo();
        // Updates should be non-empty and decodable.
        assert!(!result.updates.is_empty());
        for u in &result.updates {
            yrs::Update::decode_v1(u).expect("update bytes should be valid");
        }
    }

    #[test]
    fn undo_remote_excluded() {
        // Remote edits (no origin) should NOT be undone by local undo.
        let mut doc_a = TextSync::with_client_id("hello", sharer_id());
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", joiner_id());
        // Sync initial state from A to B.
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();

        // B inserts (remote from A's perspective).
        let remote_update = doc_b.insert(5, " world");
        doc_a.apply_update(&remote_update).unwrap();
        assert_eq!(doc_a.content(), "hello world");

        // A's undo should NOT undo B's edit (no local ops to undo).
        let result = doc_a.undo();
        assert!(!result.success, "nothing to undo — remote edits excluded");
        assert_eq!(doc_a.content(), "hello world");
    }

    #[test]
    fn redo_survives_remote_update() {
        // Verify that applying a remote update between undo and redo
        // does NOT clear the redo stack.
        let mut doc_a = TextSync::with_client_id("base\n", sharer_id());
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", joiner_id());
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
        let result = doc_a.undo();
        assert!(result.success, "A should be able to undo its insert");
        assert!(
            !doc_a.content().contains("from-A"),
            "from-A should be gone after undo"
        );
        assert!(
            doc_a.content().contains("from-B"),
            "from-B should survive A's undo"
        );

        // B undoes its own edit and sends the update to A (simulates remote undo)
        let b_result = doc_b.undo();
        assert!(b_result.success);
        for u in &b_result.updates {
            doc_a.apply_update(u).unwrap();
        }
        assert!(
            !doc_a.content().contains("from-B"),
            "from-B should be gone after B's undo"
        );

        // A redoes its own edit — this should work even after receiving B's remote undo
        let redo_result = doc_a.redo();
        assert!(
            redo_result.success,
            "A should be able to redo after remote update"
        );
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
        let mut doc_a = TextSync::with_client_id("base", sharer_id());
        doc_a.enable_undo();

        let mut doc_b = TextSync::with_client_id("", joiner_id());
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
        let result = doc_a.undo();
        assert!(result.success);
        assert!(
            doc_a.content().contains("-B"),
            "B's edit preserved after A's undo"
        );
        assert!(!doc_a.content().contains("-A"), "A's edit reversed");

        // Apply A's undo to B so they converge again.
        for u in &result.updates {
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
        let result = ts.undo();
        assert!(result.success);
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

    // --- UTF-16 offset correctness ---
    // yrs uses OffsetKind::Utf16. These tests verify that char offsets
    // (Rust Unicode scalar values) are correctly mapped to UTF-16 code units.

    #[test]
    fn utf16_insert_after_multibyte_bmp() {
        // BMP chars: é (U+00E9) is 1 UTF-16 unit but 2 UTF-8 bytes.
        let mut ts = TextSync::with_client_id("café latte", 1);
        // Char offset 5 = space after é. Insert should go between 'é' and ' '.
        ts.insert(4, "!");
        assert_eq!(ts.content(), "café! latte");
    }

    #[test]
    fn utf16_insert_after_supplementary_emoji() {
        // Supplementary plane: 🔥 (U+1F525) is 2 UTF-16 units but 1 Rust char.
        let mut ts = TextSync::with_client_id("a🔥b", 1);
        // Char offset 2 = 'b'. Insert between 🔥 and b.
        ts.insert(2, "X");
        assert_eq!(ts.content(), "a🔥Xb");
    }

    #[test]
    fn utf16_delete_multibyte_char() {
        let mut ts = TextSync::with_client_id("café", 1);
        // Delete é (char offset 3, length 1)
        ts.delete(3, 1);
        assert_eq!(ts.content(), "caf");
    }

    #[test]
    fn utf16_delete_emoji() {
        let mut ts = TextSync::with_client_id("a🔥🎉b", 1);
        // Delete 🔥 (char offset 1, length 1)
        ts.delete(1, 1);
        assert_eq!(ts.content(), "a🎉b");
    }

    #[test]
    fn utf16_insert_convergence_with_emoji() {
        // Two clients: A inserts after emoji, B receives update.
        let mut ts_a = TextSync::with_client_id("hello🔥world", sharer_id());
        let state = ts_a.encode_state();
        let mut ts_b = TextSync::from_state_with_client_id(&state, joiner_id()).unwrap();

        // A inserts at char offset 6 (after 🔥, before 'w')
        let update = ts_a.insert(6, "!!!");
        ts_b.apply_update(&update).unwrap();

        assert_eq!(ts_a.content(), "hello🔥!!!world");
        assert_eq!(ts_b.content(), "hello🔥!!!world");
    }

    #[test]
    fn utf16_reconcile_with_emoji() {
        let mut ts = TextSync::with_client_id("café 🔥 naïve", sharer_id());
        let state = ts.encode_state();
        let mut ts_b = TextSync::from_state_with_client_id(&state, joiner_id()).unwrap();

        // Reconcile to a new string with emoji changes
        let update = ts.reconcile_to("café 🎉 naïve");
        ts_b.apply_update(&update).unwrap();

        assert_eq!(ts.content(), "café 🎉 naïve");
        assert_eq!(ts_b.content(), "café 🎉 naïve");
    }

    #[test]
    fn utf16_zwj_family_emoji() {
        // ZWJ sequence: 👨‍👩‍👧‍👦 = 7 Rust chars (4 emoji + 3 ZWJ), 11 UTF-16 units
        let mut ts_a = TextSync::with_client_id("a👨\u{200d}👩\u{200d}👧\u{200d}👦b", sharer_id());
        let state = ts_a.encode_state();
        let mut ts_b = TextSync::from_state_with_client_id(&state, joiner_id()).unwrap();

        // Insert after the full ZWJ sequence (char 8 = 'b')
        let update = ts_a.insert(8, "X");
        ts_b.apply_update(&update).unwrap();

        assert_eq!(ts_a.content(), ts_b.content());
        assert!(ts_a.content().ends_with("Xb"));
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
        let result = ts.redo();
        assert!(!result.success, "redo should fail after new edit");
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
        let mut doc_a = TextSync::with_client_id("initial", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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
        let mut doc_a = TextSync::with_client_id("base", sharer_id());
        let mut doc_b = TextSync::with_client_id("", joiner_id());

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

    // --- Cursor metadata tests ---

    #[test]
    fn undo_restores_cursor_offset() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();

        // Set cursor at offset 5 before edit
        ts.set_cursor_offset(5);
        ts.insert(5, " world");
        assert_eq!(ts.content(), "hello world");

        // Undo should return the saved cursor offset
        let result = ts.undo();
        assert!(result.success);
        assert_eq!(ts.content(), "hello");
        assert_eq!(result.cursor_offset, Some(5));
    }

    #[test]
    fn undo_cursor_with_groups() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();

        // First group: insert "aaa" with cursor at 0
        ts.set_cursor_offset(0);
        ts.insert(0, "aaa");
        ts.undo_reset();

        // Second group: insert "bbb" with cursor at 3
        ts.set_cursor_offset(3);
        ts.insert(3, "bbb");
        assert_eq!(ts.content(), "aaabbb");

        // Undo second group → cursor should restore to 3
        let r1 = ts.undo();
        assert!(r1.success);
        assert_eq!(ts.content(), "aaa");
        assert_eq!(r1.cursor_offset, Some(3));

        // Undo first group → cursor should restore to 0
        let r2 = ts.undo();
        assert!(r2.success);
        assert_eq!(ts.content(), "");
        assert_eq!(r2.cursor_offset, Some(0));
    }

    #[test]
    fn redo_restores_cursor_offset() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();

        ts.set_cursor_offset(5);
        ts.insert(5, " world");
        ts.undo();

        let result = ts.redo();
        assert!(result.success);
        assert_eq!(ts.content(), "hello world");
        // Redo pops from redo stack — cursor offset comes from the popped item
        assert!(result.cursor_offset.is_some());
    }

    #[test]
    fn undo_limit_default() {
        let ts = TextSync::with_client_id("", 1);
        assert_eq!(ts.undo_limit(), DEFAULT_UNDO_LIMIT);
    }

    #[test]
    fn set_undo_limit() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.set_undo_limit(50);
        assert_eq!(ts.undo_limit(), 50);
    }

    // --- Error path coverage ---

    #[test]
    fn apply_update_truncated_bytes() {
        let mut ts = TextSync::new("hello");
        let result = ts.apply_update(&[1, 2, 3]);
        assert!(result.is_err());
        // Content unchanged after failed apply.
        assert_eq!(ts.content(), "hello");
    }

    #[test]
    fn apply_update_empty_bytes() {
        let mut ts = TextSync::new("hello");
        let result = ts.apply_update(&[]);
        assert!(result.is_err());
        assert_eq!(ts.content(), "hello");
    }

    #[test]
    fn from_state_corrupted_bytes() {
        let result = TextSync::from_state(&[0xFF, 0xFE, 0xAB]);
        assert!(result.is_err());
    }

    #[test]
    fn from_state_empty_bytes() {
        let result = TextSync::from_state(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn from_state_with_client_id_corrupted() {
        let result = TextSync::from_state_with_client_id(&[0xDE, 0xAD], 42);
        assert!(result.is_err());
    }

    #[test]
    fn large_client_id_survives_v1_encoding() {
        // yrs v0.22 had a bug where v1 encoding corrupted client_ids exceeding
        // ~32 bits. yrs v0.27 uses 53-bit ClientID, fixing this issue.
        let client_id: u64 = 268029984770; // (4089813 << 16) | 2
        let ts = TextSync::with_client_id("hello world", client_id);
        assert_eq!(
            ts.client_id(),
            client_id,
            "doc client_id should be correct in memory"
        );

        let state = ts.encode_state();
        let update = yrs::Update::decode_v1(&state).unwrap();
        let sv = update.state_vector();

        // yrs 0.27 preserves large client_ids through v1 encoding.
        for (&cid, &_clock) in sv.iter() {
            assert_eq!(
                cid.get(),
                client_id,
                "large client_id should survive v1 encoding in yrs 0.27+"
            );
        }
    }

    #[test]
    fn small_client_id_survives_encode_decode() {
        // Client IDs that fit in 32 bits survive the v1 encoding roundtrip.
        let client_id: u64 = 42_000_000;
        let ts = TextSync::with_client_id("hello", client_id);
        assert_eq!(ts.client_id(), client_id);

        let state = ts.encode_state();
        let update = yrs::Update::decode_v1(&state).unwrap();
        let sv = update.state_vector();

        for (&cid, &_clock) in sv.iter() {
            assert_eq!(
                cid.get(),
                client_id,
                "small client_id should survive v1 encoding"
            );
        }
    }

    // --- Reconcile edge cases ---

    #[test]
    fn reconcile_whitespace_only() {
        let mut ts = TextSync::new("   ");
        let update = ts.reconcile_to("\t\n ");
        assert!(!update.is_empty());
        assert_eq!(ts.content(), "\t\n ");
    }

    #[test]
    fn reconcile_very_long_single_line() {
        let long = "x".repeat(100_000);
        let mut ts = TextSync::new(&long);
        let changed = format!("{}y", &long[..99_999]);
        let update = ts.reconcile_to(&changed);
        assert!(!update.is_empty());
        assert_eq!(ts.content(), changed);
    }

    #[test]
    fn reconcile_mixed_line_endings() {
        let mut ts = TextSync::new("line1\nline2\r\nline3\r");
        let target = "line1\r\nline2\nline3\n";
        let update = ts.reconcile_to(target);
        assert!(!update.is_empty());
        assert_eq!(ts.content(), target);
    }

    // --- Undo/redo edge branches ---

    #[test]
    fn undo_on_empty_stack() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();
        // No edits made — undo should be no-op.
        let result = ts.undo();
        assert!(!result.success);
        assert!(result.updates.is_empty());
        assert_eq!(ts.content(), "hello");
    }

    #[test]
    fn redo_on_empty_stack() {
        let mut ts = TextSync::with_client_id("hello", 1);
        ts.enable_undo();
        let result = ts.redo();
        assert!(!result.success);
        assert!(result.updates.is_empty());
    }

    #[test]
    fn undo_after_remote_edit_no_crash() {
        let mut doc_a = TextSync::with_client_id("base", 1);
        doc_a.enable_undo();
        let mut doc_b = TextSync::with_client_id("", 2);
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();

        // A edits, then B edits remotely.
        doc_a.insert(4, "A");
        let remote = doc_b.insert(4, "B");
        doc_a.apply_update(&remote).unwrap();

        // A's undo should only undo A's edit.
        let result = doc_a.undo();
        assert!(result.success);
        assert!(doc_a.content().contains('B'));
        assert!(!doc_a.content().contains('A'));
    }

    #[test]
    fn undo_reset_then_undo() {
        let mut ts = TextSync::with_client_id("", 1);
        ts.enable_undo();
        ts.insert(0, "abc");
        ts.undo_reset();
        // Undo resets to group boundary — should undo the "abc" group.
        let result = ts.undo();
        assert!(result.success);
        assert_eq!(ts.content(), "");
    }

    #[test]
    fn undo_without_enable() {
        let mut ts = TextSync::with_client_id("hello", 1);
        // No enable_undo() — undo should return not-success without panic.
        let result = ts.undo();
        assert!(!result.success);
    }

    // --- content() and encode_diff edge cases ---

    #[test]
    fn content_of_empty_doc() {
        let ts = TextSync::new("");
        assert_eq!(ts.content(), "");
    }

    #[test]
    fn encode_diff_identical_state_vectors() {
        let doc_a = TextSync::with_client_id("hello", 1);
        let sv = doc_a.state_vector();
        let diff = doc_a.encode_diff(&sv);
        // Diff for identical SVs should be a minimal (possibly empty) yrs update.
        // Apply it to another doc — should produce same content.
        let mut doc_b = TextSync::with_client_id("", 2);
        let state = doc_a.encode_state();
        doc_b.apply_update(&state).unwrap();
        let sv_b = doc_b.state_vector();
        let diff_b = doc_a.encode_diff(&sv_b);
        // Both should be very small since SVs are aligned.
        assert!(diff.len() < 100, "diff should be small for aligned SVs");
        assert!(diff_b.len() < 100);
    }

    #[test]
    fn encode_diff_corrupted_sv() {
        let doc = TextSync::with_client_id("hello", 1);
        // Corrupted SV falls back to default (sends full state).
        let diff = doc.encode_diff(&[0xFF, 0x00, 0xAB]);
        assert!(!diff.is_empty());
        // Should be decodable.
        let mut doc_b = TextSync::with_client_id("", 2);
        doc_b.apply_update(&diff).unwrap();
        assert_eq!(doc_b.content(), "hello");
    }

    // --- Multi-group undo with cursor ---

    #[test]
    fn undo_multi_group_cursor_insert_delete() {
        let mut ts = TextSync::with_client_id("hello world", 1);
        ts.enable_undo();

        // Group 1: insert at end.
        ts.set_cursor_offset(11);
        ts.insert(11, "!!!");
        ts.undo_reset();

        // Group 2: delete "world".
        ts.set_cursor_offset(5);
        ts.delete(6, 5); // "hello !!!"
        assert_eq!(ts.content(), "hello !!!");

        // Undo group 2 (delete) — restores "world".
        let r1 = ts.undo();
        assert!(r1.success);
        assert_eq!(ts.content(), "hello world!!!");
        assert_eq!(r1.cursor_offset, Some(5));

        // Undo group 1 (insert "!!!").
        let r2 = ts.undo();
        assert!(r2.success);
        assert_eq!(ts.content(), "hello world");
        assert_eq!(r2.cursor_offset, Some(11));
    }

    /// Reproduce the exact share→join→joiner-edits→apply-to-sharer flow
    /// that fails in real smoke tests (joiner's updates are no-ops on sharer).
    #[test]
    fn share_join_roundtrip_bidirectional() {
        // 1. Sharer creates Doc with content, encodes full state.
        let mut sharer = TextSync::with_client_id("hello world\n", sharer_id());
        let share_state = sharer.encode_state();

        // 2. Server receives share — create a server Doc from the state.
        let mut server = TextSync::from_state(&share_state).unwrap();
        assert_eq!(server.content(), "hello world\n");

        // 3. Joiner requests resync — server sends full state.
        let server_state = server.encode_state();

        // 4. Joiner creates Doc from server state with different client_id.
        let mut joiner = TextSync::from_state_with_client_id(&server_state, joiner_id()).unwrap();
        assert_eq!(joiner.content(), "hello world\n");

        // 5. Joiner types — generates update.
        let joiner_update = joiner.insert(12, "from joiner\n");
        assert_eq!(joiner.content(), "hello world\nfrom joiner\n");

        // 6. Server applies joiner's update.
        server.apply_update(&joiner_update).unwrap();
        assert_eq!(server.content(), "hello world\nfrom joiner\n");

        // 7. Server broadcasts joiner's update bytes to sharer.
        //    (Server sends the EXACT same bytes — see doc_store.rs line 238)
        let before = sharer.content();
        sharer.apply_update(&joiner_update).unwrap();
        let after = sharer.content();

        // THIS IS THE CRITICAL ASSERTION: sharer must see joiner's edit.
        assert_ne!(
            before, after,
            "sharer content must change after joiner's update"
        );
        assert_eq!(
            sharer.content(),
            "hello world\nfrom joiner\n",
            "sharer must converge with joiner"
        );

        // 8. Sharer types — generates update.
        let sharer_update = sharer.insert(24, "from sharer\n");
        assert_eq!(sharer.content(), "hello world\nfrom joiner\nfrom sharer\n");

        // 9. Server applies, broadcasts to joiner.
        server.apply_update(&sharer_update).unwrap();
        joiner.apply_update(&sharer_update).unwrap();
        assert_eq!(joiner.content(), "hello world\nfrom joiner\nfrom sharer\n");
        assert_eq!(server.content(), joiner.content());
        assert_eq!(server.content(), sharer.content());
    }

    /// Reproduce the actual server flow: sharer shares full state,
    /// server creates Doc, joiner gets server state, both edit.
    /// Uses base64 encoding to match the real transport.
    #[test]
    fn share_join_via_server_doc_roundtrip() {
        use crate::encoding::{base64_to_update, update_to_base64};

        // 1. Sharer: enable_sync equivalent.
        let mut sharer = TextSync::with_client_id("hello world\n", sharer_id());
        sharer.enable_undo();

        // 2. Sharer types before sharing (common in real use).
        let _ = sharer.insert(12, "typed before share\n");
        assert_eq!(sharer.content(), "hello world\ntyped before share\n");

        // 3. Share: encode full state, base64, send to server.
        let share_state = sharer.encode_state();
        let share_b64 = update_to_base64(&share_state);
        let share_decoded = base64_to_update(&share_b64).unwrap();

        // 4. Server: create Doc from share state (share_doc path).
        let mut server = TextSync::from_state(&share_decoded).unwrap();
        assert_eq!(server.content(), "hello world\ntyped before share\n");

        // 5. Sharer types AFTER sharing — sends incremental updates.
        let update1 = sharer.insert(31, "after share\n");
        let u1_b64 = update_to_base64(&update1);
        let u1_decoded = base64_to_update(&u1_b64).unwrap();
        server.apply_update(&u1_decoded).unwrap();
        assert_eq!(
            server.content(),
            "hello world\ntyped before share\nafter share\n"
        );

        // 6. Joiner: sync/resync — gets server full state.
        let server_state = server.encode_state();
        let server_b64 = update_to_base64(&server_state);
        let server_decoded = base64_to_update(&server_b64).unwrap();

        let mut joiner = TextSync::from_state_with_client_id(&server_decoded, joiner_id()).unwrap();
        joiner.enable_undo();
        assert_eq!(
            joiner.content(),
            "hello world\ntyped before share\nafter share\n"
        );

        // 7. Joiner types — sends incremental update via server.
        let joiner_update = joiner.insert(43, "joiner here\n");
        let ju_b64 = update_to_base64(&joiner_update);
        let ju_decoded = base64_to_update(&ju_b64).unwrap();

        // 8. Server applies and broadcasts same bytes.
        server.apply_update(&ju_decoded).unwrap();

        // 9. Sharer receives same bytes.
        let before = sharer.content();
        sharer.apply_update(&ju_decoded).unwrap();
        let after = sharer.content();

        assert_ne!(before, after, "sharer must see joiner's edit");
        assert!(
            after.contains("joiner here"),
            "sharer must have joiner's text"
        );
        assert_eq!(
            sharer.content(),
            server.content(),
            "sharer must match server"
        );
        assert_eq!(sharer.content(), joiner.content(), "all must converge");
    }

    /// Same as above but sharer edits BEFORE the joiner joins,
    /// simulating the real scenario where sharer types, shares, then joiner joins.
    #[test]
    fn share_with_pre_edits_then_join_roundtrip() {
        // 1. Sharer creates Doc, types some content, then shares.
        let mut sharer = TextSync::with_client_id("initial\n", sharer_id());
        let _edit1 = sharer.insert(8, "sharer typed this\n");
        assert_eq!(sharer.content(), "initial\nsharer typed this\n");
        let share_state = sharer.encode_state();

        // 2. Server gets full state.
        let mut server = TextSync::from_state(&share_state).unwrap();

        // 3. Sharer types MORE after sharing (these updates go to server).
        let sharer_update2 = sharer.insert(26, "more sharer text\n");
        server.apply_update(&sharer_update2).unwrap();
        assert_eq!(
            server.content(),
            "initial\nsharer typed this\nmore sharer text\n"
        );

        // 4. Joiner joins — gets server's full state (includes all sharer edits).
        let server_state = server.encode_state();
        let mut joiner = TextSync::from_state_with_client_id(&server_state, joiner_id()).unwrap();
        assert_eq!(
            joiner.content(),
            "initial\nsharer typed this\nmore sharer text\n"
        );

        // 5. Joiner types.
        let joiner_update = joiner.insert(43, "joiner reply\n");

        // 6. Server applies, broadcasts to sharer.
        server.apply_update(&joiner_update).unwrap();
        let before = sharer.content();
        sharer.apply_update(&joiner_update).unwrap();
        let after = sharer.content();

        assert_ne!(
            before, after,
            "sharer must see joiner's edit even after pre-share edits"
        );
        assert!(
            after.contains("joiner reply"),
            "sharer must contain joiner's text"
        );
        // All three must converge.
        assert_eq!(sharer.content(), server.content());
        assert_eq!(sharer.content(), joiner.content());
    }
}
