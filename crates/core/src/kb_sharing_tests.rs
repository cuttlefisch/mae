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

/// Audit #589.1 — the attacker's test. A remote peer's join request carries a
/// self-declared `fingerprint` string that reaches `short_fingerprint` verbatim
/// via the owner's pending-request notification. The former byte slicing
/// (`&digest[..4]` / `&digest[len - 4..]`) panicked the *editor* on any
/// multi-byte codepoint at those offsets — a remote DoS triggered by merely
/// receiving a join request. Every case here must return, not panic.
#[test]
fn short_fingerprint_survives_hostile_non_ascii_input() {
    // Cut points chosen to land mid-codepoint at the head, at the tail, at both,
    // and at the length threshold — not one hand-picked value that dodges the edge.
    let hostile = [
        "SHA256:aécdefghij",         // 'é' straddles the head cut
        "SHA256:abcdefghié",         // 'é' straddles the tail cut
        "SHA256:aébcdefghié",        // both
        "SHA256:ééééééééé",          // every offset mid-codepoint
        "SHA256:\u{1F600}bcdefghij", // 4-byte emoji at the head
        "SHA256:abcdefghi\u{1F600}", // 4-byte emoji at the tail
        "SHA256:ééééé",              // 10 bytes but only 5 chars — under the threshold
        "SHA256:ééééé\u{1F600}",     // 14 bytes, 6 chars — still under
        "SHA256:",                   // empty digest
        "SHA256:é",                  // single multi-byte digest
        "\u{1F600}",                 // no prefix at all
    ];
    for fp in hostile {
        let out = short_fingerprint(fp);
        assert!(!out.is_empty() || fp.is_empty(), "{fp:?} produced nothing");
    }

    // Selective oracle: the truncation is in CHARACTERS, so a multi-byte digest
    // yields exactly 4 head + 4 tail characters — not a byte-count coincidence.
    assert_eq!(short_fingerprint("SHA256:ééééééééé"), "SHA256:éééé…éééé");
    // A 5-char digest is under the >8 threshold and must pass through intact
    // even though its BYTE length (10) is over it — the old code compared the
    // byte length against a character-count cut.
    assert_eq!(short_fingerprint("SHA256:ééééé"), "SHA256:ééééé");
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

/// Sign and append one membership op — the four-line ceremony every signed-op
/// fixture in this file repeats.
///
/// Extracted so the fixtures stay off the structural gate's per-function ceiling,
/// and so a change to the op-building signature edits one place rather than a
/// dozen (principle #8).
#[allow(clippy::too_many_arguments)]
fn append_op(
    coll: &mut KbCollectionDoc,
    kb_id: &str,
    action: mae_sync::membership::MembershipAction,
    subject: &str,
    role: Option<Role>,
    is_self: bool,
    owner_fp: &str,
    owner_secret: &[u8; 32],
    owner_pubkey: &[u8; 32],
    ts: u64,
    replication: Option<mae_sync::membership::ReplicationPolicy>,
    genesis: bool,
) {
    let mut op =
        coll.build_membership_op(kb_id, action, subject, role, is_self, owner_fp, ts, None, 0);
    if genesis {
        op.prev_hash = String::new();
    }
    if let Some(r) = replication {
        op.replication = r;
    }
    let sig = op.sign(owner_secret);
    coll.append_signed_op(&op, &sig, owner_pubkey);
}

/// ADR-067 Phase E: a real signed-op-log timeline distinguishing "joined then
/// later restricted" (residual replica risk) from "restricted before ever
/// joining" (no residual risk) — the exact fixture the ADR's own Verification
/// section requires, at the same `build_snapshot` level the buffer/MCP
/// tool/Scheme primitive all consume.
#[test]
fn owner_sees_residual_replica_risk_only_for_a_member_who_had_a_prior_full_window() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::{MembershipAction, ReplicationPolicy};

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_secret = owner.secret_bytes();
    let owner_pubkey = owner.public().to_bytes();

    let mut coll = KbCollectionDoc::new_owned("Team Notes", &owner_fp, "owner");

    // Genesis: owner's own self-admit anchors the signed op-log.
    let mut genesis = coll.build_membership_op(
        "team",
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        &owner_fp,
        1000,
        None,
        0,
    );
    genesis.prev_hash = String::new();
    let sig = genesis.sign(&owner_secret);
    coll.append_signed_op(&genesis, &sig, &owner_pubkey);

    // Alice: admitted Full, later restricted to QueryOnly -- a real window existed.
    let alice_fp = "SHA256:alice";
    coll.upsert_member(alice_fp, "alice", Role::Viewer);
    let alice_admit = coll.build_membership_op(
        "team",
        MembershipAction::Admit,
        alice_fp,
        Some(Role::Viewer),
        false,
        &owner_fp,
        1001,
        None,
        0,
    );
    let sig = alice_admit.sign(&owner_secret);
    coll.append_signed_op(&alice_admit, &sig, &owner_pubkey);

    let mut alice_restrict = coll.build_membership_op(
        "team",
        MembershipAction::SetRole,
        alice_fp,
        Some(Role::Viewer),
        false,
        &owner_fp,
        1002,
        None,
        0,
    );
    alice_restrict.replication = ReplicationPolicy::QueryOnly;
    let sig = alice_restrict.sign(&owner_secret);
    coll.append_signed_op(&alice_restrict, &sig, &owner_pubkey);

    // Bob: admitted directly at QueryOnly -- never had a Full window.
    let bob_fp = "SHA256:bob";
    coll.upsert_member(bob_fp, "bob", Role::Viewer);
    let mut bob_admit = coll.build_membership_op(
        "team",
        MembershipAction::Admit,
        bob_fp,
        Some(Role::Viewer),
        false,
        &owner_fp,
        1003,
        None,
        0,
    );
    bob_admit.replication = ReplicationPolicy::QueryOnly;
    let sig = bob_admit.sign(&owner_secret);
    coll.append_signed_op(&bob_admit, &sig, &owner_pubkey);

    // Carol: a plain Full editor, never restricted at all.
    coll.upsert_member("carolfp", "carol", Role::Editor);
    let carol_admit = coll.build_membership_op(
        "team",
        MembershipAction::Admit,
        "carolfp",
        Some(Role::Editor),
        false,
        &owner_fp,
        1004,
        None,
        0,
    );
    let sig = carol_admit.sign(&owner_secret);
    coll.append_signed_op(&carol_admit, &sig, &owner_pubkey);

    let state = state_with(&owner_fp, "team", &coll);
    let snap = build_snapshot(&state);
    let kb = &snap.kbs[0];

    let alice = kb
        .members
        .iter()
        .find(|m| m.fingerprint == alice_fp)
        .expect("alice present");
    assert_eq!(
        alice.residual_replica_risk,
        Some(true),
        "alice had a real Full-policy window before her later restriction"
    );

    let bob = kb
        .members
        .iter()
        .find(|m| m.fingerprint == bob_fp)
        .expect("bob present");
    assert_eq!(
        bob.residual_replica_risk,
        Some(false),
        "bob was restricted from his very first Admit -- no window ever existed"
    );

    let carol = kb
        .members
        .iter()
        .find(|m| m.fingerprint == "carolfp")
        .expect("carol present");
    assert_eq!(
        carol.residual_replica_risk, None,
        "carol is currently Full -- nothing restricted to report on"
    );

    // The buffer surfaces the annotation only for the real-risk case, never for
    // the no-risk or not-applicable ones.
    let (_view, text) = build_view(&snap, &HashMap::new());
    assert!(
        text.contains("alice") && text.contains("may hold a pre-restriction local copy"),
        "alice's row must carry the residual-risk annotation: {text}"
    );
    let bob_line = text
        .lines()
        .find(|l| l.contains("bob ("))
        .expect("bob's row");
    assert!(
        !bob_line.contains("may hold a pre-restriction local copy"),
        "bob's row must NOT carry the annotation -- he was never at risk: {bob_line}"
    );
    let carol_line = text
        .lines()
        .find(|l| l.contains("carol ("))
        .expect("carol's row");
    assert!(
        !carol_line.contains("may hold a pre-restriction local copy"),
        "carol's row must NOT carry the annotation -- not applicable to a Full member: {carol_line}"
    );
}

/// The three-member signed op-log both replication tests read.
///
/// Bob is restricted from his first Admit; Carol is plain Full; **Alice was
/// admitted Full and later restricted**, which is the only member for whom
/// "current" differs from "first" — without her, `.last()` and `.first()` are
/// indistinguishable and the currency assertion is vacuous.
fn replication_fixture() -> (String, KbCollectionDoc) {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::{MembershipAction, ReplicationPolicy};

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_secret = owner.secret_bytes();
    let owner_pubkey = owner.public().to_bytes();

    let mut coll = KbCollectionDoc::new_owned("Team Notes", &owner_fp, "owner");
    let mut op = |action, subject: &str, role, is_self, ts, repl, genesis| {
        append_op(
            &mut coll,
            "team",
            action,
            subject,
            role,
            is_self,
            &owner_fp,
            &owner_secret,
            &owner_pubkey,
            ts,
            repl,
            genesis,
        )
    };
    op(
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        1000,
        None,
        true,
    );
    // Bob: restricted from his first Admit — the case `residual_replica_risk`
    // reports as `Some(false)`, i.e. "no risk", which reads nothing like
    // "restricted".
    let bob_fp = "SHA256:bob";
    op(
        MembershipAction::Admit,
        bob_fp,
        Some(Role::Viewer),
        false,
        1001,
        Some(ReplicationPolicy::QueryOnly),
        false,
    );
    // Carol: a plain Full editor, never restricted.
    op(
        MembershipAction::Admit,
        "carolfp",
        Some(Role::Editor),
        false,
        1002,
        None,
        false,
    );
    // Alice: admitted **Full**, later restricted. Without a member whose policy
    // CHANGED, "current" is untestable — first and last coincide for everyone
    // else, and falsifying `.last()` to `.first()` passed on the first attempt
    // precisely because of that.
    let alice_fp = "SHA256:alice";
    op(
        MembershipAction::Admit,
        alice_fp,
        Some(Role::Viewer),
        false,
        1003,
        None,
        false,
    );
    op(
        MembershipAction::SetRole,
        alice_fp,
        Some(Role::Viewer),
        false,
        1004,
        Some(ReplicationPolicy::QueryOnly),
        false,
    );
    coll.upsert_member(bob_fp, "bob", Role::Viewer);
    coll.upsert_member("carolfp", "carol", Role::Editor);
    coll.upsert_member(alice_fp, "alice", Role::Viewer);

    (owner_fp, coll)
}

/// ADR-067: the **policy** axis, not the risk signal.
///
/// `residual_replica_risk` was the only trace of `ReplicationPolicy` in the
/// snapshot every surface reads — and it cannot answer "is this member
/// restricted?", because `None` means *both* `Full` and "legacy KB with no
/// op-log". Carol (Full) and a legacy KB both read `None` there, and they are not
/// the same thing. A restriction the owner cannot see is a control they cannot
/// audit.
#[test]
fn the_snapshot_reports_each_members_current_replication_policy() {
    let (owner_fp, coll) = replication_fixture();
    let bob_fp = "SHA256:bob";
    let alice_fp = "SHA256:alice";
    let state = state_with(&owner_fp, "team", &coll);
    let snap = build_snapshot(&state);
    let kb = &snap.kbs[0];
    let member = |fp: &str| {
        kb.members
            .iter()
            .find(|m| m.fingerprint == fp)
            .unwrap_or_else(|| panic!("{fp} present"))
            .clone()
    };

    assert_eq!(
        member(bob_fp).replication.as_deref(),
        Some("query_only"),
        "a restricted member's POLICY must be readable, not inferred from a risk signal"
    );
    assert_eq!(
        member("carolfp").replication.as_deref(),
        Some("full"),
        "and an unrestricted member's must say so — `Some(\"full\")` is a policy the \
         log states, deliberately distinct from the `None` a legacy KB gives"
    );
    assert_eq!(
        member(alice_fp).replication.as_deref(),
        Some("query_only"),
        "alice was admitted FULL and later restricted — the CURRENT policy is what \
         matters, not the one she started with"
    );
    assert_eq!(
        member(alice_fp).residual_replica_risk,
        Some(true),
        "sanity: alice is the member with a real prior-Full window"
    );
    assert_eq!(
        member(bob_fp).residual_replica_risk,
        Some(false),
        "sanity: the risk signal says 'no risk' for exactly this member, which is \
         why it cannot double as the policy"
    );
}

/// The buffer surfaces the restriction, and only the restriction.
#[test]
fn the_sharing_buffer_annotates_a_restricted_member_exactly_once() {
    let (owner_fp, coll) = replication_fixture();
    let snap = build_snapshot(&state_with(&owner_fp, "team", &coll));
    let (_view, text) = build_view(&snap, &HashMap::new());
    let bob_line = text
        .lines()
        .find(|l| l.contains("bob ("))
        .expect("bob's row");
    assert!(
        bob_line.contains("[query-only]"),
        "a restriction the owner cannot see is a control they cannot audit: {bob_line}"
    );
    let carol_line = text
        .lines()
        .find(|l| l.contains("carol ("))
        .expect("carol's row");
    assert!(
        !carol_line.contains("query-only"),
        "the default must stay quiet so the exception stands out: {carol_line}"
    );

    // Alice is BOTH restricted and at residual risk. Her row must carry the
    // richer annotation once, not two labels saying "query-only" back to back —
    // a redundant double label reads as a rendering bug and trains the owner to
    // skim past exactly the row that matters most.
    let alice_line = text
        .lines()
        .find(|l| l.contains("alice ("))
        .expect("alice's row");
    assert!(
        alice_line.contains("may hold a pre-restriction local copy"),
        "alice's row must carry the residual-risk annotation: {alice_line}"
    );
    assert_eq!(
        alice_line.matches("query-only").count(),
        1,
        "and must not stack a second query-only label: {alice_line}"
    );
}

/// A legacy/un-anchored KB has no signed op-log to derive from, and must report
/// **`None`** rather than guessing `full`. Guessing would tell an owner their
/// members are unrestricted when the truth is that nothing is known.
#[test]
fn a_kb_with_no_signed_oplog_reports_an_unknown_replication_policy() {
    let mut coll = KbCollectionDoc::new_owned("Legacy", "SHA256:owner", "owner");
    coll.upsert_member("SHA256:dave", "dave", Role::Viewer);

    let state = state_with("SHA256:owner", "legacy", &coll);
    let snap = build_snapshot(&state);
    let dave = snap.kbs[0]
        .members
        .iter()
        .find(|m| m.fingerprint == "SHA256:dave")
        .expect("dave present");

    assert_eq!(
        dave.replication, None,
        "no op-log means the policy is UNKNOWN, which is not the same as 'full'"
    );
}
