//! KB-sharing introspection snapshot — the **single source of truth** for the
//! `*KB Sharing*` management buffer, the `kb_sharing_status` MCP tool, and the
//! `(kb-sharing-status)` Scheme primitive.
//!
//! One pure builder ([`build_snapshot`]) reads this peer's local collaborative
//! state (the C1 `kb_collection_state` replicas + `kb_epochs` + the connection
//! status) and produces a serializable [`KbSharingSnapshot`]. The buffer (human),
//! the Scheme primitive (user scripts), and the MCP tool (AI peer) all consume
//! the SAME snapshot — so introspection is at parity across all three actors
//! (CLAUDE.md #3 the AI is a peer, #8 shared computation).
//!
//! The snapshot is built entirely from LOCAL replicas (no daemon round-trip): the
//! daemon remains the sole authority and broadcasts every membership change as a
//! `kbc:` delta that C1 applies to the replica, so the local view tracks the
//! authoritative one without polling.

use serde::Serialize;

use crate::editor::CollabState;

/// A complete picture of this peer's KB-sharing state.
#[derive(Debug, Clone, Serialize)]
pub struct KbSharingSnapshot {
    pub connection: ConnectionInfo,
    /// One entry per KB this peer owns/shares or has joined (and holds a local
    /// collection replica for). Sorted by `name` for stable display.
    pub kbs: Vec<KbSharingEntry>,
}

/// Daemon connection state.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub connected: bool,
    pub address: String,
    pub peer_count: usize,
    /// `off | connecting | connected | reconnecting | disconnected`.
    pub status: String,
}

/// One shared/joined KB's membership, policy, and sync state.
#[derive(Debug, Clone, Serialize)]
pub struct KbSharingEntry {
    /// The collab id / share name (the `kbc:<id>` key).
    pub id: String,
    /// Display name from the collection doc (falls back to `id`).
    pub name: String,
    /// This peer's role in the KB, if it is a member.
    pub role_of_me: Option<String>,
    /// True iff `role_of_me == "owner"` (drives owner-only actions in the UI).
    pub is_owner: bool,
    /// This peer's current authorization epoch (ADR-023) for the KB.
    pub my_epoch: u64,
    /// `restrictive | invite | permissive`.
    pub policy: String,
    /// `replicated` (hosted live-edit is deferred — see ADR-020 D1).
    pub mode: String,
    pub sync_state: SyncState,
    pub members: Vec<MemberView>,
    pub pending: Vec<PendingView>,
}

/// A member of a shared KB.
#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    pub fingerprint: String,
    pub label: String,
    pub role: String,
    pub epoch: u64,
    pub is_me: bool,
    /// `label (SHA256:ab…3f)` — the shared display form (locked identity decision).
    pub display: String,
}

/// A pending join request (invite policy) awaiting owner approval.
#[derive(Debug, Clone, Serialize)]
pub struct PendingView {
    pub fingerprint: String,
    pub label: String,
    pub requested_at: String,
    pub display: String,
}

/// Live sync status for a KB.
#[derive(Debug, Clone, Serialize)]
pub struct SyncState {
    /// Subscribed to live node updates (in `shared_kbs`).
    pub subscribed: bool,
    /// Number of nodes being synced.
    pub node_count: usize,
    /// Local node updates queued/in-flight to the daemon for this KB.
    pub pending_updates: usize,
    pub inflight_updates: usize,
}

/// Truncate an Ed25519 key fingerprint for display: `SHA256:ab12…cd`
/// (head + tail of the base64 digest). The full fingerprint stays available in
/// the structured `fingerprint` field. Non-`SHA256:` inputs pass through.
pub fn short_fingerprint(fp: &str) -> String {
    if let Some(digest) = fp.strip_prefix("SHA256:") {
        if digest.len() > 8 {
            return format!("SHA256:{}…{}", &digest[..4], &digest[digest.len() - 4..]);
        }
    }
    fp.to_string()
}

/// Format a peer as `label (SHA256:ab12…cd)` — the single display form used by
/// the buffer, pick-lists, and notifications (locked identity decision, #8).
/// Falls back to the short fingerprint alone when the label is empty.
pub fn format_peer(label: &str, fingerprint: &str) -> String {
    let short = short_fingerprint(fingerprint);
    if label.is_empty() {
        short
    } else {
        format!("{label} ({short})")
    }
}

/// Build the KB-sharing snapshot from this peer's local collaborative state.
///
/// Iterates the local collection replicas (`kb_collection_state`) — the union of
/// owner-shared (seeded on `KbShared`) and member-joined (seeded on `KbJoined`)
/// KBs — plus any subscribed KB whose replica has not yet arrived (a degraded
/// entry, never a panic). Pure + read-only; trivially unit-testable.
pub fn build_snapshot(collab: &CollabState) -> KbSharingSnapshot {
    use mae_sync::kb::KbCollectionDoc;

    let me = collab.local_fingerprint.as_str();

    // KB ids we know about: every replica, plus any subscribed KB lacking one.
    let mut ids: Vec<String> = collab.kb_collection_state.keys().cloned().collect();
    for kb_id in collab.shared_kbs.keys() {
        if !collab.kb_collection_state.contains_key(kb_id) {
            ids.push(kb_id.clone());
        }
    }
    ids.sort();
    ids.dedup();

    let mut kbs = Vec::with_capacity(ids.len());
    for id in ids {
        let sync_state = SyncState {
            subscribed: collab.shared_kbs.contains_key(&id),
            node_count: collab.shared_kbs.get(&id).map(|n| n.len()).unwrap_or(0),
            pending_updates: collab
                .pending_kb_updates
                .iter()
                .filter(|(kb, _, _)| kb == &id)
                .count(),
            inflight_updates: collab.inflight_kb_updates.len(),
        };

        // Decode the local collection replica (tolerant: a missing/undecodable
        // replica yields a degraded entry, never a panic).
        let coll = collab
            .kb_collection_state
            .get(&id)
            .and_then(|bytes| KbCollectionDoc::from_bytes(bytes).ok());

        let entry = match coll {
            Some(coll) => {
                let name = {
                    let n = coll.name();
                    if n.is_empty() {
                        id.clone()
                    } else {
                        n
                    }
                };
                let role_of_me = coll.role_of(me).map(|r| r.as_str().to_string());
                let is_owner = role_of_me.as_deref() == Some("owner");
                let my_epoch = collab
                    .kb_epochs
                    .get(&id)
                    .copied()
                    .unwrap_or_else(|| coll.epoch_of(me));

                let members = coll
                    .member_roles()
                    .into_iter()
                    .map(|m| MemberView {
                        is_me: m.fingerprint == me && !me.is_empty(),
                        epoch: coll.epoch_of(&m.fingerprint),
                        display: format_peer(&m.label, &m.fingerprint),
                        fingerprint: m.fingerprint,
                        label: m.label,
                        role: m.role.as_str().to_string(),
                    })
                    .collect();

                let pending = coll
                    .pending()
                    .into_iter()
                    .map(|p| PendingView {
                        display: format_peer(&p.label, &p.fingerprint),
                        fingerprint: p.fingerprint,
                        label: p.label,
                        requested_at: p.requested_at,
                    })
                    .collect();

                KbSharingEntry {
                    id: id.clone(),
                    name,
                    role_of_me,
                    is_owner,
                    my_epoch,
                    policy: coll.join_policy().as_str().to_string(),
                    mode: "replicated".to_string(),
                    sync_state,
                    members,
                    pending,
                }
            }
            None => KbSharingEntry {
                name: id.clone(),
                id,
                role_of_me: None,
                is_owner: false,
                my_epoch: 0,
                policy: "invite".to_string(),
                mode: "replicated".to_string(),
                sync_state,
                members: Vec::new(),
                pending: Vec::new(),
            },
        };
        kbs.push(entry);
    }

    KbSharingSnapshot {
        connection: connection_info(collab),
        kbs,
    }
}

fn connection_info(collab: &CollabState) -> ConnectionInfo {
    use crate::editor::CollabStatus;
    let (connected, peer_count, status) = match collab.status {
        CollabStatus::Off => (false, 0, "off"),
        CollabStatus::Connecting => (false, 0, "connecting"),
        CollabStatus::Connected { peer_count } => (true, peer_count, "connected"),
        CollabStatus::Reconnecting => (false, 0, "reconnecting"),
        CollabStatus::Disconnected => (false, 0, "disconnected"),
    };
    ConnectionInfo {
        connected,
        address: collab.server_address.clone(),
        peer_count,
        status: status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::CollabState;
    use mae_sync::kb::{KbCollectionDoc, Role};

    /// Seed a CollabState as if this peer were `me_fp`, holding a replica of a KB.
    fn state_with(me_fp: &str, kb_id: &str, coll: &KbCollectionDoc) -> CollabState {
        let mut s = CollabState::new();
        s.local_fingerprint = me_fp.to_string();
        s.kb_collection_state
            .insert(kb_id.to_string(), coll.encode_state());
        s
    }

    #[test]
    fn short_fingerprint_truncates_head_and_tail() {
        assert_eq!(short_fingerprint("SHA256:abcdefghij"), "SHA256:abcd…ghij");
        // Short / non-SHA256 inputs pass through.
        assert_eq!(short_fingerprint("SHA256:abc"), "SHA256:abc");
        assert_eq!(short_fingerprint("psk:x"), "psk:x");
    }

    #[test]
    fn format_peer_label_plus_short_fp() {
        assert_eq!(
            format_peer("alice", "SHA256:abcdefghij"),
            "alice (SHA256:abcd…ghij)"
        );
        // Empty label → short fingerprint alone.
        assert_eq!(format_peer("", "SHA256:abcdefghij"), "SHA256:abcd…ghij");
    }

    #[test]
    fn owner_sees_its_own_kb_with_members_and_role() {
        // Owner alice shares a KB and adds bob as editor.
        let mut coll = KbCollectionDoc::new_owned("Team Notes", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Editor);

        let state = state_with("alicefp", "team", &coll);
        let snap = build_snapshot(&state);

        assert_eq!(snap.kbs.len(), 1);
        let kb = &snap.kbs[0];
        assert_eq!(kb.id, "team");
        assert_eq!(kb.name, "Team Notes");
        assert_eq!(kb.role_of_me.as_deref(), Some("owner"));
        assert!(kb.is_owner);
        assert_eq!(kb.policy, "invite");

        // Members include alice (me, owner) and bob (editor).
        let me = kb
            .members
            .iter()
            .find(|m| m.is_me)
            .expect("self is a member");
        assert_eq!(me.role, "owner");
        assert_eq!(me.fingerprint, "alicefp");
        let bob = kb
            .members
            .iter()
            .find(|m| m.fingerprint == "bobfp")
            .expect("bob present");
        assert_eq!(bob.role, "editor");
        assert!(!bob.is_me);
        assert!(bob.display.starts_with("bob ("));
    }

    #[test]
    fn joined_member_sees_roster_and_own_role() {
        // Bob joined a KB owned by alice; bob is a viewer.
        let mut coll = KbCollectionDoc::new_owned("Team Notes", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Viewer);

        let mut state = state_with("bobfp", "team", &coll);
        state.kb_epochs.insert("team".to_string(), 0);

        let snap = build_snapshot(&state);
        let kb = &snap.kbs[0];
        assert_eq!(kb.role_of_me.as_deref(), Some("viewer"));
        assert!(!kb.is_owner);
        // Bob sees alice in the roster.
        assert!(kb
            .members
            .iter()
            .any(|m| m.fingerprint == "alicefp" && m.role == "owner"));
    }

    #[test]
    fn pending_requests_surface() {
        let mut coll = KbCollectionDoc::new_owned("Team Notes", "alicefp", "alice");
        let _ = coll.add_pending("carolfp", "carol", "2026-06-23T10:00:00Z");

        let state = state_with("alicefp", "team", &coll);
        let snap = build_snapshot(&state);
        let kb = &snap.kbs[0];
        assert_eq!(kb.pending.len(), 1);
        assert_eq!(kb.pending[0].fingerprint, "carolfp");
        assert_eq!(kb.pending[0].label, "carol");
        assert!(kb.pending[0].display.starts_with("carol ("));
    }

    #[test]
    fn subscribed_kb_without_replica_is_degraded_not_dropped() {
        let mut s = CollabState::new();
        s.local_fingerprint = "mefp".to_string();
        s.shared_kbs.insert("ghost".to_string(), Default::default());
        let snap = build_snapshot(&s);
        assert_eq!(snap.kbs.len(), 1);
        assert_eq!(snap.kbs[0].id, "ghost");
        assert_eq!(snap.kbs[0].role_of_me, None);
        assert!(snap.kbs[0].members.is_empty());
    }
}
