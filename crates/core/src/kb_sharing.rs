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
    /// ADR-067 Phase E: for a currently `QueryOnly`-restricted member, whether they
    /// were ever granted `Full` replication before being restricted — a real
    /// residual-replica-risk signal (an already-replicated local copy may exist,
    /// which this ADR cannot delete or confirm). `None` when not applicable: the
    /// member is currently `Full` (nothing restricted to report on), or this KB has
    /// no signed op-log to derive history from at all (a legacy/un-anchored KB —
    /// the same named scope boundary as Phase B's own `kb_access` gate).
    pub residual_replica_risk: Option<bool>,
    /// ADR-067: this member's **current replication policy** — `"full"` or
    /// `"query_only"` — derived from the signed op-log.
    ///
    /// The policy axis was previously invisible in this snapshot, which is the
    /// one thing `kb_sharing_status` (MCP), `(kb-sharing-status)` (Scheme) and the
    /// `*KB Sharing*` buffer all read. `residual_replica_risk` was its only trace,
    /// and it is a *risk* signal rather than a policy one: `Some(_)` implies
    /// `QueryOnly`, but `None` means `Full` **or** a legacy KB with no op-log —
    /// so "is this member restricted?" could not be answered from the snapshot at
    /// all without an inference that is wrong for legacy KBs.
    ///
    /// `None` here means the same thing it means there: no anchored history to
    /// derive from. Deliberately distinct from `Some("full")`, which is a policy
    /// the log actually states.
    pub replication: Option<String>,
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
///
/// @ai-caution: [text-index] `fp` is NOT trusted to be ASCII. It reaches this
/// function straight off the collection CRDT — a hostile peer's join request
/// carries a self-declared `fingerprint` string (`collab_bridge::mod.rs`'s
/// pending-request notification path), so a remote peer chooses these bytes.
/// The head/tail cut is therefore taken in CHARACTERS, never bytes: the former
/// `&digest[..4]` / `&digest[digest.len() - 4..]` byte slicing panicked the
/// editor on any multi-byte codepoint straddling those offsets (audit #589.1).
/// This is a display truncation of untrusted input, not an index-domain bug, so
/// `grapheme::checked_byte_boundary` (ADR-087's *assert-and-clamp* chokepoint
/// for offsets that should already be valid) is deliberately not used here.
pub fn short_fingerprint(fp: &str) -> String {
    if let Some(digest) = fp.strip_prefix("SHA256:") {
        // `.count()` rather than `.len()`: the threshold must be in the same
        // unit as the cut, or an 8-byte/4-char digest slices past its own end.
        if digest.chars().count() > 8 {
            let head: String = digest.chars().take(4).collect();
            let tail: String = {
                let n = digest.chars().count();
                digest.chars().skip(n - 4).collect()
            };
            return format!("SHA256:{head}…{tail}");
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

                // ADR-067 Phase E: computed once per KB (not per member) since it
                // only depends on the collection's own op-log, not on which member
                // is being viewed.
                let oplog_ops = coll.oplog_ops();
                let members: Vec<MemberView> = coll
                    .member_roles()
                    .into_iter()
                    .map(|m| MemberView {
                        is_me: m.fingerprint == me && !me.is_empty(),
                        epoch: coll.epoch_of(&m.fingerprint),
                        display: format_peer(&m.label, &m.fingerprint),
                        residual_replica_risk:
                            mae_sync::membership::had_full_replication_window_self_anchored(
                                &oplog_ops,
                                &m.fingerprint,
                            ),
                        replication: mae_sync::membership::current_replication_self_anchored(
                            &oplog_ops,
                            &m.fingerprint,
                        )
                        .map(|r| r.as_str().to_string()),
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
        crate::foldable_view::line_at(&self.lines, row)
    }

    /// Toggle collapse state for a key (default expanded).
    pub fn toggle(&mut self, key: CollapseKey) {
        crate::foldable_view::toggle(&mut self.collapsed, key);
    }

    pub fn is_collapsed(&self, key: &CollapseKey) -> bool {
        crate::foldable_view::is_collapsed(&self.collapsed, key)
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
/// One member's row in the `*KB Sharing*` buffer.
///
/// Extracted from `build_view` to keep it off the structural gate's per-function
/// ceiling — the gate is per-item so the remedy is local.
///
/// **Both ADR-067 annotations are decided here, together, because they interact.**
/// `residual_replica_risk` only ever adds text for the real-risk case, so a
/// restricted-but-never-replicated member reads as no false alarm; and the plain
/// `[query-only]` label is suppressed when that richer one already says it, so the
/// two never stack into a redundant double label — which reads as a rendering bug
/// and trains the owner to skim past exactly the row that matters most.
fn member_row(m: &MemberView) -> String {
    let you = if m.is_me { "  (you)" } else { "" };
    let residual_risk = match m.residual_replica_risk {
        Some(true) => "  [query-only; may hold a pre-restriction local copy]",
        _ => "",
    };
    // A restriction the owner cannot SEE is a control they cannot audit. Shown
    // only when it IS a restriction: `full` is the default and stays quiet, so
    // the exception is what stands out.
    let replication = match m.replication.as_deref() {
        Some("query_only") if residual_risk.is_empty() => "  [query-only]",
        _ => "",
    };
    format!(
        "      {} — {}{you}{replication}{residual_risk}",
        m.display, m.role
    )
}

pub fn build_view(
    snapshot: &KbSharingSnapshot,
    collapsed: &HashMap<CollapseKey, bool>,
) -> (KbSharingView, String) {
    let mut view = KbSharingView::new();
    view.collapsed = collapsed.clone();
    let mut text = String::new();
    let mut push = |view: &mut KbSharingView, line: KbSharingLine| {
        let line_text = line.text.clone();
        crate::foldable_view::push_line(&mut text, &line_text, &mut view.lines, line);
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
                push(
                    &mut view,
                    KbSharingLine {
                        text: member_row(m),
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
#[path = "kb_sharing_tests.rs"]
mod tests;
