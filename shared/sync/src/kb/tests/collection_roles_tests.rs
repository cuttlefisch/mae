//! `KbCollectionDoc` ownership & roles tests (mirrors `collection_roles.rs`):
//! epoch-fenced rebase, v2 schema (owner/roles/policy/pending), transport
//! policy, and the `update_new_op_authors` epoch-fencing free function.

use super::*;

// ---- ADR-023 (B-19) epoch-fenced-rebase primitives ----

#[test]
fn member_epoch_advances_only_on_role_change() {
    // The daemon authors these ops, so the epoch is unforgeable by the client.
    let mut coll = KbCollectionDoc::new("Test", "alice");
    assert_eq!(coll.epoch_of("bob"), 0, "non-member starts at epoch 0");

    // Fresh grants stay at epoch 0 — no prior write-capable lineage to fence,
    // so owners + directly-added editors need no editor-side epoch sync.
    coll.set_owner("alice", "alice");
    assert_eq!(coll.epoch_of("alice"), 0, "fresh owner seeds at epoch 0");
    coll.upsert_member("bob", "bob", Role::Viewer);
    assert_eq!(coll.epoch_of("bob"), 0, "first grant ⇒ epoch 0");

    // Re-stamping the SAME role must not advance (B-12 owner re-share idempotency).
    coll.set_owner("alice", "alice");
    assert_eq!(coll.epoch_of("alice"), 0, "owner re-stamp preserves epoch");
    coll.upsert_member("bob", "bob", Role::Viewer);
    assert_eq!(
        coll.epoch_of("bob"),
        0,
        "same-role re-assignment is a no-op"
    );

    // The B-19 cascade vector: a role CHANGE. The epoch MUST advance so bob's
    // post-grant client_id differs from his viewer-era one — to an UNPREDICTABLE
    // token (#72), never the guessable prev+1.
    let viewer_epoch = coll.epoch_of("bob"); // 0
    coll.set_role("bob", Role::Editor);
    let editor_epoch = coll.epoch_of("bob");
    assert_ne!(
        editor_epoch, viewer_epoch,
        "viewer→editor advances the epoch"
    );
    assert_ne!(
        editor_epoch,
        viewer_epoch + 1,
        "advance is an unpredictable token, not prev+1 (#72)"
    );
    coll.upsert_member("bob", "bob", Role::Viewer);
    let reviewer_epoch = coll.epoch_of("bob");
    assert_ne!(reviewer_epoch, editor_epoch, "editor→viewer advances again");
    assert_ne!(
        reviewer_epoch, 0,
        "an advance never returns to the sentinel"
    );
}

#[test]
fn derive_kb_client_id_rotates_with_epoch_and_stays_53bit() {
    let fp = "ed25519:AAAA";
    let ids: Vec<u64> = (0..4).map(|e| derive_kb_client_id(fp, e)).collect();
    // Distinct per epoch — a viewer-era op can never masquerade as current-epoch.
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b, "epochs must yield distinct client_ids");
        }
    }
    // B-17: yrs ClientID is 53-bit; never 0/1.
    for id in &ids {
        assert!(*id < (1u64 << 53), "client_id must fit yrs' 53 bits");
        assert!(*id > 1, "client_id must avoid the reserved 0/1");
    }
    // Deterministic across the editor/daemon boundary.
    assert_eq!(derive_kb_client_id(fp, 2), derive_kb_client_id(fp, 2));
}

#[test]
fn update_new_op_authors_flags_stale_epoch_lineage() {
    // The daemon's fence in miniature: an owner-authored node is the canonical
    // base; a viewer (old epoch) and a granted editor (new epoch) each author an
    // edit. update_new_op_authors must attribute each update to its real author,
    // so the daemon can reject the viewer-era lineage and accept only C_now.
    let fp = "ed25519:bob";
    let c_viewer = derive_kb_client_id(fp, 0); // pre-grant (added at epoch 0)
    let c_editor = derive_kb_client_id(fp, 1); // post-grant, viewer→editor (C_now)

    let base = KbNodeDoc::new_with_client_id("n1", "Original", "body", &[], 99);
    let base_state = base.encode_state();

    // Viewer-era edit (would be denied live, but lands in the local lineage).
    let mut viewer = KbNodeDoc::from_bytes_with_client_id(&base_state, c_viewer).unwrap();
    let viewer_update = viewer.set_title("hijacked");
    let viewer_authors = update_new_op_authors(&viewer_update, &base_state).unwrap();
    assert_eq!(
        viewer_authors,
        vec![c_viewer],
        "stale lineage is attributable"
    );
    assert!(
        !viewer_authors.iter().all(|a| *a == c_editor),
        "fence rejects: not every new op is from C_now"
    );

    // A fresh, current-epoch edit is accepted (every new op is C_now).
    let mut editor = KbNodeDoc::from_bytes_with_client_id(&base_state, c_editor).unwrap();
    let editor_update = editor.set_title("legit edit");
    let editor_authors = update_new_op_authors(&editor_update, &base_state).unwrap();
    assert_eq!(editor_authors, vec![c_editor]);
    assert!(
        editor_authors.iter().all(|a| *a == c_editor),
        "fence accepts: all new ops authored under C_now"
    );

    // Grandfathering: re-presenting only already-canonical ops flags no author.
    let empty = update_new_op_authors(&base_state, &base_state).unwrap();
    assert!(empty.is_empty(), "ops the daemon already has are not 'new'");
}

/// B-20 regression: a stale-epoch op that is a *contiguous-clock continuation*
/// of a client already present in the canonical base must still be fenced.
///
/// Live 9c: bob (editor, epoch 2) makes an accepted edit, so his epoch-2 client
/// becomes canonical. He is demoted to viewer then re-promoted to editor (epoch
/// jumps to 4), but his editor never rotated off the epoch-2 client (no rejoin),
/// so a viewer-interval edit rides that *still-canonical* client. Because the
/// op merely extends an existing lineage, the incoming update's own state vector
/// omits it — the pre-fix fence saw "no new authors" and let it cascade. The
/// fix integrates the update against the authoritative state and catches the
/// clock advance.
#[test]
fn update_new_op_authors_flags_contiguous_stale_continuation() {
    let fp = "ed25519:bob";
    let c_e2 = derive_kb_client_id(fp, 2); // canonical via an accepted edit (9b)
    let c_now = derive_kb_client_id(fp, 4); // current epoch after demote->promote

    // Owner seeds the node; bob (epoch 2) makes the accepted edit -> the daemon's
    // authoritative state now contains bob's epoch-2 client.
    let owner = KbNodeDoc::new_with_client_id("n", "Original", "body", &[], 999_111);
    let mut bob = KbNodeDoc::from_bytes_with_client_id(&owner.encode_state(), c_e2).unwrap();
    let accepted = bob.set_title("POST-GRANT-EDIT");
    let mut daemon = KbNodeDoc::from_bytes(&owner.encode_state()).unwrap();
    daemon.apply_update(&accepted).unwrap();
    let base_state = daemon.encode_state(); // authoritative state the fence sees

    // bob (still epoch 2) appends a viewer-interval edit -> contiguous extension.
    let stale_update = bob.set_title("VIEWER-ERA");
    let authors = update_new_op_authors(&stale_update, &base_state).unwrap();
    assert!(
        authors.contains(&c_e2),
        "the contiguous stale-epoch continuation must be attributable (B-20)"
    );
    assert!(
        authors.iter().any(|a| *a != c_now),
        "fence MUST reject: a stale-epoch (c_e2) author is present though c_now is epoch 4"
    );
}

#[test]
fn collection_encode_decode_roundtrip() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.add_node("n1", "Node One");
    coll.add_member("bob");

    let bytes = coll.encode_state();
    let restored = KbCollectionDoc::from_bytes(&bytes).unwrap();
    assert_eq!(restored.name(), "KB1");
    assert_eq!(restored.creator(), "alice");
    assert_eq!(restored.node_count(), 1);
    assert_eq!(restored.members().len(), 2);
}

#[test]
fn collection_two_client_merge() {
    let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
    let state = coll_a.encode_state();
    let mut coll_b = KbCollectionDoc::from_bytes(&state).unwrap();

    let u_a = coll_a.add_node("n1", "From A");
    let u_b = coll_b.add_node("n2", "From B");

    coll_a.apply_update(&u_b).unwrap();
    coll_b.apply_update(&u_a).unwrap();

    assert_eq!(coll_a.node_count(), 2);
    assert_eq!(coll_b.node_count(), 2);

    let nodes_a = coll_a.list_nodes();
    let nodes_b = coll_b.list_nodes();
    assert_eq!(nodes_a.len(), nodes_b.len());
}

// --- ADR-018 v2 schema: owner / roles / policy / pending ---

#[test]
fn role_hierarchy_includes() {
    assert!(Role::Owner.includes(Role::Editor));
    assert!(Role::Owner.includes(Role::Viewer));
    assert!(Role::Editor.includes(Role::Viewer));
    assert!(!Role::Viewer.includes(Role::Editor));
    assert!(!Role::Editor.includes(Role::Owner));
}

#[test]
fn collection_v2_new_owned_seeds_owner_role_policy() {
    let coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
    assert_eq!(coll.schema_version(), 2);
    assert_eq!(coll.owner(), "SHA256:owner");
    assert_eq!(coll.owner_label(), "alice");
    assert_eq!(coll.role_of("SHA256:owner"), Some(Role::Owner));
    assert_eq!(coll.join_policy(), JoinPolicy::Invite);
    assert!(coll.pending().is_empty());
    assert_eq!(coll.member_roles().len(), 1);
}

#[test]
fn collection_v2_roles_and_upsert() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
    coll.upsert_member("SHA256:bob", "bob", Role::Editor);
    assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
    coll.set_role("SHA256:bob", Role::Viewer);
    assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Viewer));
    coll.remove_principal("SHA256:bob");
    assert_eq!(coll.role_of("SHA256:bob"), None);
}

// --- #72 epoch-fence hardening (security-negative oracles) ---

#[test]
fn epoch_advance_is_not_predictable_counter() {
    // Pre-rotation defense (ADR-023): a role change must NOT advance the epoch
    // to a guessable prev+1, or an attacker precomputes derive(fp, prev+1) and
    // authors viewer-era ops under the future editor client_id.
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
    coll.upsert_member("SHA256:bob", "bob", Role::Viewer); // fresh grant -> epoch 0
    let prev = coll.epoch_of("SHA256:bob");
    let predicted = derive_kb_client_id("SHA256:bob", prev + 1);
    coll.set_role("SHA256:bob", Role::Editor); // role change -> epoch advance
    let actual = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
    assert_ne!(
        predicted, actual,
        "epoch advance must be an unpredictable token, not prev+1"
    );
}

#[test]
fn readd_after_remove_does_not_reuse_clientid() {
    // Monotonicity across remove/re-add (ADR-023): a directly-added editor
    // authors under derive(fp, 0). If remove+re-add resets to epoch 0, their
    // pre-removal lineage is silently un-fenced. The re-added member's
    // authoring client_id MUST differ from the pre-removal one.
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:owner", "alice");
    coll.upsert_member("SHA256:bob", "bob", Role::Editor); // fresh grant -> epoch 0
    let before = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
    coll.remove_principal("SHA256:bob");
    coll.upsert_member("SHA256:bob", "bob", Role::Editor); // re-add
    let after = derive_kb_client_id("SHA256:bob", coll.epoch_of("SHA256:bob"));
    assert_ne!(
        before, after,
        "re-add must issue a fresh epoch, not reuse the pre-removal client_id"
    );
}

#[test]
fn collection_v2_join_policy() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    assert_eq!(coll.join_policy(), JoinPolicy::Invite);
    coll.set_join_policy(JoinPolicy::Restrictive);
    assert_eq!(coll.join_policy(), JoinPolicy::Restrictive);
}

#[test]
fn transport_policy_logic() {
    // Round-trip.
    for p in [
        TransportPolicy::Hub,
        TransportPolicy::P2p,
        TransportPolicy::Both,
    ] {
        assert_eq!(TransportPolicy::parse(p.as_str()), Some(p));
    }
    assert_eq!(TransportPolicy::parse("nonsense"), None);

    // allows(): the exposure matrix.
    assert!(TransportPolicy::Hub.allows(Transport::Hub));
    assert!(!TransportPolicy::Hub.allows(Transport::P2p));
    assert!(TransportPolicy::P2p.allows(Transport::P2p));
    assert!(!TransportPolicy::P2p.allows(Transport::Hub));
    assert!(TransportPolicy::Both.allows(Transport::Hub));
    assert!(TransportPolicy::Both.allows(Transport::P2p));

    // with(): widening is idempotent; mixing transports ⇒ Both.
    assert_eq!(
        TransportPolicy::Hub.with(Transport::Hub),
        TransportPolicy::Hub
    );
    assert_eq!(
        TransportPolicy::Hub.with(Transport::P2p),
        TransportPolicy::Both
    );
    assert_eq!(
        TransportPolicy::P2p.with(Transport::Hub),
        TransportPolicy::Both
    );
    assert_eq!(
        TransportPolicy::Both.with(Transport::Hub),
        TransportPolicy::Both
    );
}

#[test]
fn collection_transport_policy_defaults_to_hub() {
    // Conservative default: a freshly-shared (or pre-feature) KB is Hub-only —
    // NOT exposed to the mesh until explicitly p2p-shared.
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    assert_eq!(coll.transport_policy(), TransportPolicy::Hub);
    assert!(coll.transport_policy().allows(Transport::Hub));
    assert!(!coll.transport_policy().allows(Transport::P2p));

    // Opt into the mesh.
    coll.set_transport_policy(TransportPolicy::Both);
    assert_eq!(coll.transport_policy(), TransportPolicy::Both);
    assert!(coll.transport_policy().allows(Transport::P2p));
}

#[test]
fn collection_encryption_defaults_to_none_and_round_trips() {
    // ADR-037: a pre-feature / freshly-shared KB is plaintext (absent flag), so
    // v0.14 KBs are unchanged; the owner can opt a KB into E2E.
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    assert_eq!(coll.encryption(), Encryption::None, "absent flag ⇒ None");
    coll.set_encryption(Encryption::E2e);
    assert_eq!(coll.encryption(), Encryption::E2e, "round-trips E2e");
    assert_eq!(Encryption::parse("e2e"), Some(Encryption::E2e));
    assert_eq!(Encryption::parse("bogus"), None);
}

#[test]
fn transport_policy_raw_and_union_widening() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    // Never set ⇒ raw None (distinct from an explicit Hub), effective Hub.
    assert_eq!(coll.transport_policy_raw(), None);
    assert_eq!(coll.transport_policy(), TransportPolicy::Hub);

    // First share over p2p ⇒ P2p-only (set, not unioned with the Hub default).
    coll.set_transport_policy(TransportPolicy::P2p);
    assert_eq!(coll.transport_policy_raw(), Some(TransportPolicy::P2p));

    // A later hub re-share widens P2p ∪ Hub ⇒ Both.
    let widened = coll.transport_policy().union(TransportPolicy::Hub);
    assert_eq!(widened, TransportPolicy::Both);

    // union algebra.
    assert_eq!(
        TransportPolicy::Hub.union(TransportPolicy::Hub),
        TransportPolicy::Hub
    );
    assert_eq!(
        TransportPolicy::Both.union(TransportPolicy::P2p),
        TransportPolicy::Both
    );
}

#[test]
fn collection_v2_pending_then_approve_atomic() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    coll.add_pending("SHA256:bob", "bob", "2026-06-16T00:00:00Z", None, None);
    assert_eq!(coll.pending().len(), 1);
    assert_eq!(coll.role_of("SHA256:bob"), None);
    coll.approve("SHA256:bob", Role::Editor);
    assert!(coll.pending().is_empty(), "approve clears pending");
    assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
    let m = coll
        .member_roles()
        .into_iter()
        .find(|m| m.fingerprint == "SHA256:bob")
        .unwrap();
    assert_eq!(m.label, "bob", "approve carries the pending label");
}

#[test]
fn add_pending_round_trips_the_joiner_pubkey() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    // With a pubkey: pending() recovers it, so the owner can wrap_to_member on approve.
    let pk = [42u8; 32];
    coll.add_pending("SHA256:bob", "bob", "t", Some(&pk), None);
    let bob = coll
        .pending()
        .into_iter()
        .find(|p| p.fingerprint == "SHA256:bob")
        .unwrap();
    assert_eq!(
        bob.pubkey,
        Some(pk),
        "the joiner's pubkey round-trips through the pending record"
    );
    // Without a pubkey (a v1 record): reads back None (backward-compatible).
    coll.add_pending("SHA256:carol", "carol", "t", None, None);
    let carol = coll
        .pending()
        .into_iter()
        .find(|p| p.fingerprint == "SHA256:carol")
        .unwrap();
    assert_eq!(
        carol.pubkey, None,
        "a pubkey-less pending record reads back None"
    );
}

#[test]
fn collection_v2_two_client_member_merge_converges() {
    let mut a =
        KbCollectionDoc::new_owned_with("KB", "SHA256:o", "alice", Some(1), JoinPolicy::Invite);
    let state = a.encode_state();
    let mut b = KbCollectionDoc::from_bytes(&state).unwrap();
    let ua = a.upsert_member("SHA256:bob", "bob", Role::Editor);
    let ub = b.upsert_member("SHA256:carol", "carol", Role::Viewer);
    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();
    for c in [&a, &b] {
        assert_eq!(c.role_of("SHA256:bob"), Some(Role::Editor));
        assert_eq!(c.role_of("SHA256:carol"), Some(Role::Viewer));
    }
    assert_eq!(a.member_roles().len(), b.member_roles().len());
}

#[test]
fn collection_v2_roundtrip_preserves_schema() {
    let mut coll = KbCollectionDoc::new_owned("KB", "SHA256:o", "alice");
    coll.upsert_member("SHA256:bob", "bob", Role::Viewer);
    coll.set_join_policy(JoinPolicy::Permissive);
    coll.add_pending("SHA256:eve", "eve", "t", None, None);
    let bytes = coll.encode_state();
    let r = KbCollectionDoc::from_bytes(&bytes).unwrap();
    assert_eq!(r.schema_version(), 2);
    assert_eq!(r.owner(), "SHA256:o");
    assert_eq!(r.role_of("SHA256:bob"), Some(Role::Viewer));
    assert_eq!(r.join_policy(), JoinPolicy::Permissive);
    assert_eq!(r.pending().len(), 1);
}

/// ADR-037 #167/#168 — on an E2e KB a deletion is SEALED into a client-id-stamped
/// outer op-set op, so `update_new_op_authors` attributes it to the SEAL client_id and
/// the ADR-023 fence rejects a stale-epoch sealed delete. This is WHY #168's always-seal
/// closes #167's deletion-fence gap for E2e KBs. (Contrast: a PLAINTEXT pure-delete is
/// unattributable — yrs tombstones carry no deleter — the residual #167 gap on the
/// unencrypted path, which needs a separate deleter-attribution design.)
#[test]
fn update_new_op_authors_attributes_a_sealed_delete_to_the_seal_client() {
    use crate::content_crypto::ContentKey;
    use crate::op_set;
    let key = ContentKey::generate();
    let mut node = KbNodeDoc::new_with_client_id("n", "T", "secret-body", &[], 1);
    let create = node.encode_state();
    let inner_delete = node.set_body(""); // a pure delete at the plaintext layer
                                          // Owner seals the create (op-set base); the attacker seals the delete at a STALE epoch.
    let valid_cid = derive_kb_client_id("SHA256:owner", 0);
    let (_i0, outer0) = op_set::seal_op(&[], &key, &create, valid_cid).unwrap();
    let base = op_set::merge(&[], &outer0).unwrap();
    let stale_cid = derive_kb_client_id("SHA256:attacker", 0);
    let (_i1, outer1) = op_set::seal_op(&base, &key, &inner_delete, stale_cid).unwrap();
    // The fence's author extraction reports the STALE seal client (so it rejects it),
    // even though the INNER op is a pure (otherwise-unattributable) delete.
    let authors = update_new_op_authors(&outer1, &base).unwrap();
    assert!(
        authors.contains(&stale_cid),
        "the sealed delete's outer op carries the stale seal client_id ⇒ the fence catches it"
    );
    assert!(
        !authors.contains(&valid_cid),
        "the prior op-set base is grandfathered, not re-reported as a new author"
    );
}
