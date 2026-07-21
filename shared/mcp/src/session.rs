//! MCP client session management.
//!
//! Each connected MCP client gets a `ClientSession` that tracks
//! its lifecycle, capabilities, and subscriptions.
//!
//! @ai-caution: Sync methods (`sync/state_vector`, `sync/update`,
//! `sync/full_state`, `sync/enable`) are implemented in `sync_exec.rs`.
//! Awareness/presence (`sync/awareness`) is implemented too
//! (`daemon/src/collab_handler/sync_methods.rs::handle_sync_awareness`,
//! `shared/sync/src/awareness.rs`).
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
    /// The authenticated peer identity (key/TLS modes). `None` for anonymous
    /// (no-auth) sessions. Authoritative for attribution + KB membership
    /// (ADR-017 strict binding).
    pub peer_identity: Option<crate::identity::PeerIdentity>,
    /// AI provider this client declared at `initialize` (ADR-048), e.g. `"ollama"`.
    /// Only ever set when [`Self::authenticated_principal`] is `Some` at the time of
    /// declaration (i.e. this session completed a PSK handshake) — an unauthenticated
    /// client's self-report is never stored here, since it would be a spoofable,
    /// unenforceable claim. Consulted by the AI-residency gate
    /// (`ai_event_handler::handle_mcp_request`) to decide whether this session may
    /// touch a `LocalModelsOnly`-flagged KB.
    pub declared_ai_provider: Option<String>,
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
            peer_identity: None,
            declared_ai_provider: None,
        }
    }

    /// Create a session bound to an authenticated peer identity.
    pub fn with_identity(identity: crate::identity::PeerIdentity) -> Self {
        Self {
            peer_identity: Some(identity),
            ..Self::new()
        }
    }

    /// The authenticated peer label, if this session is key/TLS-authenticated
    /// with a real (non-synthetic) identity. Display/logging only — never the
    /// subject for access control (use `authenticated_principal`).
    pub fn authenticated_label(&self) -> Option<&str> {
        self.peer_identity
            .as_ref()
            .filter(|p| p.is_authenticated())
            .map(|p| p.label.as_str())
    }

    /// The authoritative access-control **principal** (ADR-018): the key
    /// fingerprint for a key/TLS peer, `psk:<keyid>` for psk, or `None` for an
    /// unauthenticated (`none`/loopback) session. This is what KB ownership and
    /// membership key on — never the mutable, non-unique label.
    pub fn authenticated_principal(&self) -> Option<&str> {
        self.peer_identity.as_ref().and_then(|p| p.principal())
    }

    /// `(principal, label)` for combined logging/attribution, when a principal
    /// exists. The label is display-only.
    pub fn principal_and_label(&self) -> Option<(&str, &str)> {
        self.peer_identity
            .as_ref()
            .and_then(|p| p.principal().map(|pr| (pr, p.label.as_str())))
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
