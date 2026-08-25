//! Device secret-storage and revocation spike — the second half of the
//! multi-device identity question, gating ADR-098.
//!
//! The first spike (`multi_device_identity.rs`) established that ADR-040's
//! `Rebind` is succession, not concurrency. Two questions it explicitly left
//! open are the ones that actually decide whether a multi-device design is safe:
//!
//! 1. **Revocation.** Enrolment without revocation is not a feature, it is a
//!    liability — a lost laptop must stop being able to act. Is device
//!    revocation expressible today?
//! 2. **Secret storage.** How does the material a new device needs reach it,
//!    without an owner round-trip? Matrix answers this with SSSS: secrets held
//!    server-side as ciphertext, unlocked by a user-held recovery key.
//!
//! Before proposing new ops, this file tests a cheaper hypothesis that the first
//! spike did not consider: MAE already has **delegated invite** (`can_invite`)
//! and an inviter-removal cascade. Maybe a member can simply admit their own
//! devices and remove them again, and no new op set is needed at all.
//!
//! The answer turns out to be "half" — and the half that is missing is the
//! important one.
//!
//! As with the first spike, several tests assert *limitations*. They are written
//! to fail loudly if a future change moves the wall, rather than silently
//! encoding a workaround for a wall that has moved.

use ed25519_dalek::{Signer, SigningKey};

use mae_sync::content_crypto::{decrypt, encrypt, ContentKey};
use mae_sync::kb::Role;
use mae_sync::membership::{
    derive_valid_members, derive_valid_members_with, find_wrapped_content_key, fingerprint_of,
    InviterRemovalPolicy, MembershipAction, MembershipOp, MembershipView, SignedMembershipOp,
};

const KB: &str = "kb-device-revocation-spike";
const NOW: u64 = 10_000;

struct Principal {
    signing: SigningKey,
    pubkey: [u8; 32],
    fp: String,
}

/// Distinct seed per identity — never one shared "unicorn" key, and seeded
/// rather than random so a failure reproduces exactly.
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
        content_hash: None,
    }
}

fn genesis(owner: &Principal) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Admit, &owner.fp, &owner.fp);
    op.role = Some(Role::Owner);
    sign(op, owner)
}

/// An `Admit` authored by `author` (owner, or any member holding `can_invite`).
fn admit_by(
    author: &Principal,
    subject: &Principal,
    role: Role,
    can_invite: bool,
    prev: &SignedMembershipOp,
) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Admit, &subject.fp, &author.fp);
    op.role = Some(role);
    op.can_invite = can_invite;
    op.prev_hash = prev.chain_hash();
    sign(op, author)
}

fn remove_by(
    author: &Principal,
    subject: &Principal,
    prev: &SignedMembershipOp,
) -> SignedMembershipOp {
    let mut op = blank(MembershipAction::Remove, &subject.fp, &author.fp);
    op.prev_hash = prev.chain_hash();
    sign(op, author)
}

fn members(ops: &[SignedMembershipOp], owner: &Principal) -> Vec<String> {
    let mut v: Vec<String> = derive_valid_members(ops, &owner.pubkey, NOW)
        .into_keys()
        .collect();
    v.sort();
    v
}

fn members_with(
    ops: &[SignedMembershipOp],
    owner: &Principal,
    cascade: InviterRemovalPolicy,
) -> Vec<String> {
    let view = MembershipView {
        cascade,
        blocklist: Default::default(),
    };
    let mut v: Vec<String> = derive_valid_members_with(ops, &owner.pubkey, NOW, &view)
        .into_keys()
        .collect();
    v.sort();
    v
}

// ===========================================================================
// Part 1 — can delegated invite express a device set?
// ===========================================================================

/// **Yes, for membership.** A member holding `can_invite` can admit their own
/// second device, and both remain members simultaneously — the concurrency
/// `Rebind` could not express.
///
/// This is a genuinely cheaper path than a new op kind, and ADR-098 should
/// consider it before inventing one.
#[test]
fn a_member_with_can_invite_can_enrol_their_own_second_device() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let a = admit_by(&owner, &laptop, Role::Editor, true, &g);
    let d = admit_by(&laptop, &phone, Role::Editor, false, &a);

    let m = members(&[g, a, d], &owner);
    assert!(m.contains(&laptop.fp), "the laptop stays a member");
    assert!(
        m.contains(&phone.fp),
        "FINDING: delegated invite DOES express a concurrent device set — unlike Rebind, \
         enrolling the phone does not retire the laptop"
    );
    assert_eq!(m.len(), 3, "owner + two live devices: {m:?}");
}

/// The escalation guard holds: `author.role.includes(op.role)` means a device
/// can never be enrolled above the enrolling identity's own role.
#[test]
fn an_enrolled_device_cannot_exceed_the_enrolling_identity_role() {
    let owner = principal(1);
    let viewer = principal(4);
    let viewer_phone = principal(5);

    let g = genesis(&owner);
    let a = admit_by(&owner, &viewer, Role::Viewer, true, &g);
    // A viewer tries to enrol a device as an Editor.
    let d = admit_by(&viewer, &viewer_phone, Role::Editor, false, &a);

    let m = members(&[g, a, d], &owner);
    assert!(
        !m.contains(&viewer_phone.fp),
        "a viewer must not be able to enrol an Editor device — privilege escalation"
    );
}

/// **The decisive negative.** A member holds the content key, so it is tempting
/// to assume they can hand it to their own device. They cannot:
/// `find_wrapped_content_key` honours a wrap only when
/// `owners.contains(&o.author)` — the owner-principal chain — so a member's own
/// wrap is simply ignored.
///
/// So even with delegated invite solving membership, an E2E KB still needs an
/// **owner action per device**. This is the same wall the first spike found via
/// `Rebind`, reached by a completely different route, which is what makes it a
/// property of the design rather than of one op kind.
#[test]
fn a_member_cannot_deliver_the_content_key_to_their_own_device() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);

    let mut admit_laptop = blank(MembershipAction::Admit, &laptop.fp, &owner.fp);
    admit_laptop.role = Some(Role::Editor);
    admit_laptop.can_invite = true;
    admit_laptop.prev_hash = g.chain_hash();
    admit_laptop.wrapped_key = Some(b"wrapped-to-the-laptop".to_vec());
    let a = sign(admit_laptop, &owner);

    // The laptop enrols the phone AND attempts to deliver the content key to it.
    let mut admit_phone = blank(MembershipAction::Admit, &phone.fp, &laptop.fp);
    admit_phone.role = Some(Role::Editor);
    admit_phone.prev_hash = a.chain_hash();
    admit_phone.wrapped_key = Some(b"wrapped-to-the-phone-by-the-member".to_vec());
    let d = sign(admit_phone, &laptop);

    let ops = vec![g, a, d];

    assert!(
        members(&ops, &owner).contains(&phone.fp),
        "precondition: the phone IS a member — only the key delivery is in question"
    );
    assert!(
        find_wrapped_content_key(&ops, &owner.pubkey, &phone.fp).is_none(),
        "FINDING: a member's own content-key wrap is NOT honoured — only owner-chain wraps \
         count, so an E2E KB requires an owner action per device even when delegated invite \
         has already solved membership"
    );
}

// ===========================================================================
// Part 2 — revocation
// ===========================================================================

/// **The revocation gap.** `Remove` requires `author.role == Role::Owner`, so a
/// member cannot revoke their own lost device. The person who knows the laptop
/// was stolen is precisely the person who cannot act on it.
#[test]
fn a_member_cannot_revoke_their_own_device() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let a = admit_by(&owner, &laptop, Role::Editor, true, &g);
    let d = admit_by(&laptop, &phone, Role::Editor, false, &a);
    // The laptop tries to revoke the device it itself enrolled.
    let r = remove_by(&laptop, &phone, &d);

    let m = members(&[g, a, d, r], &owner);
    assert!(
        m.contains(&phone.fp),
        "FINDING: the member's Remove is inert — revocation is owner-only, so a user cannot \
         revoke their own compromised device without reaching the KB owner"
    );
}

/// The owner *can* revoke a single device, leaving the member's other devices
/// intact. The complement to the test above, proving the gap is about *who may
/// act*, not about whether per-device revocation is representable at all.
#[test]
fn the_owner_can_revoke_one_device_without_disturbing_the_others() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let a = admit_by(&owner, &laptop, Role::Editor, true, &g);
    let d = admit_by(&laptop, &phone, Role::Editor, false, &a);
    let r = remove_by(&owner, &phone, &d);

    let m = members(&[g, a, d, r], &owner);
    assert!(
        !m.contains(&phone.fp),
        "the owner's revocation takes effect"
    );
    assert!(
        m.contains(&laptop.fp),
        "the member's other device is unaffected — per-device revocation IS representable"
    );
}

/// The cascade is the only bulk-revocation primitive, and it is **opt-in per
/// peer**: under the default `PendingOnly`, removing a member leaves every
/// device they enrolled still a member.
///
/// That default is a real hazard for a device-set design — "remove this user"
/// would leave their laptop and phone with access — and it is a *local* peer
/// setting, so two peers can legitimately disagree about whether a revoked
/// user's devices are still members.
#[test]
fn removing_a_member_leaves_their_devices_behind_unless_cascade_is_opted_into() {
    let owner = principal(1);
    let laptop = principal(2);
    let phone = principal(3);

    let g = genesis(&owner);
    let a = admit_by(&owner, &laptop, Role::Editor, true, &g);
    let d = admit_by(&laptop, &phone, Role::Editor, false, &a);
    let r = remove_by(&owner, &laptop, &d);
    let ops = vec![g, a, d, r];

    let default_view = members_with(&ops, &owner, InviterRemovalPolicy::PendingOnly);
    assert!(
        default_view.contains(&phone.fp),
        "FINDING: under the DEFAULT policy, removing the member leaves their enrolled device \
         a member — 'remove this user' does not remove their devices"
    );

    let cascaded = members_with(&ops, &owner, InviterRemovalPolicy::CascadeAll);
    assert!(
        !cascaded.contains(&phone.fp),
        "CascadeAll does remove the whole device subtree — the bulk primitive exists, it is \
         simply not the default"
    );
    assert!(
        !cascaded.contains(&laptop.fp),
        "and the removed member is gone in both views"
    );
}

// ===========================================================================
// Part 3 — an SSSS analogue, buildable from primitives MAE already ships
// ===========================================================================
//
// Matrix stores private cross-signing material server-side as ciphertext,
// unlocked by a user-held recovery key. MAE needs the same shape so a new device
// can obtain member secrets without an owner round-trip.
//
// These tests check that it can be built from `content_crypto` as it already
// exists — XChaCha20-Poly1305 AEAD plus a sha2-based derivation — with **no new
// dependency**. That matters: MAE deliberately derives with sha2 rather than
// pulling an `hkdf` crate, to avoid a second `digest` major (see
// shared/sync/Cargo.toml's comment on the crypto coherence constraint).

/// Derive a storage key from a user-held recovery secret, domain-separated so it
/// can never collide with a content key derived elsewhere. Stands in for what a
/// real design would specify properly (Matrix uses base58 + HKDF); the point
/// here is only that the primitive is reachable today.
fn storage_key_from_recovery(recovery: &[u8]) -> ContentKey {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"mae/device-secret-storage/v0");
    h.update(recovery);
    let out: [u8; 32] = h.finalize().into();
    ContentKey::from_bytes(out)
}

/// The member secret a new device needs in order to act — stood in for by a
/// canary so leakage is detectable byte-wise.
const MEMBER_SECRET: &[u8] = b"MEMBER-SIGNING-SECRET-CANARY-0123456789";

#[test]
fn a_recovery_key_sealed_member_secret_round_trips() {
    let key = storage_key_from_recovery(b"correct horse battery staple");
    let blob = encrypt(&key, MEMBER_SECRET);
    let opened = decrypt(&key, &blob).expect("the correct recovery key opens the blob");
    assert_eq!(opened, MEMBER_SECRET);
}

/// The attacker's test: a wrong recovery key opens nothing. Several distinct
/// wrong keys, including one differing in a single byte, rather than one
/// conveniently-chosen wrong value.
#[test]
fn a_wrong_recovery_key_opens_nothing() {
    let right = storage_key_from_recovery(b"correct horse battery staple");
    let blob = encrypt(&right, MEMBER_SECRET);

    for wrong in [
        b"correct horse battery stapl".as_slice(),
        b"correct horse battery staple ".as_slice(),
        b"Correct horse battery staple".as_slice(),
        b"".as_slice(),
    ] {
        let k = storage_key_from_recovery(wrong);
        assert!(
            decrypt(&k, &blob).is_err(),
            "a wrong recovery key must open nothing (tried {wrong:?})"
        );
    }
}

/// Exhaustive per-byte tamper sweep, mirroring
/// `content_crypto::aead_rejects_wrong_key_and_every_tampered_byte`. A blob a
/// key-blind server holds must be non-malleable at every offset — nonce, tag and
/// ciphertext alike.
#[test]
fn every_tampered_byte_of_the_sealed_blob_is_rejected() {
    let key = storage_key_from_recovery(b"correct horse battery staple");
    let blob = encrypt(&key, MEMBER_SECRET);

    for i in 0..blob.len() {
        let mut bad = blob.clone();
        bad[i] ^= 0x01;
        assert!(
            decrypt(&key, &bad).is_err(),
            "flipping byte {i} of {} must be rejected, not silently accepted",
            blob.len()
        );
    }
}

/// A key-blind host must learn nothing. Asserted at the byte level against the
/// canary rather than by inspecting a length or a type — the same oracle
/// `scripts/collab-encrypted-e2e.sh` uses when it scans the daemon's store and
/// WAL for a plaintext canary.
#[test]
fn the_sealed_blob_leaks_no_plaintext_to_a_key_blind_host() {
    let key = storage_key_from_recovery(b"correct horse battery staple");
    let blob = encrypt(&key, MEMBER_SECRET);

    assert!(
        !blob
            .windows(MEMBER_SECRET.len())
            .any(|w| w == MEMBER_SECRET),
        "the sealed blob must not contain the member secret in the clear"
    );
    // A non-trivial prefix must not appear either — a whole-value scan alone
    // would miss a partially-encrypted blob.
    let prefix = &MEMBER_SECRET[..16];
    assert!(
        !blob.windows(prefix.len()).any(|w| w == prefix),
        "no recognisable prefix of the secret may appear in the blob"
    );
}

/// Negative control for Part 3: the leak oracle above must be capable of
/// firing. If a plaintext blob were stored, the scan has to catch it — otherwise
/// every confidentiality assertion here is vacuous.
#[test]
fn the_leak_oracle_catches_an_unencrypted_blob() {
    let plaintext_blob = MEMBER_SECRET.to_vec();
    assert!(
        plaintext_blob
            .windows(MEMBER_SECRET.len())
            .any(|w| w == MEMBER_SECRET),
        "the scan used by the confidentiality test must detect a plaintext blob — if this \
         fails, that test proves nothing"
    );
}
