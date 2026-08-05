//! Forward-secrecy spike — the third and final open question from the
//! multi-device arc, gating ADR-098.
//!
//! The previous two spikes established that a device set is expressible but that
//! content-key delivery and revocation are both owner-rooted. Both reports ended
//! on the same unresolved point, and it is the one that decides whether
//! self-service device management is viable at all:
//!
//! > A revoked device **already held** the content key. Revoking it does not
//! > un-share what it has. So a real revocation must also rotate the content
//! > key — an owner action. Can device revocation avoid dragging the owner back
//! > in?
//!
//! This file answers that at the op-set layer, where the encryption actually
//! happens, rather than at the membership layer where the previous two spikes
//! worked. It measures three things:
//!
//! 1. what a revoked device can still read after a rotation (the forward-secrecy
//!    boundary — how far forward does it actually reach?);
//! 2. what a *legitimate* member loses when a rotation happens (the cost side,
//!    which turns out to be the decisive finding);
//! 3. whether the whole picture is consistent with rotating on every device
//!    revocation.
//!
//! As with the earlier spikes, several tests assert current limitations and are
//! written to fail loudly if the behaviour changes, rather than silently
//! encoding a workaround.

use std::collections::BTreeSet;

use mae_sync::content_crypto::ContentKey;
use mae_sync::kb::KbNodeDoc;
use mae_sync::op_set::{merge, open_new_ops, seal_op};

/// Distinct, non-"unicorn" keys — each a different constant so a mix-up cannot
/// pass by coincidence.
fn key(b: u8) -> ContentKey {
    ContentKey::from_bytes([b; 32])
}

fn nothing_seen() -> BTreeSet<String> {
    BTreeSet::new()
}

/// A plaintext node update, as the editor would produce: a real `KbNodeDoc`
/// delta, not an opaque blob, so what is sealed is what actually syncs.
fn node_update(id: &str, title: &str, body: &str) -> Vec<u8> {
    KbNodeDoc::new_with_client_id(id, title, body, &[], 0x00AB_CDEF_0123).encode()
}

/// Build an op-set containing `pre` ops sealed under `k_old`, then `post` ops
/// sealed under `k_new` — the shape a KB has after a mid-life content-key
/// rotation.
fn op_set_spanning_a_rotation(
    k_old: &ContentKey,
    k_new: &ContentKey,
    pre: usize,
    post: usize,
) -> Vec<u8> {
    let mut state: Vec<u8> = Vec::new();
    for i in 0..pre {
        let upd = node_update(
            &format!("pre-{i}"),
            &format!("Pre-rotation note {i}"),
            "Content the revoked device was legitimately able to read.",
        );
        let (_, delta) = seal_op(&state, k_old, &upd, 100 + i as u64).expect("seal pre");
        state = merge(&state, &delta).expect("merge pre");
    }
    for i in 0..post {
        let upd = node_update(
            &format!("post-{i}"),
            &format!("Post-rotation note {i}"),
            "Content authored after the device was revoked.",
        );
        let (_, delta) = seal_op(&state, k_new, &upd, 200 + i as u64).expect("seal post");
        state = merge(&state, &delta).expect("merge post");
    }
    state
}

// ===========================================================================
// 1 — What forward secrecy actually buys
// ===========================================================================

/// The property that **does** hold: after the owner rotates, the revoked device
/// cannot open anything authored under the new key.
///
/// This is real and worth having — it is the entire security value of rotating
/// on removal.
#[test]
fn a_revoked_device_cannot_open_post_rotation_content() {
    let k_old = key(0x11);
    let k_new = key(0x22);
    let state = op_set_spanning_a_rotation(&k_old, &k_new, 3, 4);

    // The revoked device still holds only the old key.
    let opened = open_new_ops(&state, &k_old, &nothing_seen());

    assert_eq!(
        opened.ops.len(),
        3,
        "the revoked device opens exactly the pre-rotation ops"
    );
    assert_eq!(
        opened.undecryptable, 4,
        "and every post-rotation op is undecryptable to it — forward secrecy holds"
    );
}

/// The limit of that property, stated plainly: revocation is **forward only**.
/// The revoked device keeps everything sealed under the old key, permanently,
/// with no owner action able to take it back.
///
/// `find_wrapped_content_key`'s own doc calls this out — a removed member
/// "keeps their OLD wrapped blob … and so can read history they already had —
/// the intended ADR-037 §D3 semantics, NOT a leak". This test pins it as a
/// property ADR-098 must design around rather than discover.
#[test]
fn a_revoked_device_retains_every_pre_rotation_op_permanently() {
    let k_old = key(0x11);
    let k_new = key(0x22);
    let state = op_set_spanning_a_rotation(&k_old, &k_new, 5, 2);

    let opened = open_new_ops(&state, &k_old, &nothing_seen());
    assert_eq!(
        opened.ops.len(),
        5,
        "FINDING: revocation cannot un-share history — a lost laptop keeps read access to \
         everything authored before it was revoked, forever"
    );
}

// ===========================================================================
// 2 — The cost, and it is the decisive finding
// ===========================================================================

/// **The killer.** A *legitimate, continuously-authorised* member who re-syncs
/// from scratch after a rotation — reinstall, new device, offline catch-up —
/// holds only the current key, and therefore **loses every pre-rotation op**.
///
/// `open_new_ops` decrypts with a single `ContentKey` and skips what does not
/// open. `find_wrapped_content_key` returns only the latest wrap. So a rotation
/// does not merely inconvenience the revoked device; it destroys history
/// availability for everyone who re-syncs afterwards.
///
/// This is issue #176 (open) and the `FIXME(#237)` on `find_wrapped_content_key`.
/// It was filed as a rotation-lifecycle gap. Its significance for ADR-098 is far
/// larger: **it makes "rotate on every device revocation" unworkable**, because
/// each revocation would permanently truncate the readable history of every
/// member who later re-syncs.
#[test]
fn a_legitimate_member_resyncing_after_rotation_loses_all_pre_rotation_content() {
    let k_old = key(0x11);
    let k_new = key(0x22);
    let state = op_set_spanning_a_rotation(&k_old, &k_new, 6, 3);

    // A member who was present throughout, but whose local state was lost, now
    // derives only the CURRENT key and replays the full op-set.
    let opened = open_new_ops(&state, &k_new, &nothing_seen());

    assert_eq!(
        opened.ops.len(),
        3,
        "the re-syncing member opens only post-rotation ops"
    );
    assert_eq!(
        opened.undecryptable, 6,
        "FINDING (#176/#237): six legitimately-authored ops are permanently unreadable to a \
         member who never lost authorisation — rotation truncates history for the innocent, \
         not only for the revoked"
    );
}

/// Quantifies how that compounds, which is what makes it disqualifying for
/// per-device revocation rather than merely a bug.
///
/// Each rotation strands everything sealed before it. With one rotation per
/// device revocation, a KB's readable history for a re-syncing member shrinks to
/// "whatever was written since the most recent device was revoked" — and in an
/// organisation with many users and several devices each, that window is short.
#[test]
fn every_additional_rotation_strands_strictly_more_history() {
    let keys = [key(0x31), key(0x32), key(0x33), key(0x34)];
    let mut state: Vec<u8> = Vec::new();
    let mut i = 0u64;

    // Four epochs of two ops each, one rotation between consecutive epochs —
    // i.e. three device revocations over the KB's life.
    for k in &keys {
        for _ in 0..2 {
            let upd = node_update(&format!("n-{i}"), &format!("Note {i}"), "body");
            let (_, delta) = seal_op(&state, k, &upd, 300 + i).expect("seal");
            state = merge(&state, &delta).expect("merge");
            i += 1;
        }
    }

    // A member re-syncing after the last rotation holds only the newest key.
    let latest = open_new_ops(&state, &keys[3], &nothing_seen());
    assert_eq!(latest.ops.len(), 2, "only the newest epoch is readable");
    assert_eq!(
        latest.undecryptable, 6,
        "FINDING: three rotations strand six of eight ops — the loss is cumulative, so \
         rotating per device revocation degrades the KB monotonically"
    );

    // Holding an older key reads that epoch but nothing after it — confirming the
    // stranding is per-epoch and not an artefact of key ordering.
    let mid = open_new_ops(&state, &keys[1], &nothing_seen());
    assert_eq!(mid.ops.len(), 2, "an older key opens exactly its own epoch");
    assert_eq!(mid.undecryptable, 6, "and nothing from any other epoch");
}

/// The fix #176 recommends — retain every key ever wrapped to you and try each
/// per blob — would resolve the cost. Demonstrated here so ADR-098 can rely on
/// it being genuinely available rather than hypothetical: the op-set itself
/// needs no change, only the caller's key handling.
///
/// Note what this does **not** do: trying every key restores history for a
/// legitimate member without giving the revoked device anything, because the
/// revoked device is never wrapped the new key in the first place. The two
/// properties are independent, which is exactly why the current single-key
/// behaviour is a defect rather than a deliberate trade-off.
#[test]
fn trying_every_retained_key_restores_full_history_without_helping_the_revoked_device() {
    let k_old = key(0x11);
    let k_new = key(0x22);
    let state = op_set_spanning_a_rotation(&k_old, &k_new, 6, 3);

    // A legitimate member retaining BOTH keys.
    let mut total = 0usize;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for k in [&k_new, &k_old] {
        let opened = open_new_ops(&state, k, &seen);
        total += opened.ops.len();
        for (id, _) in &opened.ops {
            seen.insert(id.clone());
        }
    }
    assert_eq!(
        total, 9,
        "retaining both keys recovers the complete history — the op-set already supports this, \
         only the single-key caller does not"
    );

    // The revoked device retains only the old key, so the same strategy gains it
    // nothing beyond what it already had.
    let revoked = open_new_ops(&state, &k_old, &nothing_seen());
    assert_eq!(
        revoked.ops.len(),
        6,
        "the revoked device still opens only pre-rotation ops — restoring history for \
         legitimate members does not weaken forward secrecy"
    );
}

// ===========================================================================
// 3 — Negative controls
// ===========================================================================

/// A device that never held any key reads nothing at all — the baseline that
/// makes every count above meaningful. Without it, a broken `open_new_ops`
/// returning empty for everything would satisfy the forward-secrecy assertions.
#[test]
fn a_device_with_an_unrelated_key_opens_nothing() {
    let k_old = key(0x11);
    let k_new = key(0x22);
    let state = op_set_spanning_a_rotation(&k_old, &k_new, 4, 4);

    let stranger = open_new_ops(&state, &key(0x99), &nothing_seen());
    assert!(
        stranger.ops.is_empty(),
        "a key that sealed nothing must open nothing"
    );
    assert_eq!(
        stranger.undecryptable, 8,
        "and every op is reported undecryptable rather than silently absent"
    );
}

/// The oracle has teeth: without a rotation, a single key opens everything. If
/// this failed, the stranding measured above could be an artefact of the harness
/// rather than of rotation.
#[test]
fn without_a_rotation_one_key_opens_the_entire_op_set() {
    let k = key(0x11);
    let state = op_set_spanning_a_rotation(&k, &k, 5, 4);

    let opened = open_new_ops(&state, &k, &nothing_seen());
    assert_eq!(opened.ops.len(), 9, "no rotation ⇒ no stranding");
    assert_eq!(opened.undecryptable, 0);
}
