//! Awareness protocol — ephemeral cursor/selection/presence state.
//!
//! Awareness is transported as a lightweight JSON-RPC layer (`sync/awareness`)
//! on top of the existing collab transport. It is NOT persisted — no WAL, no
//! SQLite. The state server relays awareness updates between peers on the same
//! document, with echo filtering.
//!
//! Throttling: clients should send at most 20 Hz (50ms). Stale users are
//! cleaned up after 30s with no update.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

/// Ephemeral awareness state for a single user on a single document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AwarenessState {
    pub user_name: String,
    pub cursor_row: usize,
    pub cursor_col: usize,
    /// Selection range: (start_row, start_col, end_row, end_col).
    /// None when not in visual mode.
    pub selection: Option<(usize, usize, usize, usize)>,
    /// Current editor mode: "normal", "insert", "visual", etc.
    pub mode: String,
    /// Currently viewed KB node ID (when browsing a shared KB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb_node_id: Option<String>,
    /// Which shared KB the user is connected to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kb_id: Option<String>,
}

/// A tracked remote user with awareness state and timing.
#[derive(Debug, Clone)]
pub struct RemoteUser {
    pub client_id: u64,
    pub user_name: String,
    pub color_index: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub selection: Option<(usize, usize, usize, usize)>,
    pub mode: String,
    pub last_seen: Instant,
    pub doc_id: String,
}

/// Manages remote user awareness state for a collaborative session.
///
/// Stores per-client awareness, handles updates, and provides timeout cleanup.
#[derive(Debug, Default)]
pub struct AwarenessMap {
    users: HashMap<u64, RemoteUser>,
}

/// Timeout for stale user cleanup (30 seconds).
const STALE_TIMEOUT_SECS: u64 = 30;

impl AwarenessMap {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
        }
    }

    /// Update or insert a remote user's awareness state.
    pub fn update(
        &mut self,
        client_id: u64,
        doc_id: String,
        state: AwarenessState,
        color_index: usize,
    ) {
        let user = self.users.entry(client_id).or_insert_with(|| RemoteUser {
            client_id,
            user_name: state.user_name.clone(),
            color_index,
            cursor_row: 0,
            cursor_col: 0,
            selection: None,
            mode: String::new(),
            last_seen: Instant::now(),
            doc_id: doc_id.clone(),
        });
        user.user_name = state.user_name;
        user.cursor_row = state.cursor_row;
        user.cursor_col = state.cursor_col;
        user.selection = state.selection;
        user.mode = state.mode;
        user.last_seen = Instant::now();
        user.doc_id = doc_id;
    }

    /// Remove a specific user (e.g. on disconnect notification).
    pub fn remove(&mut self, client_id: u64) -> Option<RemoteUser> {
        self.users.remove(&client_id)
    }

    /// Remove users that haven't sent an update within the timeout.
    /// Returns the number of users removed.
    pub fn cleanup_stale(&mut self) -> usize {
        let now = Instant::now();
        let before = self.users.len();
        self.users
            .retain(|_, u| now.duration_since(u.last_seen).as_secs() < STALE_TIMEOUT_SECS);
        before - self.users.len()
    }

    /// Get all remote users for a specific document.
    pub fn users_for_doc(&self, doc_id: &str) -> Vec<&RemoteUser> {
        self.users.values().filter(|u| u.doc_id == doc_id).collect()
    }

    /// Get all remote users.
    pub fn all_users(&self) -> impl Iterator<Item = &RemoteUser> {
        self.users.values()
    }

    /// Number of tracked remote users.
    pub fn len(&self) -> usize {
        self.users.len()
    }

    /// Whether there are no tracked remote users.
    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state(name: &str) -> AwarenessState {
        AwarenessState {
            user_name: name.to_string(),
            cursor_row: 10,
            cursor_col: 5,
            selection: None,
            mode: "normal".to_string(),
            kb_node_id: None,
            kb_id: None,
        }
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let state = AwarenessState {
            user_name: "Alice".to_string(),
            cursor_row: 42,
            cursor_col: 10,
            selection: Some((1, 0, 3, 15)),
            mode: "visual".to_string(),
            kb_node_id: None,
            kb_id: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: AwarenessState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, parsed);
    }

    #[test]
    fn awareness_map_update_and_lookup() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        map.update(2, "doc1".into(), sample_state("Bob"), 1);
        map.update(3, "doc2".into(), sample_state("Carol"), 2);

        assert_eq!(map.len(), 3);
        assert_eq!(map.users_for_doc("doc1").len(), 2);
        assert_eq!(map.users_for_doc("doc2").len(), 1);
    }

    #[test]
    fn awareness_map_remove() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        assert_eq!(map.len(), 1);
        let removed = map.remove(1);
        assert!(removed.is_some());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn awareness_map_stale_cleanup() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        // Manually set last_seen to be stale
        if let Some(user) = map.users.get_mut(&1) {
            user.last_seen = Instant::now() - std::time::Duration::from_secs(60);
        }
        let removed = map.cleanup_stale();
        assert_eq!(removed, 1);
        assert!(map.is_empty());
    }

    #[test]
    fn awareness_map_fresh_not_cleaned() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        let removed = map.cleanup_stale();
        assert_eq!(removed, 0);
        assert_eq!(map.len(), 1);
    }

    // --- Branch coverage ---

    #[test]
    fn update_overwrites_same_client_id() {
        let mut map = AwarenessMap::new();
        let state1 = AwarenessState {
            user_name: "Alice".to_string(),
            cursor_row: 1,
            cursor_col: 1,
            selection: None,
            mode: "normal".to_string(),
            kb_node_id: None,
            kb_id: None,
        };
        map.update(1, "doc1".into(), state1, 0);
        assert_eq!(map.users_for_doc("doc1")[0].cursor_row, 1);

        let state2 = AwarenessState {
            user_name: "Alice (renamed)".to_string(),
            cursor_row: 42,
            cursor_col: 10,
            selection: Some((1, 0, 3, 15)),
            mode: "visual".to_string(),
            kb_node_id: None,
            kb_id: None,
        };
        map.update(1, "doc1".into(), state2, 0);
        assert_eq!(map.len(), 1, "should overwrite, not add");
        let user = &map.users_for_doc("doc1")[0];
        assert_eq!(user.cursor_row, 42);
        assert_eq!(user.user_name, "Alice (renamed)");
        assert_eq!(user.selection, Some((1, 0, 3, 15)));
        assert_eq!(user.mode, "visual");
    }

    #[test]
    fn cleanup_stale_mixed_fresh_and_stale() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        map.update(2, "doc1".into(), sample_state("Bob"), 1);
        map.update(3, "doc1".into(), sample_state("Carol"), 2);

        // Make Alice and Carol stale.
        if let Some(user) = map.users.get_mut(&1) {
            user.last_seen = Instant::now() - std::time::Duration::from_secs(60);
        }
        if let Some(user) = map.users.get_mut(&3) {
            user.last_seen = Instant::now() - std::time::Duration::from_secs(60);
        }

        let removed = map.cleanup_stale();
        assert_eq!(removed, 2);
        assert_eq!(map.len(), 1);
        assert_eq!(map.users_for_doc("doc1")[0].user_name, "Bob");
    }

    #[test]
    fn users_for_doc_empty_map() {
        let map = AwarenessMap::new();
        assert!(map.users_for_doc("any-doc").is_empty());
        assert!(map.is_empty());
    }

    #[test]
    fn remove_nonexistent_client() {
        let mut map = AwarenessMap::new();
        assert!(map.remove(999).is_none());
    }

    #[test]
    fn update_changes_doc_id() {
        let mut map = AwarenessMap::new();
        map.update(1, "doc1".into(), sample_state("Alice"), 0);
        assert_eq!(map.users_for_doc("doc1").len(), 1);
        assert_eq!(map.users_for_doc("doc2").len(), 0);

        // Same client moves to different doc.
        map.update(1, "doc2".into(), sample_state("Alice"), 0);
        assert_eq!(map.users_for_doc("doc1").len(), 0);
        assert_eq!(map.users_for_doc("doc2").len(), 1);
    }
}
