//! Removing a member who has ROTATED their key (ADR-040 `Rebind`) — the attacker's tests.
//!
//! ADR-040 §1 makes a `Rebind` say "`new_fp` is the same principal, going forward", so a
//! member is ONE principal (one *seat*) across its whole rebind chain. ADR-026's strong
//! removal therefore has to act on the seat, not on whichever fingerprint the remover
//! happened to name: an owner's `Remove` naming ANY key the seat has held evicts it, no
//! step of the derivation may re-insert it, and a removal concurrent with a rotation
//! dominates the rotation (the rotation is the removed member's concurrent action).
//!
//! Every test here names the key the attacker would rely on, and checks the member has no
//! access under ANY key of the chain — not just the one the removal named.

use super::tests::{admit_admin, genesis, governed, id, make, make_rebind, Id};
use super::*;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};

const NOW: u64 = 100;

fn admit(by: &Id, who: &Id, role: Role, can_invite: bool, prev: &str) -> SignedMembershipOp {
    make(
        by,
        MembershipAction::Admit,
        &who.fp,
        Some(role),
        can_invite,
        None,
        prev,
    )
}

fn remove(by: &Id, subject: &Id, prev: &str) -> SignedMembershipOp {
    make(
        by,
        MembershipAction::Remove,
        &subject.fp,
        None,
        false,
        None,
        prev,
    )
}

fn set_role(by: &Id, subject: &Id, role: Role, prev: &str) -> SignedMembershipOp {
    make(
        by,
        MembershipAction::SetRole,
        &subject.fp,
        Some(role),
        false,
        None,
        prev,
    )
}

fn derive(ops: &[SignedMembershipOp], owner: &Id) -> BTreeMap<String, ValidMember> {
    derive_valid_members(ops, &owner.pubkey, NOW)
}

/// Assert none of `keys` holds a seat — the removed principal has no access under any key.
fn assert_no_access(m: &BTreeMap<String, ValidMember>, keys: &[&Id], why: &str) {
    for k in keys {
        assert!(
            !m.contains_key(&k.fp),
            "{why}: key {} still holds a seat; members = {:?}",
            k.fp,
            m.keys().collect::<Vec<_>>()
        );
    }
}

/// Owner, an inviting Editor `bob`, and `bob`'s rotation to `bob2`.
struct Rotated {
    owner: Id,
    bob: Id,
    bob2: Id,
    g: SignedMembershipOp,
    admit_bob: SignedMembershipOp,
    rebind: SignedMembershipOp,
}

fn rotated() -> Rotated {
    let owner = id(1);
    let bob = id(2);
    let bob2 = id(3);
    let g = genesis(&owner);
    let admit_bob = admit(&owner, &bob, Role::Editor, true, &g.chain_hash());
    let rebind = make_rebind(&bob, &bob2, &admit_bob.chain_hash());
    Rotated {
        owner,
        bob,
        bob2,
        g,
        admit_bob,
        rebind,
    }
}

impl Rotated {
    fn base(&self) -> Vec<SignedMembershipOp> {
        vec![self.g.clone(), self.admit_bob.clone(), self.rebind.clone()]
    }
}

/// The reported bug, successor-key form: the owner removes the key bob rotated INTO. The
/// successor has no `Admit` of its own and the rotation post-pass re-inserted it after the
/// removal, so the removal was valid and had no effect.
#[test]
fn removing_the_successor_key_evicts_the_rotated_member() {
    let r = rotated();
    let rm = remove(&r.owner, &r.bob2, &r.rebind.chain_hash());
    // The removed member then tries to act with the key it still holds.
    let carol = id(4);
    let after = admit(&r.bob2, &carol, Role::Viewer, false, &rm.chain_hash());
    let mut ops = r.base();
    ops.extend([rm, after]);
    let m = derive(&ops, &r.owner);
    assert_no_access(&m, &[&r.bob, &r.bob2, &carol], "Remove(successor)");
    assert!(m.contains_key(&r.owner.fp), "the owner is untouched");
}

/// The predecessor-key form: the owner knows bob only by the key it admitted and names
/// THAT one after bob rotated. ADR-040 §1 says the two keys are one principal, so the
/// removal must evict the seat — it used to be refused as "not a current member".
#[test]
fn removing_the_predecessor_key_evicts_the_rotated_member() {
    let r = rotated();
    let rm = remove(&r.owner, &r.bob, &r.rebind.chain_hash());
    let mut ops = r.base();
    ops.push(rm);
    let m = derive(&ops, &r.owner);
    assert_no_access(&m, &[&r.bob, &r.bob2], "Remove(predecessor)");
}

/// Every key of a longer chain a→b→c names the seat: removing the OLDEST key evicts the
/// member who now holds the newest.
#[test]
fn removing_any_key_of_a_chained_rotation_evicts_the_member() {
    let owner = id(1);
    let (a, b, c) = (id(2), id(3), id(4));
    let g = genesis(&owner);
    let adm = admit(&owner, &a, Role::Editor, false, &g.chain_hash());
    let r1 = make_rebind(&a, &b, &adm.chain_hash());
    let r2 = make_rebind(&b, &c, &r1.chain_hash());
    for named in [&a, &b, &c] {
        let rm = remove(&owner, named, &r2.chain_hash());
        let ops = vec![g.clone(), adm.clone(), r1.clone(), r2.clone(), rm];
        let m = derive(&ops, &owner);
        assert_no_access(&m, &[&a, &b, &c], "Remove(any chain key)");
    }
}

/// A rotation causally AFTER the removal cannot resurrect the seat — neither with the key
/// the removal named nor with the other key the removed member still holds.
#[test]
fn a_rotation_after_removal_cannot_resurrect_the_seat() {
    let r = rotated();
    let bob3 = id(5);
    for removed in [&r.bob, &r.bob2] {
        let rm = remove(&r.owner, removed, &r.rebind.chain_hash());
        for signer in [&r.bob, &r.bob2] {
            let escape = make_rebind(signer, &bob3, &rm.chain_hash());
            let mut ops = r.base();
            ops.extend([rm.clone(), escape]);
            let m = derive(&ops, &r.owner);
            assert_no_access(
                &m,
                &[&r.bob, &r.bob2, &bob3],
                "post-removal rotation must not resurrect",
            );
        }
    }
}

/// The owner's re-key after a removal (`author_rotate_on_remove`) re-asserts every
/// remaining member with a fresh `Admit` — including a rotated member, at its successor
/// key. That Admit continues the same seat, so removing the member afterwards by its OLD
/// key must still evict it; the re-key Admit must not become a second, unremovable seat.
#[test]
fn a_rekey_admit_of_the_successor_does_not_shield_it_from_removal() {
    let r = rotated();
    let rekey = admit(
        &r.owner,
        &r.bob2,
        Role::Editor,
        true,
        &r.rebind.chain_hash(),
    );
    let rm = remove(&r.owner, &r.bob, &rekey.chain_hash());
    let mut ops = r.base();
    ops.extend([rekey, rm]);
    let m = derive(&ops, &r.owner);
    assert_no_access(
        &m,
        &[&r.bob, &r.bob2],
        "Remove(old key) after a re-key admit",
    );
}

/// The same defect without any rotation: remove, re-admit, remove again. Only the FIRST
/// removal of a key was ever consulted, and the re-admit was causally after it, so the
/// second removal was valid and inert.
#[test]
fn a_second_removal_after_a_re_admit_takes_effect() {
    let owner = id(1);
    let bob = id(2);
    let g = genesis(&owner);
    let a1 = admit(&owner, &bob, Role::Editor, false, &g.chain_hash());
    let r1 = remove(&owner, &bob, &a1.chain_hash());
    let a2 = admit(&owner, &bob, Role::Editor, false, &r1.chain_hash());
    let mid = derive(&[g.clone(), a1.clone(), r1.clone(), a2.clone()], &owner);
    assert!(mid.contains_key(&bob.fp), "the re-admit restores bob");
    let r2 = remove(&owner, &bob, &a2.chain_hash());
    let m = derive(&[g, a1, r1, a2, r2], &owner);
    assert_no_access(&m, &[&bob], "second removal");
}

/// Demotion has the same shape as removal: a `SetRole` naming the successor key was valid
/// and never applied, so an owner could not downgrade a member who had rotated.
#[test]
fn demoting_a_rotated_member_takes_effect_under_either_key() {
    let r = rotated();
    for named in [&r.bob, &r.bob2] {
        let demote = set_role(&r.owner, named, Role::Viewer, &r.rebind.chain_hash());
        let mut ops = r.base();
        ops.push(demote);
        let m = derive(&ops, &r.owner);
        assert_eq!(
            m.get(&r.bob2.fp).map(|x| x.role),
            Some(Role::Viewer),
            "SetRole naming {} reaches the rotated seat",
            named.fp
        );
        assert!(!m.contains_key(&r.bob.fp), "the old key stays retired");
    }
}

// --- the legitimate paths must keep working ---

/// A rotated member who is NOT removed keeps the seat at the successor key, with its exact
/// role, and the removal of an UNRELATED member does not touch it.
#[test]
fn a_rotated_member_who_is_not_removed_keeps_access() {
    let r = rotated();
    let dave = id(6);
    let admit_dave = admit(&r.owner, &dave, Role::Viewer, false, &r.rebind.chain_hash());
    let rm_dave = remove(&r.owner, &dave, &admit_dave.chain_hash());
    let mut ops = r.base();
    ops.extend([admit_dave, rm_dave]);
    let m = derive(&ops, &r.owner);
    assert_eq!(m.get(&r.bob2.fp).map(|x| x.role), Some(Role::Editor));
    assert!(!m.contains_key(&r.bob.fp), "predecessor still retired");
    assert!(!m.contains_key(&dave.fp), "the unrelated removal applied");
}

/// A re-admit causally after the removal brings the member back — at the successor key
/// with the role the RE-ADMIT grants (not the pre-removal role carried over), or at the
/// old key as a fresh seat.
#[test]
fn a_re_admit_after_removing_a_rotated_member_works() {
    let r = rotated();
    let rm = remove(&r.owner, &r.bob2, &r.rebind.chain_hash());
    let readmit_new = admit(&r.owner, &r.bob2, Role::Viewer, false, &rm.chain_hash());
    let mut ops = r.base();
    ops.extend([rm.clone(), readmit_new]);
    let m = derive(&ops, &r.owner);
    assert_eq!(
        m.get(&r.bob2.fp).map(|x| (x.role, x.can_invite)),
        Some((Role::Viewer, false)),
        "re-admitted at the successor key with the re-admit's grant"
    );
    assert!(!m.contains_key(&r.bob.fp));

    let readmit_old = admit(&r.owner, &r.bob, Role::Viewer, false, &rm.chain_hash());
    let mut ops = r.base();
    ops.extend([rm, readmit_old]);
    let m = derive(&ops, &r.owner);
    assert_eq!(m.get(&r.bob.fp).map(|x| x.role), Some(Role::Viewer));
    assert!(
        !m.contains_key(&r.bob2.fp),
        "the removed seat stays removed"
    );
}

/// Rotating INTO a fingerprint that was removed earlier is judged on the rotation's own
/// causal point: that earlier removal is in the rebind's past, so it does not evict the
/// new seat.
#[test]
fn an_earlier_removal_of_the_successor_fingerprint_does_not_evict_the_new_seat() {
    let r = rotated();
    let x = id(7);
    let adm_x = admit(&r.owner, &x, Role::Viewer, false, &r.g.chain_hash());
    let rm_x = remove(&r.owner, &x, &adm_x.chain_hash());
    let admit_bob = admit(&r.owner, &r.bob, Role::Editor, false, &rm_x.chain_hash());
    let into_x = make_rebind(&r.bob, &x, &admit_bob.chain_hash());
    let m = derive(&[r.g.clone(), adm_x, rm_x, admit_bob, into_x], &r.owner);
    assert_eq!(m.get(&x.fp).map(|v| v.role), Some(Role::Editor));
}

/// Provenance follows the rotation: under `CascadeAll`, a rotated inviter's invitees are
/// NOT orphaned by the rotation — but they ARE cascaded once the inviter's seat is removed,
/// whichever key the removal named.
#[test]
fn a_rotated_inviters_invitees_survive_the_rotation_and_fall_with_the_seat() {
    let r = rotated();
    let carol = id(8);
    let view = MembershipView {
        cascade: InviterRemovalPolicy::CascadeAll,
        ..MembershipView::default()
    };
    let invite = admit(
        &r.bob,
        &carol,
        Role::Viewer,
        false,
        &r.admit_bob.chain_hash(),
    );
    let rebind = make_rebind(&r.bob, &r.bob2, &invite.chain_hash());
    let ops = vec![r.g.clone(), r.admit_bob.clone(), invite, rebind.clone()];
    let m = derive_valid_members_with(&ops, &r.owner.pubkey, NOW, &view);
    assert_eq!(
        m.get(&carol.fp).map(|c| c.invited_by.clone()),
        Some(r.bob2.fp.clone()),
        "the invitee traces to the inviter's successor and is not cascaded"
    );
    for named in [&r.bob, &r.bob2] {
        let mut ops = ops.clone();
        ops.push(remove(&r.owner, named, &rebind.chain_hash()));
        let m = derive_valid_members_with(&ops, &r.owner.pubkey, NOW, &view);
        assert_no_access(&m, &[&r.bob, &r.bob2, &carol], "inviter seat removed");
    }
}

/// Re-sign `op` with `epoch` bumped until `pred(op)` holds — a way to place a sibling on a
/// chosen side of another in ADR-042's order (siblings ascend by `chain_hash`) without
/// hand-picking a hash.
fn resign_until(
    mut op: SignedMembershipOp,
    by: &Id,
    pred: impl Fn(&SignedMembershipOp) -> bool,
) -> SignedMembershipOp {
    for e in 0..64 {
        op.op.epoch = e;
        op.sig = op.op.sign(&by.secret);
        if pred(&op) {
            return op;
        }
    }
    panic!("no epoch in 0..64 satisfies the ordering predicate");
}

/// A `SetRole` overlays only a grant it causally follows. A demotion concurrent with a
/// re-admit (e.g. the owner's re-key) does not reach the re-admit's grant even when it
/// sorts after it — the same "a SetRole applies to the admits in its past" rule the
/// derivation had before seats.
#[test]
fn a_set_role_does_not_overlay_a_concurrent_re_admit() {
    let owner = id(1);
    let bob = id(2);
    let g = genesis(&owner);
    let a1 = admit(&owner, &bob, Role::Editor, false, &g.chain_hash());
    let a2 = admit(&owner, &bob, Role::Editor, false, &a1.chain_hash());
    let demote = set_role(&owner, &bob, Role::Viewer, &a1.chain_hash());
    let demote = resign_until(demote, &owner, |d| d.chain_hash() > a2.chain_hash());
    let m = converged(&[g, a1, a2, demote], &owner, 5);
    assert_eq!(m.get(&bob.fp).map(|b| b.role), Some(Role::Editor));
}

/// Two live seats claiming one key — a member rotating INTO a key the owner concurrently
/// admits — resolve to the seat whose latest binding comes later in ADR-042's causal order,
/// identically on every peer.
#[test]
fn two_seats_on_one_key_resolve_to_the_later_binding() {
    let r = rotated();
    let x = id(9);
    for later_is_rebind in [true, false] {
        let rebind = make_rebind(&r.bob, &x, &r.admit_bob.chain_hash());
        let adm_x = admit(&r.owner, &x, Role::Viewer, false, &r.admit_bob.chain_hash());
        let adm_x = resign_until(adm_x, &r.owner, |a| {
            (a.chain_hash() < rebind.chain_hash()) == later_is_rebind
        });
        let ops = [r.g.clone(), r.admit_bob.clone(), rebind, adm_x];
        let m = converged(&ops, &r.owner, 5);
        let want = if later_is_rebind {
            Role::Editor
        } else {
            Role::Viewer
        };
        assert_eq!(m.get(&x.fp).map(|v| v.role), Some(want));
    }
}

// --- owner protection must survive the owner's own rotation ---

/// Under single-owner governance the owner is irrevocable through the chain. Once the
/// owner rotates, a `Remove` naming the successor key — by an admin or by the owner itself
/// — must not evict the owner now that removal reaches rotated seats.
#[test]
fn a_rotated_single_owner_stays_irrevocable() {
    let owner = id(1);
    let owner2 = id(2);
    let alice = id(3);
    let g = genesis(&owner);
    let admit_alice = admit_admin(&owner, &alice, &g.chain_hash());
    let rot = make_rebind(&owner, &owner2, &admit_alice.chain_hash());
    for (by, named) in [
        (&alice, &owner2),
        (&alice, &owner),
        (&owner2, &owner2),
        (&owner2, &owner),
    ] {
        let rm = remove(by, named, &rot.chain_hash());
        let ops = vec![g.clone(), admit_alice.clone(), rot.clone(), rm];
        let m = derive(&ops, &owner);
        assert_eq!(
            m.get(&owner2.fp).map(|x| x.role),
            Some(Role::Owner),
            "{} removing {} must not evict the rotated owner",
            by.fp,
            named.fp
        );
    }
}

/// Under quorum, co-signatures naming DIFFERENT keys of the same rotated member count as
/// votes against one principal; a single admin's vote still does not reach the threshold.
#[test]
fn quorum_votes_naming_different_keys_of_one_member_combine() {
    let owner = id(1);
    let (a, b, c, c2) = (id(2), id(3), id(4), id(5));
    let g = genesis(&owner);
    let ad_a = admit_admin(&owner, &a, &g.chain_hash());
    let ad_b = admit_admin(&owner, &b, &ad_a.chain_hash());
    let ad_c = admit_admin(&owner, &c, &ad_b.chain_hash());
    let rot = make_rebind(&c, &c2, &ad_c.chain_hash());
    let vote_a = remove(&a, &c, &rot.chain_hash());
    let vote_b = remove(&b, &c2, &vote_a.chain_hash());
    let gov = Governance::Quorum { threshold: 2 };
    let base = vec![g, ad_a, ad_b, ad_c, rot];

    let mut one = base.clone();
    one.push(vote_a.clone());
    assert!(
        governed(&one, &owner, gov).contains_key(&c2.fp),
        "one vote is below the threshold"
    );

    let mut two = base;
    two.extend([vote_a, vote_b]);
    let m = governed(&two, &owner, gov);
    assert_no_access(&m, &[&c, &c2], "two votes naming different keys");
}

// --- concurrency: every peer derives the same result, and it is the removal ---

/// Derive `ops` as `peers` peers would, each receiving them in its own seeded random
/// order, and assert they all agree. Returns the agreed membership.
fn converged(ops: &[SignedMembershipOp], owner: &Id, peers: u64) -> BTreeMap<String, ValidMember> {
    let mut agreed: Option<BTreeMap<String, ValidMember>> = None;
    for seed in 0..peers {
        let mut mine = ops.to_vec();
        mine.shuffle(&mut StdRng::seed_from_u64(0x5ea7_0000 + seed));
        let m = derive(&mine, owner);
        match &agreed {
            None => agreed = Some(m),
            Some(a) => assert_eq!(a, &m, "peer {seed} diverged"),
        }
    }
    agreed.expect("at least one peer")
}

/// A `Remove` and a `Rebind` neither of which is causally before the other. Each case puts
/// them on sibling branches of the op tree; the removed member also keeps acting on its
/// own branch. Whatever order the ops arrive in, every peer must derive the removal — the
/// rotation is the removed member's concurrent action, which ADR-026 strong removal voids.
#[test]
fn a_concurrent_removal_and_rotation_converge_to_removal_on_every_peer() {
    let r = rotated();
    let (bob3, carol) = (id(5), id(6));

    // (1) Remove(bob) ∥ Rebind(bob→bob2); bob2 then admits carol on its branch.
    let rm = remove(&r.owner, &r.bob, &r.admit_bob.chain_hash());
    let via_bob2 = admit(&r.bob2, &carol, Role::Viewer, false, &r.rebind.chain_hash());
    let mut case1 = r.base();
    case1.extend([rm, via_bob2]);

    // (2) after bob→bob2: Remove(bob2) ∥ Rebind(bob2→bob3).
    let rm2 = remove(&r.owner, &r.bob2, &r.rebind.chain_hash());
    let escape = make_rebind(&r.bob2, &bob3, &r.rebind.chain_hash());
    let mut case2 = r.base();
    case2.extend([rm2, escape.clone()]);

    // (3) after bob→bob2: Remove(bob) — the OLD key — ∥ Rebind(bob2→bob3).
    let rm_old = remove(&r.owner, &r.bob, &r.rebind.chain_hash());
    let mut case3 = r.base();
    case3.extend([rm_old, escape]);

    for (n, ops) in [case1, case2, case3].iter().enumerate() {
        let m = converged(ops, &r.owner, 7);
        assert_no_access(
            &m,
            &[&r.bob, &r.bob2, &bob3, &carol],
            &format!("concurrent case {}", n + 1),
        );
        assert!(m.contains_key(&r.owner.fp));
    }
}

/// A rebind INTO a key another seat has retired is refused (review finding). Otherwise an
/// admin rebinding onto the owner's pre-rotation key derives under it, `naming` that key
/// finds the genesis seat, and a `Remove` of the admin by the key the member map shows is
/// refused as owner-protected; onto a member's retired key, a `Remove` of that member
/// would also evict the rebinder.
#[test]
fn a_rebind_into_another_seats_retired_key_is_refused() {
    // Shape 1: onto the owner's retired key.
    let owner = id(1);
    let owner2 = id(2);
    let mallory = id(3);
    let g = genesis(&owner);
    let adm = admit_admin(&owner, &mallory, &g.chain_hash());
    let rot = make_rebind(&owner, &owner2, &adm.chain_hash());
    let grab = make_rebind(&mallory, &owner, &rot.chain_hash());
    let ops = vec![g.clone(), adm, rot.clone(), grab.clone()];
    let m = derive(&ops, &owner);
    assert!(
        m.contains_key(&mallory.fp),
        "the rebind is refused: mallory keeps her key"
    );
    assert!(
        !m.contains_key(&owner.fp),
        "nobody derives under the owner's retired key"
    );
    // ...so the owner removes her by the key the member map shows.
    let rm = remove(&owner2, &mallory, &grab.chain_hash());
    let mut ops2 = ops.clone();
    ops2.push(rm);
    let m2 = derive(&ops2, &owner);
    assert_no_access(
        &m2,
        &[&mallory, &owner],
        "Remove(mallory) after a refused grab",
    );
    assert!(
        m2.contains_key(&owner2.fp),
        "the rotated owner is untouched"
    );

    // Shape 2: onto a member's retired key; removing that member must not hit the rebinder.
    let r = rotated();
    let dave = id(5);
    let adm_d = admit(&r.owner, &dave, Role::Editor, false, &r.rebind.chain_hash());
    let grab_d = make_rebind(&dave, &r.bob, &adm_d.chain_hash());
    let rm_bob = remove(&r.owner, &r.bob, &grab_d.chain_hash());
    let mut ops3 = r.base();
    ops3.extend([adm_d, grab_d, rm_bob]);
    let m3 = derive(&ops3, &r.owner);
    assert_no_access(&m3, &[&r.bob, &r.bob2], "Remove(bob's retired key)");
    assert!(
        m3.contains_key(&dave.fp),
        "dave's refused rebind leaves him seated as himself"
    );
}
