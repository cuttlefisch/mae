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

use std::collections::HashMap;

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
    /// Principals on THIS daemon's LOCAL self-protection blocklist (ADR-039 A2, #162).
    /// Fetched from the daemon (`kb/blocklist`) — local-only, never propagated; distinct
    /// from a membership removal. A blocked principal is fenced at every membership check.
    pub blocked: Vec<BlockedView>,
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

/// A principal on the LOCAL self-protection blocklist (ADR-039 A2, #162). `label` is
/// best-effort from the member replica (a blocked principal need not be a member, so it
/// may be empty → the display falls back to the short fingerprint).
#[derive(Debug, Clone, Serialize)]
pub struct BlockedView {
    pub fingerprint: String,
    pub label: String,
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

/// Build the `BlockedView`s for a KB from the cached local blocklist (ADR-039 A2,
/// #162), resolving each blocked fingerprint's label from the member replica when it
/// happens to be a (still-listed) member — otherwise the label is empty and the display
/// falls back to the short fingerprint. Sorted for a stable view.
fn blocked_views(fps: Option<&Vec<String>>, members: &[MemberView]) -> Vec<BlockedView> {
    let mut out: Vec<BlockedView> = fps
        .into_iter()
        .flatten()
        .map(|fp| {
            let label = members
                .iter()
                .find(|m| &m.fingerprint == fp)
                .map(|m| m.label.clone())
                .unwrap_or_default();
            BlockedView {
                display: format_peer(&label, fp),
                fingerprint: fp.clone(),
                label,
            }
        })
        .collect();
    out.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    out
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

                let members: Vec<MemberView> = coll
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

                // Local blocklist (ADR-039 A2): label is best-effort from the member
                // replica (a blocked principal need not be a member → may be empty).
                let blocked = blocked_views(collab.kb_blocklists.get(&id), &members);

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
                    blocked,
                }
            }
            None => KbSharingEntry {
                blocked: blocked_views(collab.kb_blocklists.get(&id), &[]),
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

// --- `*KB Sharing*` buffer view model (P1) ---------------------------------
//
// A magit-style interactive buffer (mirrors `notifications_view` / `git_status`):
// a flat `Vec` of semantic lines + a fold map, built from a [`KbSharingSnapshot`].
// At-point dispatch maps the cursor row → (kb_id, optional fingerprint) → action.

/// Semantic line type for the `*KB Sharing*` buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KbSharingLineKind {
    /// Top "KB Sharing" header.
    Header,
    /// Connection status line.
    ConnectionLine,
    /// A foldable KB heading (folds its members + pending).
    KbHeader { kb_id: String },
    /// "Your role: …" line.
    RoleLine { kb_id: String },
    /// "Policy: …" line (owner action: set-policy).
    PolicyLine { kb_id: String },
    /// "Members (N):" subheading.
    MembersHeader { kb_id: String },
    /// A member row (owner actions: promote/demote/remove; anyone: copy-fp).
    Member { kb_id: String, fingerprint: String },
    /// "Pending requests (N):" subheading.
    PendingHeader { kb_id: String },
    /// A pending-request row (owner actions: approve/deny).
    Pending { kb_id: String, fingerprint: String },
    /// "Blocked (N):" subheading (local self-protection, ADR-039 A2).
    BlockedHeader { kb_id: String },
    /// A blocked-principal row (action: unblock; not owner-gated).
    Blocked { kb_id: String, fingerprint: String },
    /// Blank separator / non-actionable info.
    Blank,
}

/// A line in the `*KB Sharing*` buffer mapped to its KB / member / action.
#[derive(Debug, Clone)]
pub struct KbSharingLine {
    pub text: String,
    pub kind: KbSharingLineKind,
}

impl KbSharingLine {
    pub fn blank() -> Self {
        KbSharingLine {
            text: String::new(),
            kind: KbSharingLineKind::Blank,
        }
    }

    /// The KB id this line acts on, if any.
    pub fn kb_id(&self) -> Option<&str> {
        match &self.kind {
            KbSharingLineKind::KbHeader { kb_id }
            | KbSharingLineKind::RoleLine { kb_id }
            | KbSharingLineKind::PolicyLine { kb_id }
            | KbSharingLineKind::MembersHeader { kb_id }
            | KbSharingLineKind::Member { kb_id, .. }
            | KbSharingLineKind::PendingHeader { kb_id }
            | KbSharingLineKind::Pending { kb_id, .. }
            | KbSharingLineKind::BlockedHeader { kb_id }
            | KbSharingLineKind::Blocked { kb_id, .. } => Some(kb_id),
            _ => None,
        }
    }

    /// The member/pending/blocked fingerprint this line acts on, if any.
    pub fn fingerprint(&self) -> Option<&str> {
        match &self.kind {
            KbSharingLineKind::Member { fingerprint, .. }
            | KbSharingLineKind::Pending { fingerprint, .. }
            | KbSharingLineKind::Blocked { fingerprint, .. } => Some(fingerprint),
            _ => None,
        }
    }
}

/// Type-safe fold key — the `*KB Sharing*` buffer folds each KB, and within a KB
/// its members and pending sections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CollapseKey {
    Kb(String),
    Members(String),
    Pending(String),
    Blocked(String),
}

/// Structured state for the `*KB Sharing*` buffer. Carries the [`KbSharingSnapshot`]
/// so at-point dispatch can resolve action context (e.g. is this peer the owner,
/// what is a member's current role).
#[derive(Debug, Clone, Default)]
pub struct KbSharingView {
    pub lines: Vec<KbSharingLine>,
    pub collapsed: HashMap<CollapseKey, bool>,
    pub snapshot: Option<KbSharingSnapshot>,
}

impl KbSharingView {
    pub fn new() -> Self {
        KbSharingView::default()
    }

    pub fn line_at(&self, row: usize) -> Option<&KbSharingLine> {
        self.lines.get(row)
    }

    /// Toggle collapse state for a key (default expanded).
    pub fn toggle(&mut self, key: CollapseKey) {
        let collapsed = self.collapsed.entry(key).or_insert(false);
        *collapsed = !*collapsed;
    }

    pub fn is_collapsed(&self, key: &CollapseKey) -> bool {
        self.collapsed.get(key).copied().unwrap_or(false)
    }

    /// The fold key for a line, if it is a foldable header. A KB header folds the
    /// whole KB; the members/pending subheadings fold their own sections.
    pub fn collapse_key_for_line(line: &KbSharingLine) -> Option<CollapseKey> {
        match &line.kind {
            KbSharingLineKind::KbHeader { kb_id } => Some(CollapseKey::Kb(kb_id.clone())),
            KbSharingLineKind::MembersHeader { kb_id } => Some(CollapseKey::Members(kb_id.clone())),
            KbSharingLineKind::PendingHeader { kb_id } => Some(CollapseKey::Pending(kb_id.clone())),
            KbSharingLineKind::BlockedHeader { kb_id } => Some(CollapseKey::Blocked(kb_id.clone())),
            _ => None,
        }
    }

    /// Look up this peer's entry for `kb_id` in the captured snapshot (for action
    /// guards — e.g. only the owner may manage members).
    pub fn entry_for(&self, kb_id: &str) -> Option<&KbSharingEntry> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.kbs.iter().find(|k| k.id == kb_id))
    }
}

/// Build the `*KB Sharing*` view (lines + rope text) from a snapshot, preserving
/// the given fold state. Pure → unit-testable. Section layout per KB:
/// ```text
/// ▾ KB: Team Notes  [owner · invite · synced]
///     Your role: owner (epoch 0)
///     Policy: invite
///     Members (2):
///       alice (SHA256:ab…cd) — owner  (you)
///       bob   (SHA256:9x…h0) — editor
///     Pending (1):
///       carol (SHA256:c1…f2)  — requested 2026-06-23
/// ```
pub fn build_view(
    snapshot: &KbSharingSnapshot,
    collapsed: &HashMap<CollapseKey, bool>,
) -> (KbSharingView, String) {
    let mut view = KbSharingView::new();
    view.collapsed = collapsed.clone();
    let mut text = String::new();
    let mut push = |view: &mut KbSharingView, line: KbSharingLine| {
        text.push_str(&line.text);
        text.push('\n');
        view.lines.push(line);
    };

    push(
        &mut view,
        KbSharingLine {
            text: "KB Sharing".to_string(),
            kind: KbSharingLineKind::Header,
        },
    );
    let conn = &snapshot.connection;
    let conn_text = if conn.connected {
        format!(
            "  Connected to {} — {} peer(s)",
            conn.address, conn.peer_count
        )
    } else {
        format!("  {} ({})", conn.status, conn.address)
    };
    push(
        &mut view,
        KbSharingLine {
            text: conn_text,
            kind: KbSharingLineKind::ConnectionLine,
        },
    );
    push(&mut view, KbSharingLine::blank());

    if snapshot.kbs.is_empty() {
        push(
            &mut view,
            KbSharingLine {
                text: "  (no shared or joined KBs — :kb-share <name> to share one)".to_string(),
                kind: KbSharingLineKind::Blank,
            },
        );
        return (view, text);
    }

    for kb in &snapshot.kbs {
        let kb_collapsed = view.is_collapsed(&CollapseKey::Kb(kb.id.clone()));
        let marker = if kb_collapsed { '\u{25B8}' } else { '\u{25BE}' }; // ▸ / ▾
        let role = kb.role_of_me.as_deref().unwrap_or("not a member");
        let sync = if kb.sync_state.subscribed {
            "synced"
        } else {
            "offline"
        };
        push(
            &mut view,
            KbSharingLine {
                text: format!(
                    "{marker} KB: {}  [{} · {} · {}]",
                    kb.name, role, kb.policy, sync
                ),
                kind: KbSharingLineKind::KbHeader {
                    kb_id: kb.id.clone(),
                },
            },
        );
        if kb_collapsed {
            continue;
        }

        push(
            &mut view,
            KbSharingLine {
                text: format!("    Your role: {role} (epoch {})", kb.my_epoch),
                kind: KbSharingLineKind::RoleLine {
                    kb_id: kb.id.clone(),
                },
            },
        );
        push(
            &mut view,
            KbSharingLine {
                text: format!("    Policy: {}", kb.policy),
                kind: KbSharingLineKind::PolicyLine {
                    kb_id: kb.id.clone(),
                },
            },
        );

        // Members section.
        let members_collapsed = view.is_collapsed(&CollapseKey::Members(kb.id.clone()));
        let m_marker = if members_collapsed {
            '\u{25B8}'
        } else {
            '\u{25BE}'
        };
        push(
            &mut view,
            KbSharingLine {
                text: format!("  {m_marker} Members ({}):", kb.members.len()),
                kind: KbSharingLineKind::MembersHeader {
                    kb_id: kb.id.clone(),
                },
            },
        );
        if !members_collapsed {
            for m in &kb.members {
                let you = if m.is_me { "  (you)" } else { "" };
                push(
                    &mut view,
                    KbSharingLine {
                        text: format!("      {} — {}{you}", m.display, m.role),
                        kind: KbSharingLineKind::Member {
                            kb_id: kb.id.clone(),
                            fingerprint: m.fingerprint.clone(),
                        },
                    },
                );
            }
        }

        // Pending section (only when there are requests).
        if !kb.pending.is_empty() {
            let pending_collapsed = view.is_collapsed(&CollapseKey::Pending(kb.id.clone()));
            let p_marker = if pending_collapsed {
                '\u{25B8}'
            } else {
                '\u{25BE}'
            };
            push(
                &mut view,
                KbSharingLine {
                    text: format!("  {p_marker} Pending ({}):", kb.pending.len()),
                    kind: KbSharingLineKind::PendingHeader {
                        kb_id: kb.id.clone(),
                    },
                },
            );
            if !pending_collapsed {
                for p in &kb.pending {
                    push(
                        &mut view,
                        KbSharingLine {
                            text: format!("      {} — requested {}", p.display, p.requested_at),
                            kind: KbSharingLineKind::Pending {
                                kb_id: kb.id.clone(),
                                fingerprint: p.fingerprint.clone(),
                            },
                        },
                    );
                }
            }
        }

        // Blocked section (local self-protection, ADR-039 A2) — only when non-empty.
        if !kb.blocked.is_empty() {
            let blocked_collapsed = view.is_collapsed(&CollapseKey::Blocked(kb.id.clone()));
            let b_marker = if blocked_collapsed {
                '\u{25B8}'
            } else {
                '\u{25BE}'
            };
            push(
                &mut view,
                KbSharingLine {
                    text: format!("  {b_marker} Blocked ({}):", kb.blocked.len()),
                    kind: KbSharingLineKind::BlockedHeader {
                        kb_id: kb.id.clone(),
                    },
                },
            );
            if !blocked_collapsed {
                for b in &kb.blocked {
                    push(
                        &mut view,
                        KbSharingLine {
                            text: format!("      {} — blocked locally (B = unblock)", b.display),
                            kind: KbSharingLineKind::Blocked {
                                kb_id: kb.id.clone(),
                                fingerprint: b.fingerprint.clone(),
                            },
                        },
                    );
                }
            }
        }
        push(&mut view, KbSharingLine::blank());
    }

    view.snapshot = Some(snapshot.clone());
    (view, text)
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
    fn blocklist_renders_blocked_view_with_member_label() {
        // alice (owner) blocks bob (a member) and a non-member stranger fingerprint.
        let mut coll = KbCollectionDoc::new_owned("Team", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Editor);
        let mut state = state_with("alicefp", "team", &coll);
        state.kb_blocklists.insert(
            "team".to_string(),
            vec!["bobfp".to_string(), "SHA256:stranger".to_string()],
        );

        let snap = build_snapshot(&state);
        let kb = &snap.kbs[0];
        // Bob remains a MEMBER (the local block is not a removal) AND is listed Blocked.
        assert!(kb.members.iter().any(|m| m.fingerprint == "bobfp"));
        assert_eq!(kb.blocked.len(), 2);
        let bob = kb
            .blocked
            .iter()
            .find(|b| b.fingerprint == "bobfp")
            .expect("bob blocked");
        assert_eq!(bob.label, "bob", "label resolved from the member replica");
        let stranger = kb
            .blocked
            .iter()
            .find(|b| b.fingerprint == "SHA256:stranger")
            .expect("stranger blocked");
        assert_eq!(
            stranger.label, "",
            "a non-member block has no label → display falls back to the fingerprint"
        );

        // The buffer view renders a foldable Blocked section with a row per principal.
        let (view, _text) = build_view(&snap, &HashMap::new());
        assert!(view.lines.iter().any(
            |l| matches!(&l.kind, KbSharingLineKind::BlockedHeader { kb_id } if kb_id == "team")
        ));
        let blocked_rows = view
            .lines
            .iter()
            .filter(|l| matches!(&l.kind, KbSharingLineKind::Blocked { .. }))
            .count();
        assert_eq!(blocked_rows, 2);
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
        let _ = coll.add_pending("carolfp", "carol", "2026-06-23T10:00:00Z", None, None);

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

    // --- buffer view model ---

    fn owner_snapshot() -> KbSharingSnapshot {
        let mut coll = KbCollectionDoc::new_owned("Team Notes", "alicefp", "alice");
        let _ = coll.upsert_member("bobfp", "bob", Role::Editor);
        let _ = coll.add_pending("carolfp", "carol", "2026-06-23", None, None);
        let mut s = CollabState::new();
        s.local_fingerprint = "alicefp".to_string();
        s.kb_collection_state
            .insert("team".to_string(), coll.encode_state());
        s.shared_kbs.insert("team".to_string(), Default::default());
        build_snapshot(&s)
    }

    #[test]
    fn view_lays_out_kb_members_and_pending_with_action_targets() {
        let snap = owner_snapshot();
        let (view, text) = build_view(&snap, &HashMap::new());

        // The KB header, a member row for bob, and a pending row for carol exist.
        assert!(text.contains("KB: Team Notes"));
        assert!(text.contains("Members ("));
        assert!(text.contains("Pending ("));

        let member = view
            .lines
            .iter()
            .find(|l| matches!(&l.kind, KbSharingLineKind::Member { fingerprint, .. } if fingerprint == "bobfp"))
            .expect("bob member row");
        assert_eq!(member.kb_id(), Some("team"));
        assert_eq!(member.fingerprint(), Some("bobfp"));

        let pending = view
            .lines
            .iter()
            .find(|l| matches!(&l.kind, KbSharingLineKind::Pending { fingerprint, .. } if fingerprint == "carolfp"))
            .expect("carol pending row");
        assert_eq!(pending.fingerprint(), Some("carolfp"));

        // The captured snapshot resolves owner context for action guards.
        assert!(view.entry_for("team").unwrap().is_owner);
    }

    #[test]
    fn folding_a_kb_hides_its_member_rows() {
        let snap = owner_snapshot();
        let mut collapsed = HashMap::new();
        collapsed.insert(CollapseKey::Kb("team".to_string()), true);
        let (_view, text) = build_view(&snap, &collapsed);
        // KB header still present, but member rows hidden.
        assert!(text.contains("KB: Team Notes"));
        assert!(!text.contains("bob (SHA256"));
    }

    #[test]
    fn members_header_is_a_fold_key() {
        let line = KbSharingLine {
            text: "x".into(),
            kind: KbSharingLineKind::MembersHeader {
                kb_id: "team".into(),
            },
        };
        assert_eq!(
            KbSharingView::collapse_key_for_line(&line),
            Some(CollapseKey::Members("team".into()))
        );
    }
}
