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
use std::collections::HashMap;
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
}

impl EditorEvent {
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
        }
    }
}

/// Default per-client event queue capacity.
const DEFAULT_QUEUE_CAPACITY: usize = 100;

/// Manages per-client event channels.
pub struct EventBroadcaster {
    /// Map of session_id → (subscriptions, sender).
    clients: HashMap<u64, (Vec<String>, mpsc::Sender<EditorEvent>)>,
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
        self.clients.insert(session_id, (subscriptions, tx));
        rx
    }

    /// Remove a client's subscription (on disconnect).
    pub fn unsubscribe(&mut self, session_id: u64) {
        self.clients.remove(&session_id);
    }

    /// Update a client's subscription list.
    pub fn update_subscriptions(&mut self, session_id: u64, subscriptions: Vec<String>) {
        if let Some((subs, _)) = self.clients.get_mut(&session_id) {
            *subs = subscriptions;
        }
    }

    /// Broadcast an event to all subscribed clients.
    /// Uses `try_send` — if a client's queue is full, the event is dropped
    /// for that client (backpressure). Dead channels (closed receivers) are
    /// automatically cleaned up.
    pub fn broadcast(&mut self, event: &EditorEvent) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let event_type = event.event_type();
        debug!(seq = seq, event_type = event_type, "broadcasting event");
        let mut closed: Vec<u64> = Vec::new();
        for (session_id, (subs, tx)) in &self.clients {
            if subs.iter().any(|s| s == event_type || s == "*") {
                match tx.try_send(event.clone()) {
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            session_id = session_id,
                            event_type = event_type,
                            "client event queue full; dropping event"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        debug!(session_id = session_id, "removing closed client channel");
                        closed.push(*session_id);
                    }
                    Ok(()) => {}
                }
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
        let event_type = event.event_type();
        debug!(
            seq = seq,
            event_type = event_type,
            exclude = exclude_session,
            "broadcasting event (with exclusion)"
        );
        let mut closed: Vec<u64> = Vec::new();
        for (session_id, (subs, tx)) in &self.clients {
            if *session_id == exclude_session {
                continue;
            }
            if subs.iter().any(|s| s == event_type || s == "*") {
                match tx.try_send(event.clone()) {
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        warn!(
                            session_id = session_id,
                            event_type = event_type,
                            "client event queue full; dropping event"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        debug!(session_id = session_id, "removing closed client channel");
                        closed.push(*session_id);
                    }
                    Ok(()) => {}
                }
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
}
