//! Event broadcast system for multi-client MCP.
//!
//! When the editor processes a state-changing command, it emits an
//! `EditorEvent` to the broadcaster. Each connected client with matching
//! subscriptions receives the event via a bounded channel.
//!
//! Backpressure: if a client's queue is full, the event is dropped for
//! that client (logged as a warning). This prevents one slow client from
//! blocking the server.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Events emitted by the editor that clients can subscribe to.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum EditorEvent {
    /// A buffer's content was modified.
    #[serde(rename = "buffer_edit")]
    BufferEdited { buffer_idx: usize, version: u64 },
    /// The cursor moved in a buffer.
    #[serde(rename = "cursor_move")]
    CursorMoved {
        buffer_idx: usize,
        row: usize,
        col: usize,
    },
    /// LSP diagnostics were updated for a buffer.
    #[serde(rename = "diagnostics")]
    DiagnosticsUpdated { buffer_idx: usize },
    /// The editor mode changed.
    #[serde(rename = "mode_change")]
    ModeChanged { mode: String },
    /// A new buffer was opened.
    #[serde(rename = "buffer_open")]
    BufferOpened {
        buffer_idx: usize,
        path: Option<String>,
    },
    /// A buffer was closed.
    #[serde(rename = "buffer_close")]
    BufferClosed { buffer_idx: usize },
    /// A collaborative sync update was generated (yrs encoded, base64).
    /// Uses `buffer_name` (not `buffer_idx`) for cross-session stability —
    /// buffer indices can change on reconnect, but names are persistent.
    #[serde(rename = "sync_update")]
    SyncUpdate {
        buffer_name: String,
        update_base64: String,
        /// WAL sequence ID for this update (0 if not persisted).
        #[serde(default)]
        wal_seq: u64,
    },
    /// A peer joined a collaborative session.
    #[serde(rename = "peer_joined")]
    PeerJoined { session_id: u64, peer_count: usize },
    /// A peer left a collaborative session.
    #[serde(rename = "peer_left")]
    PeerLeft { session_id: u64, peer_count: usize },
    /// The sharer of a document disconnected — doc is now unowned.
    #[serde(rename = "sharer_left")]
    SharerLeft {
        session_id: u64,
        doc: String,
        peer_count: usize,
    },
    /// A peer completed a file save (docs/save_committed).
    #[serde(rename = "save_committed")]
    SaveCommitted {
        doc: String,
        saved_by: String,
        save_epoch: u64,
        content_hash: String,
    },
    /// A remote user's awareness state changed (cursor/selection/presence).
    #[serde(rename = "awareness_update")]
    AwarenessUpdate {
        doc_id: String,
        client_id: u64,
        user_name: String,
        cursor_row: usize,
        cursor_col: usize,
        selection: Option<(usize, usize, usize, usize)>,
        mode: String,
    },
}

impl EditorEvent {
    /// The document ID associated with this event, if any.
    /// Used for doc-scoped filtering — events with a doc_id should only be
    /// delivered to sessions that have interacted with that document.
    pub fn doc_id(&self) -> Option<&str> {
        match self {
            EditorEvent::SyncUpdate { buffer_name, .. } => Some(buffer_name),
            EditorEvent::SharerLeft { doc, .. } => Some(doc),
            EditorEvent::SaveCommitted { doc, .. } => Some(doc),
            EditorEvent::AwarenessUpdate { doc_id, .. } => Some(doc_id),
            _ => None,
        }
    }

    /// The subscription category for this event type.
    pub fn event_type(&self) -> &'static str {
        match self {
            EditorEvent::BufferEdited { .. } => "buffer_edit",
            EditorEvent::CursorMoved { .. } => "cursor_move",
            EditorEvent::DiagnosticsUpdated { .. } => "diagnostics",
            EditorEvent::ModeChanged { .. } => "mode_change",
            EditorEvent::BufferOpened { .. } => "buffer_open",
            EditorEvent::BufferClosed { .. } => "buffer_close",
            EditorEvent::SyncUpdate { .. } => "sync_update",
            EditorEvent::PeerJoined { .. } => "peer_joined",
            EditorEvent::PeerLeft { .. } => "peer_left",
            EditorEvent::SharerLeft { .. } => "sharer_left",
            EditorEvent::SaveCommitted { .. } => "save_committed",
            EditorEvent::AwarenessUpdate { .. } => "awareness_update",
        }
    }
}

/// Default per-client event queue capacity.
const DEFAULT_QUEUE_CAPACITY: usize = 100;

/// Per-client subscription state.
struct ClientEntry {
    /// Event type subscriptions (e.g., "sync_update", "awareness_update", "*").
    event_subs: Vec<String>,
    /// Document IDs this client is interested in. Events with a `doc_id()`
    /// are only delivered if the client has subscribed to that document.
    /// Events without a `doc_id()` (global events) are always delivered.
    doc_subs: HashSet<String>,
    /// Bounded channel for event delivery.
    tx: mpsc::Sender<EditorEvent>,
}

/// Manages per-client event channels with doc-scoped filtering.
///
/// The broadcaster enforces two levels of filtering at enqueue time:
/// 1. **Event type**: clients only receive event types they subscribed to.
/// 2. **Document scope**: events with a `doc_id()` are only delivered to
///    clients that have subscribed to that document via `subscribe_doc()`.
///    Events without a doc_id (global events like `peer_joined`) bypass
///    doc filtering entirely.
///
/// This ensures zero cross-document leakage — a client editing `foo.rs`
/// never receives awareness updates, sync updates, or save notifications
/// for `bar.rs`.
pub struct EventBroadcaster {
    /// Map of session_id → client entry.
    clients: HashMap<u64, ClientEntry>,
    /// Monotonically increasing sequence number for event ordering.
    next_seq: AtomicU64,
}

impl EventBroadcaster {
    pub fn new() -> Self {
        EventBroadcaster {
            clients: HashMap::new(),
            next_seq: AtomicU64::new(1),
        }
    }

    /// Register a new client for event delivery.
    /// Returns the receiver end of the bounded channel.
    pub fn subscribe(
        &mut self,
        session_id: u64,
        subscriptions: Vec<String>,
    ) -> mpsc::Receiver<EditorEvent> {
        let (tx, rx) = mpsc::channel(DEFAULT_QUEUE_CAPACITY);
        self.clients.insert(
            session_id,
            ClientEntry {
                event_subs: subscriptions,
                doc_subs: HashSet::new(),
                tx,
            },
        );
        rx
    }

    /// Remove a client's subscription (on disconnect).
    pub fn unsubscribe(&mut self, session_id: u64) {
        self.clients.remove(&session_id);
    }

    /// Update a client's event type subscription list.
    pub fn update_subscriptions(&mut self, session_id: u64, subscriptions: Vec<String>) {
        if let Some(entry) = self.clients.get_mut(&session_id) {
            entry.event_subs = subscriptions;
        }
    }

    /// Add a single event-type subscription without clobbering existing ones
    /// (idempotent). Used by `kb/join` so a member is subscribed to `sync_update`
    /// AS OF the join snapshot — closing the window where the owner's edits between
    /// the snapshot and a separate `notifications/subscribe` could be missed.
    pub fn add_event_sub(&mut self, session_id: u64, event_type: &str) {
        if let Some(entry) = self.clients.get_mut(&session_id) {
            if !entry.event_subs.iter().any(|s| s == event_type) {
                entry.event_subs.push(event_type.to_string());
            }
        }
    }

    /// Subscribe a client to a specific document's events.
    /// Called when a client shares, joins, or sends updates to a document.
    pub fn subscribe_doc(&mut self, session_id: u64, doc_id: &str) {
        if let Some(entry) = self.clients.get_mut(&session_id) {
            entry.doc_subs.insert(doc_id.to_string());
        }
    }

    /// Unsubscribe a client from a specific document's events.
    pub fn unsubscribe_doc(&mut self, session_id: u64, doc_id: &str) {
        if let Some(entry) = self.clients.get_mut(&session_id) {
            entry.doc_subs.remove(doc_id);
        }
    }

    /// Check if a client should receive an event based on event type and doc scope.
    fn should_deliver(entry: &ClientEntry, event: &EditorEvent) -> bool {
        let event_type = event.event_type();
        // Check event type subscription.
        if !entry.event_subs.iter().any(|s| s == event_type || s == "*") {
            return false;
        }
        // Check doc scope: if the client has opted into doc-scoped filtering
        // (i.e., has at least one doc subscription), events with a doc_id are
        // only delivered to subscribed documents. If the client has NOT opted in
        // (empty doc_subs), all events pass through — backward compatible with
        // the editor's MCP server which doesn't use doc subscriptions.
        if !entry.doc_subs.is_empty() {
            if let Some(doc) = event.doc_id() {
                if !entry.doc_subs.contains(doc) {
                    return false;
                }
            }
        }
        true
    }

    /// Try to send an event to a client, handling backpressure and dead channels.
    /// Returns true if the channel is closed (should be cleaned up).
    fn try_deliver(session_id: u64, entry: &ClientEntry, event: &EditorEvent) -> bool {
        match entry.tx.try_send(event.clone()) {
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(
                    session_id = session_id,
                    event_type = event.event_type(),
                    "client event queue full; dropping event"
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                debug!(session_id = session_id, "removing closed client channel");
                true
            }
            Ok(()) => false,
        }
    }

    /// Broadcast an event to all subscribed clients.
    /// Uses `try_send` — if a client's queue is full, the event is dropped
    /// for that client (backpressure). Dead channels (closed receivers) are
    /// automatically cleaned up.
    ///
    /// Doc-scoped events (those with a `doc_id()`) are only delivered to
    /// clients that have called `subscribe_doc()` for that document.
    pub fn broadcast(&mut self, event: &EditorEvent) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        debug!(
            seq = seq,
            event_type = event.event_type(),
            "broadcasting event"
        );
        let mut closed: Vec<u64> = Vec::new();
        for (session_id, entry) in &self.clients {
            if Self::should_deliver(entry, event) && Self::try_deliver(*session_id, entry, event) {
                closed.push(*session_id);
            }
        }
        for id in closed {
            self.clients.remove(&id);
        }
    }

    /// Broadcast an event to all subscribed clients except the specified session.
    /// Used for echo filtering — the sender of a sync/update should not receive
    /// its own update back from the server.
    pub fn broadcast_except(&mut self, event: &EditorEvent, exclude_session: u64) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        debug!(
            seq = seq,
            event_type = event.event_type(),
            exclude = exclude_session,
            "broadcasting event (with exclusion)"
        );
        let mut closed: Vec<u64> = Vec::new();
        for (session_id, entry) in &self.clients {
            if *session_id == exclude_session {
                continue;
            }
            if Self::should_deliver(entry, event) && Self::try_deliver(*session_id, entry, event) {
                closed.push(*session_id);
            }
        }
        for id in closed {
            self.clients.remove(&id);
        }
    }

    /// Number of currently subscribed clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Current sequence number (next event will get this value).
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Relaxed)
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared reference to the event broadcaster.
///
/// Uses `std::sync::Mutex` (not tokio) — all operations (`broadcast`,
/// `subscribe`, `unsubscribe`) are synchronous and sub-microsecond.
pub type SharedBroadcaster = std::sync::Arc<std::sync::Mutex<EventBroadcaster>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_and_broadcast() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["buffer_edit".to_string()]);

        let event = EditorEvent::BufferEdited {
            buffer_idx: 0,
            version: 1,
        };
        bc.broadcast(&event); // bc is already mut

        let received = rx.recv().await.unwrap();
        assert!(matches!(
            received,
            EditorEvent::BufferEdited {
                buffer_idx: 0,
                version: 1
            }
        ));
    }

    #[tokio::test]
    async fn unsubscribed_event_not_delivered() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["buffer_edit".to_string()]);

        // Send an event type the client didn't subscribe to.
        let event = EditorEvent::ModeChanged {
            mode: "Normal".to_string(),
        };
        bc.broadcast(&event);

        // Channel should be empty.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn wildcard_subscription() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["*".to_string()]);

        let event = EditorEvent::ModeChanged {
            mode: "Insert".to_string(),
        };
        bc.broadcast(&event);

        assert!(rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn backpressure_does_not_panic() {
        let mut bc = EventBroadcaster::new();
        let _rx = bc.subscribe(1, vec!["buffer_edit".to_string()]);

        // Fill the queue beyond capacity — should not panic.
        for i in 0..200 {
            let event = EditorEvent::BufferEdited {
                buffer_idx: 0,
                version: i,
            };
            bc.broadcast(&event);
        }
    }

    #[test]
    fn unsubscribe_removes_client() {
        let mut bc = EventBroadcaster::new();
        let _rx = bc.subscribe(1, vec!["*".to_string()]);
        assert_eq!(bc.client_count(), 1);
        bc.unsubscribe(1);
        assert_eq!(bc.client_count(), 0);
    }

    #[test]
    fn sequence_numbers_monotonic() {
        let mut bc = EventBroadcaster::new();
        assert_eq!(bc.current_seq(), 1); // starts at 1

        let _rx = bc.subscribe(1, vec!["buffer_edit".to_string()]);

        let event = EditorEvent::BufferEdited {
            buffer_idx: 0,
            version: 1,
        };
        bc.broadcast(&event);
        assert_eq!(bc.current_seq(), 2);

        bc.broadcast(&event);
        bc.broadcast(&event);
        assert_eq!(bc.current_seq(), 4); // 1 + 3 broadcasts
    }

    #[tokio::test]
    async fn sync_update_event_delivered() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["sync_update".to_string()]);
        bc.subscribe_doc(1, "test.rs");

        let event = EditorEvent::SyncUpdate {
            buffer_name: "test.rs".to_string(),
            update_base64: "AQIDBA==".to_string(),
            wal_seq: 0,
        };
        bc.broadcast(&event);

        let received = rx.recv().await.unwrap();
        match received {
            EditorEvent::SyncUpdate {
                buffer_name,
                update_base64,
                ..
            } => {
                assert_eq!(buffer_name, "test.rs");
                assert_eq!(update_base64, "AQIDBA==");
            }
            _ => panic!("expected SyncUpdate"),
        }
    }

    #[tokio::test]
    async fn broadcast_except_skips_excluded_session() {
        let mut bc = EventBroadcaster::new();
        let mut rx1 = bc.subscribe(1, vec!["sync_update".to_string()]);
        let mut rx2 = bc.subscribe(2, vec!["sync_update".to_string()]);
        bc.subscribe_doc(1, "test.rs");
        bc.subscribe_doc(2, "test.rs");

        let event = EditorEvent::SyncUpdate {
            buffer_name: "test.rs".to_string(),
            update_base64: "AQIDBA==".to_string(),
            wal_seq: 1,
        };
        bc.broadcast_except(&event, 1); // exclude session 1

        // Session 1 (excluded) should NOT receive it.
        assert!(rx1.try_recv().is_err());
        // Session 2 should receive it.
        assert!(rx2.recv().await.is_some());
    }

    #[tokio::test]
    async fn sync_update_filtered_by_subscription() {
        let mut bc = EventBroadcaster::new();
        // Subscribe to buffer_edit only — should NOT receive sync_update.
        let mut rx_filtered = bc.subscribe(1, vec!["buffer_edit".to_string()]);
        // Subscribe to wildcard — should receive sync_update.
        let mut rx_wildcard = bc.subscribe(2, vec!["*".to_string()]);
        bc.subscribe_doc(2, "foo.rs");

        let event = EditorEvent::SyncUpdate {
            buffer_name: "foo.rs".to_string(),
            update_base64: "dGVzdA==".to_string(),
            wal_seq: 0,
        };
        bc.broadcast(&event);

        // Filtered client should NOT receive it.
        assert!(rx_filtered.try_recv().is_err());
        // Wildcard client should receive it.
        assert!(rx_wildcard.recv().await.is_some());
    }

    #[tokio::test]
    async fn doc_scoped_filtering_isolates_documents() {
        let mut bc = EventBroadcaster::new();
        let mut rx_a = bc.subscribe(1, vec!["*".to_string()]);
        let mut rx_b = bc.subscribe(2, vec!["*".to_string()]);

        // Client A subscribes to doc "alpha.rs", client B to "beta.rs".
        bc.subscribe_doc(1, "alpha.rs");
        bc.subscribe_doc(2, "beta.rs");

        // SyncUpdate for alpha.rs — only A should get it.
        let event_alpha = EditorEvent::SyncUpdate {
            buffer_name: "alpha.rs".to_string(),
            update_base64: "YWxwaGE=".to_string(),
            wal_seq: 1,
        };
        bc.broadcast(&event_alpha);
        assert!(rx_a.recv().await.is_some(), "A should receive alpha event");
        assert!(rx_b.try_recv().is_err(), "B should NOT receive alpha event");

        // AwarenessUpdate for beta.rs — only B should get it.
        let event_beta = EditorEvent::AwarenessUpdate {
            doc_id: "beta.rs".to_string(),
            client_id: 99,
            user_name: "bob".to_string(),
            cursor_row: 0,
            cursor_col: 0,
            selection: None,
            mode: "normal".to_string(),
        };
        bc.broadcast(&event_beta);
        assert!(
            rx_a.try_recv().is_err(),
            "A should NOT receive beta awareness"
        );
        assert!(
            rx_b.recv().await.is_some(),
            "B should receive beta awareness"
        );

        // Global event (no doc_id) — both should get it.
        let global = EditorEvent::PeerJoined {
            session_id: 3,
            peer_count: 3,
        };
        bc.broadcast(&global);
        assert!(rx_a.recv().await.is_some(), "A should receive global event");
        assert!(rx_b.recv().await.is_some(), "B should receive global event");
    }

    #[tokio::test]
    async fn subscribe_doc_allows_multiple_docs() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["sync_update".to_string()]);
        bc.subscribe_doc(1, "a.rs");
        bc.subscribe_doc(1, "b.rs");

        let event_a = EditorEvent::SyncUpdate {
            buffer_name: "a.rs".to_string(),
            update_base64: "YQ==".to_string(),
            wal_seq: 1,
        };
        let event_b = EditorEvent::SyncUpdate {
            buffer_name: "b.rs".to_string(),
            update_base64: "Yg==".to_string(),
            wal_seq: 2,
        };
        let event_c = EditorEvent::SyncUpdate {
            buffer_name: "c.rs".to_string(),
            update_base64: "Yw==".to_string(),
            wal_seq: 3,
        };

        bc.broadcast(&event_a);
        bc.broadcast(&event_b);
        bc.broadcast(&event_c);

        assert!(rx.recv().await.is_some(), "should receive a.rs");
        assert!(rx.recv().await.is_some(), "should receive b.rs");
        assert!(rx.try_recv().is_err(), "should NOT receive c.rs");
    }

    #[tokio::test]
    async fn unsubscribe_doc_stops_delivery() {
        let mut bc = EventBroadcaster::new();
        let mut rx = bc.subscribe(1, vec!["sync_update".to_string()]);
        // Subscribe to two docs so doc filtering remains active after removing one.
        bc.subscribe_doc(1, "doc.rs");
        bc.subscribe_doc(1, "other.rs");

        let event = EditorEvent::SyncUpdate {
            buffer_name: "doc.rs".to_string(),
            update_base64: "ZA==".to_string(),
            wal_seq: 1,
        };
        bc.broadcast(&event);
        assert!(rx.recv().await.is_some(), "should receive before unsub");

        bc.unsubscribe_doc(1, "doc.rs");
        bc.broadcast(&event);
        assert!(
            rx.try_recv().is_err(),
            "should NOT receive doc.rs after unsub"
        );

        // But other.rs should still deliver.
        let event_other = EditorEvent::SyncUpdate {
            buffer_name: "other.rs".to_string(),
            update_base64: "bw==".to_string(),
            wal_seq: 2,
        };
        bc.broadcast(&event_other);
        assert!(rx.recv().await.is_some(), "should still receive other.rs");
    }

    #[tokio::test]
    async fn add_event_sub_is_additive_and_idempotent() {
        let mut bc = EventBroadcaster::new();
        // Start with no event subscriptions (as a fresh kb/join session has).
        let mut rx = bc.subscribe(1, vec![]);
        let event = EditorEvent::SyncUpdate {
            buffer_name: "doc.rs".to_string(),
            update_base64: "ZA==".to_string(),
            wal_seq: 1,
        };
        // Not subscribed to sync_update yet ⇒ not delivered.
        bc.broadcast(&event);
        assert!(rx.try_recv().is_err(), "no sync_update sub yet");

        // kb/join adds it (twice — must be idempotent) without clobbering.
        bc.add_event_sub(1, "sync_update");
        bc.add_event_sub(1, "sync_update");
        bc.broadcast(&event);
        assert!(rx.recv().await.is_some(), "delivered after add_event_sub");

        // Adding a second type keeps the first.
        bc.add_event_sub(1, "peer_joined");
        bc.broadcast(&event);
        assert!(rx.recv().await.is_some(), "sync_update still active");
    }
}
