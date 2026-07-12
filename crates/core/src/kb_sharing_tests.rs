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
    assert!(view
        .lines
        .iter()
        .any(|l| matches!(&l.kind, KbSharingLineKind::BlockedHeader { kb_id } if kb_id == "team")));
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
