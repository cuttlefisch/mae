//! MCP client session management.
//!
//! Each connected MCP client gets a `ClientSession` that tracks
//! its lifecycle, capabilities, and subscriptions.
//!
//! @ai-caution: Sync methods (`sync/state_vector`, `sync/update`,
//! `sync/full_state`, `sync/enable`) are implemented in `sync_exec.rs`.
//! Awareness/presence (`sync/awareness`) is a future phase (not yet started).
//! Do not remove `handle_request` match arms or session fields without
//! checking the sync roadmap (ADR-006).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Unique session identifier (monotonically increasing per server lifetime).
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// Information about a connected MCP client.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub version: Option<String>,
}

/// Per-client session state.
pub struct ClientSession {
    /// Unique session ID (server-scoped, not globally unique).
    pub id: u64,
    /// Client identification from the `initialize` handshake.
    pub client_info: Option<ClientInfo>,
    /// Whether the client has completed the initialize handshake.
    pub initialized: bool,
    /// Event types this client has subscribed to.
    /// Note: this is an informational copy for `$/resync`/`$/health` responses.
    /// The EventBroadcaster holds the authoritative subscription list for
    /// actual event delivery. Both are updated in `notifications/subscribe`.
    pub subscriptions: HashSet<String>,
    /// When this client connected.
    pub connected_at: Instant,
    /// Last activity timestamp (updated on every message).
    pub last_activity: Instant,
    /// Total messages received from this client.
    pub messages_received: u64,
    /// Total messages sent to this client.
    pub messages_sent: u64,
    /// Total tool calls dispatched for this client.
    pub tool_calls: u64,
    /// Total events delivered to this client.
    pub events_delivered: u64,
    /// Total events dropped due to backpressure.
    pub events_dropped: u64,
}

impl ClientSession {
    pub fn new() -> Self {
        let now = Instant::now();
        ClientSession {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            client_info: None,
            initialized: false,
            subscriptions: HashSet::new(),
            connected_at: now,
            last_activity: now,
            messages_received: 0,
            messages_sent: 0,
            tool_calls: 0,
            events_delivered: 0,
            events_dropped: 0,
        }
    }

    /// Update the last activity timestamp.
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Check if this session has been idle beyond the given timeout.
    pub fn is_idle(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    /// Client display name for logging.
    pub fn display_name(&self) -> String {
        match &self.client_info {
            Some(info) => {
                if let Some(ref v) = info.version {
                    format!("{}@{} (session {})", info.name, v, self.id)
                } else {
                    format!("{} (session {})", info.name, self.id)
                }
            }
            None => format!("session {}", self.id),
        }
    }
}

impl Default for ClientSession {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ClientSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSession")
            .field("id", &self.id)
            .field("client_info", &self.client_info)
            .field("initialized", &self.initialized)
            .field("subscriptions", &self.subscriptions)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_unique() {
        let s1 = ClientSession::new();
        let s2 = ClientSession::new();
        assert_ne!(s1.id, s2.id);
    }

    #[test]
    fn idle_detection() {
        let session = ClientSession::new();
        assert!(!session.is_idle(std::time::Duration::from_secs(30)));
    }

    #[test]
    fn display_name_without_client_info() {
        let session = ClientSession::new();
        assert!(session.display_name().contains("session"));
    }

    #[test]
    fn display_name_with_client_info() {
        let mut session = ClientSession::new();
        session.client_info = Some(ClientInfo {
            name: "claude-code".to_string(),
            version: Some("1.0".to_string()),
        });
        let name = session.display_name();
        assert!(name.contains("claude-code"));
        assert!(name.contains("1.0"));
    }
}
