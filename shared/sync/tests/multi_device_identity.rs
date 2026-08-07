//! Multi-device identity spike (CC3) — gates ADR-098 (durable identity for
//! network clients).
//!
//! **The question.** Browser MAE gives every browser profile its own keypair, so
//! one human with a laptop, a desktop and a phone is three Ed25519 fingerprints.
//! MAE's membership is keyed on a fingerprint inside a signed, hash-chained,
//! peer-verifiable op-log (ADR-026). Can one member hold several device keys at
//! once — or must an owner approve every device, on every KB, which does not
//! scale to an AD-managed organisation?
//!
//! ADR-040's `Rebind` op looks like the answer: an old key cross-signs a new one,
//! additively, no history rewrite, explicitly modelled on Matrix cross-signing.
//! This file exists to check whether it actually *is* the answer, by executing it
//! rather than by reading the ADR — the design's own words ("transferred",
//! "retired") suggest succession, not concurrency, and a spike that only reads
//! the prose cannot tell the difference between what was designed and what was
//! built.
//!
//! These tests **document current behaviour**. Several assert limitations rather
//! than capabilities: they are written so that if a future change makes
//! multi-device work, they fail loudly and get rewritten, rather than silently
//! encoding a workaround for a wall that has moved.

use ed25519_dalek::{Signer, SigningKey};

use mae_sync::kb::Role;
use mae_sync::membership::{
    derive_valid_members, find_wrapped_content_key, fingerprint_of, is_owner_principal,
    MembershipAction, MembershipOp, SignedMembershipOp,
};

const KB: &str = "kb-multidevice-spike";
/// Well past every op's `issued_at` so no timebox interferes with what is being
/// measured here.
const NOW: u64 = 10_000;

/// A distinct principal. Seeded rather than random so a failure reproduces, but
/// each identity gets a *different* seed — never one shared "unicorn" key.
struct Principal {
    signing: SigningKey,
    pubkey: [u8; 32],
    fp: String,
}

fn principal(seed: u8) -> Principal {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    let pubkey = signing.verifying_key().to_bytes();
    let fp = fingerprint_of(&pubkey);
    Principal {
        signing,
        pubkey,
        fp,
    }
}

fn sign(op: MembershipOp, by: &Principal) -> SignedMembershipOp {
    let sig = by.signing.sign(&op.canonical_bytes()).to_bytes().to_vec();
    SignedMembershipOp {
        op,
        sig,
        author_pubkey: by.pubkey,
    }
}

fn blank(action: MembershipAction, subject: &str, author: &str) -> MembershipOp {
    MembershipOp {
        kb_id: KB.to_string(),
        action,
        subject: subject.to_string(),
        role: None,
        can_invite: false,
        author: author.to_string(),
        issued_at: 1,
        expires_at: None,
        epoch: 0,
        prev_hash: String::new(),
        wrapped_key: None,
        new_pubkey: None,
        new_wrap_pubkey: None,
        recovery_pubkey: None,
        replication: Default::default(),
    }
}

/// The owner-genesis op: a self-admit with an empty `prev_hash`, which every
/// owner-rooted reader treats as the trust root.
fn genesis(owner: &Principal) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Admit, &owner.fp, &owner.fp);
    op.role = Some(Role::Owner);
    sign(op, owner)
}

fn admit(
    owner: &Principal,
    subject: &Principal,
    role: Role,
    prev: &SignedMembershipOp,
) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Admit, &subject.fp, &owner.fp);
    op.role = Some(role);
    op.prev_hash = prev.chain_hash();
    sign(op, owner)
}

/// A `Rebind`: `from` cross-signs `to` as its successor. Carries both of the
/// successor's published keys, as ADR-040 §1/§3 require.
fn rebind(from: &Principal, to: &Principal, prev: &SignedMembershipOp) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Rebind, &to.fp, &from.fp);
    op.prev_hash = prev.chain_hash();
    op.new_pubkey = Some(to.pubkey);
    // The X25519 wrap key is a distinct key (ADR-041); for this spike only its
    // presence matters, not its curve, since nothing here performs a real ECDH.
    op.new_wrap_pubkey = Some(to.pubkey);
    sign(op, from)
}

fn member_fps(ops: &[SignedMembershipOp], owner: &Principal) -> Vec<String> {
    let mut fps: Vec<String> = derive_valid_members(ops, &owner.pubkey, NOW)
        .into_keys()
        .collect();
    fps.sort();
    fps
}

// ---------------------------------------------------------------------------
// Finding 1 — a member cannot hold two devices. Rebind is succession.
// ---------------------------------------------------------------------------

/// The central negative result. Alice is admitted on her laptop, then enrols a
/// phone the only way the op-log allows. The phone works — and **the laptop
/// stops being a member**.
///
/// This is the whole CC3 problem, executable: "add a device" and "replace a
/// device" are the same operation, so a browser device enrolment silently logs
/// the user out everywhere else.
#[test]
fn enrolling_a_second_device_via_rebind_retires_the_first() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let a = admit(&owner, &laptop, Role::Editor, &g);
    let r = rebind(&laptop, &phone, &a);

    let before = member_fps(&[g.clone(), a.clone()], &owner);
    assert!(
        before.contains(&laptop.fp),
        "precondition: the laptop is a member before the rebind"
    );

    let after = member_fps(&[g, a, r], &owner);
    assert!(
        after.contains(&phone.fp),
        "the newly enrolled phone must be a member"
    );
    assert!(
        !after.contains(&laptop.fp),
        "FINDING: the laptop is retired by enrolling the phone — `Rebind` is succession, \
         not concurrent multi-device. If this assertion ever fails, multi-device became \
         expressible and ADR-098's design must be revisited."
    );
}

/// Fanning out from one identity to several devices does not work either: once
/// the laptop has rebound to the phone it is retired, so its attempt to enrol a
/// third device contributes nothing.
///
/// This closes the obvious workaround — "just rebind once per device" — and is
/// why ADR-098 cannot treat multi-device as an authoring convention over the
/// existing op set.
#[test]
fn a_retired_device_cannot_enrol_a_further_device() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);
    let desktop = principal(4);

    let g = genesis(&owner);
    let a = admit(&owner, &laptop, Role::Editor, &g);
    let r1 = rebind(&laptop, &phone, &a);
    // The laptop — now retired — tries to add a third device.
    let r2 = rebind(&laptop, &desktop, &r1);

    let after = member_fps(&[g, a, r1, r2], &owner);
    assert!(
        after.contains(&phone.fp),
        "the phone remains the live device"
    );
    assert!(
        !after.contains(&desktop.fp),
        "FINDING: a retired device cannot enrol another — there is no fan-out path"
    );
    assert_eq!(
        after.len(),
        2,
        "exactly the owner and one live device, never a device set: {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Finding 2 — the OWNER is different, and the difference is not in the ADR.
// ---------------------------------------------------------------------------

/// `owner_principal_chain` is a forward-closure **set** that never retires a
/// predecessor, so an owner who rotates keeps *both* keys authoritative — the
/// owner already has working multi-device, by a mechanism the member path lacks.
///
/// ADR-040 §2 describes rebind uniformly as transfer-and-retire, and its threat
/// model states "a retired key's post-rebind ops are fenced". That is true for
/// members and **not** for the owner. The asymmetry is deliberate and reasoned in
/// the code comment on `owner_principal_chain`, but it is not in the ADR — worth
/// recording, because a rotated-away owner key retains full authority forever,
/// which matters for how ADR-098 treats owner devices.
#[test]
fn an_owner_keeps_every_rotated_key_authoritative_unlike_a_member() {
    let owner = principal(1);
    let owner_phone = principal(5);

    let g = genesis(&owner);
    let r = rebind(&owner, &owner_phone, &g);
    let ops = vec![g, r];

    assert!(
        is_owner_principal(&ops, &owner.pubkey, &owner_phone.fp),
        "the owner's new device must be an owner principal"
    );
    assert!(
        is_owner_principal(&ops, &owner.pubkey, &owner.fp),
        "FINDING: the owner's ORIGINAL key is still an owner principal after rebinding — \
         the owner chain is an additive set, while the member path retires. This asymmetry \
         is not documented in ADR-040."
    );
}

// ---------------------------------------------------------------------------
// Finding 3 — E2E content access is owner-gated per device, structurally.
// ---------------------------------------------------------------------------

/// Even where membership resolves, an E2E KB's content key is delivered by an
/// op wrapping it *to a specific key*. A newly enrolled device has no such op
/// until the owner authors one, so it can be a full member and still read
/// nothing.
///
/// ADR-040 §3 calls this "the correct authority boundary" and it is right for
/// key rotation. For multi-device it is the scaling wall: N devices × M KBs
/// owner actions. Any ADR-098 design that gives a user several devices must say
/// how the content key reaches device N without an owner round-trip — which is
/// the problem Matrix solves with passphrase-backed secret storage (SSSS), and
/// which ADR-040 explicitly deferred as its open question Q2.
#[test]
fn a_newly_enrolled_device_holds_no_content_key_until_an_owner_wrap_exists() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);

    // The laptop is admitted WITH a content-key wrap, as an E2E admit does.
    let mut admit_op = blank(MembershipAction::Admit, &laptop.fp, &owner.fp);
    admit_op.role = Some(Role::Editor);
    admit_op.prev_hash = g.chain_hash();
    admit_op.wrapped_key = Some(b"wrapped-to-the-laptop".to_vec());
    let a = sign(admit_op, &owner);

    let r = rebind(&laptop, &phone, &a);
    let ops = vec![g, a, r];

    assert_eq!(
        find_wrapped_content_key(&ops, &owner.pubkey, &laptop.fp).as_deref(),
        Some(&b"wrapped-to-the-laptop"[..]),
        "precondition: the laptop was delivered a content key"
    );
    assert!(
        find_wrapped_content_key(&ops, &owner.pubkey, &phone.fp).is_none(),
        "FINDING: the enrolled phone has no content key and cannot read an E2E KB until the \
         OWNER authors a re-wrap to it — an owner action per device, per KB"
    );
}

/// The complement, so the finding above is a real boundary rather than an
/// artefact of how this spike builds ops: once the owner *does* author the
/// re-wrap, the new device resolves. Without this, the previous test could pass
/// against a `find_wrapped_content_key` that simply never finds anything.
#[test]
fn the_owner_re_wrap_does_deliver_the_key_to_the_new_device() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let mut admit_op = blank(MembershipAction::Admit, &laptop.fp, &owner.fp);
    admit_op.role = Some(Role::Editor);
    admit_op.prev_hash = g.chain_hash();
    admit_op.wrapped_key = Some(b"wrapped-to-the-laptop".to_vec());
    let a = sign(admit_op, &owner);
    let r = rebind(&laptop, &phone, &a);

    // The owner observes the rebind and re-wraps the current content key.
    let mut rewrap = blank(MembershipAction::Admit, &phone.fp, &owner.fp);
    rewrap.role = Some(Role::Editor);
    rewrap.prev_hash = r.chain_hash();
    rewrap.wrapped_key = Some(b"wrapped-to-the-phone".to_vec());
    let w = sign(rewrap, &owner);

    let ops = vec![g, a, r, w];
    assert_eq!(
        find_wrapped_content_key(&ops, &owner.pubkey, &phone.fp).as_deref(),
        Some(&b"wrapped-to-the-phone"[..]),
        "the owner's re-wrap must reach the new device — proving the previous test measured \
         a real authority boundary, not a broken lookup"
    );
}

// ---------------------------------------------------------------------------
// Adversarial: the enrolment path must not become a privilege-escalation path.
// ---------------------------------------------------------------------------

/// A device enrolment must never grant more than the enrolling identity held.
/// Whatever ADR-098 designs, this property has to survive — so it is pinned here
/// against the mechanism that exists today.
#[test]
fn enrolling_a_device_cannot_elevate_the_role_it_inherits() {
    let owner = principal(1);
    let viewer = principal(6);
    let viewer_phone = principal(7);

    let g = genesis(&owner);
    let a = admit(&owner, &viewer, Role::Viewer, &g);
    let r = rebind(&viewer, &viewer_phone, &a);

    let members = derive_valid_members(&[g, a, r], &owner.pubkey, NOW);
    let phone = members
        .iter()
        .find(|(fp, _)| *fp == &viewer_phone.fp)
        .map(|(_, m)| m)
        .expect("the enrolled device is a member");
    assert_eq!(
        phone.role,
        Role::Viewer,
        "a viewer's device must inherit exactly Viewer — never a self-elevation to Editor/Owner"
    );
}

/// A device nobody vouched for is not a member. The negative control for the
/// whole file: if this ever passed, every assertion above would be meaningless
/// because membership would not be gated at all.
#[test]
fn an_unvouched_device_is_not_a_member() {
    let owner = principal(1);
    let laptop = principal(2);
    let stranger = principal(8);

    let g = genesis(&owner);
    let a = admit(&owner, &laptop, Role::Editor, &g);

    let fps = member_fps(&[g, a], &owner);
    assert!(
        !fps.contains(&stranger.fp),
        "an unvouched key must never be a member"
    );
}
