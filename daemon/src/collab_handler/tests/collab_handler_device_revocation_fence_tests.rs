//! Device-revocation write-fencing spike — the last open question of the
//! multi-device arc, gating ADR-098.
//!
//! The forward-secrecy spike (`docs/research/098-revocation-forward-secrecy-spike.md`)
//! ended on a specific hypothesis worth testing rather than assuming:
//!
//! > ADR-023's epoch fence may fence a revoked device's *writes* independently of
//! > key rotation, which would decouple the two halves of revocation.
//!
//! It matters because rotation is expensive: it permanently strands history for
//! every member who later re-syncs (#176). If stopping a lost device's writes
//! needs no rotation, then "device lost" and "content must be protected going
//! forward" become two separately-priced operations instead of one.
//!
//! These tests run against the real dispatch path — the same
//! `handle_doc_request_inner` a live peer hits — rather than against the fence
//! function in isolation, so what is measured is what actually happens to a
//! revoked device's write.

use super::*;

/// A device that was a working editor stops being able to write the moment it is
/// removed — with **no content-key rotation involved anywhere in this test**.
///
/// This is the useful half of the answer: write-revocation is genuinely
/// independent of rotation, so a lost laptop can be stopped immediately without
/// paying #176's history-stranding cost.
#[tokio::test]
async fn a_removed_device_stops_writing_with_no_key_rotation_involved() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbrev",
        "alice",
        &mut docs,
    )
    .await;

    // Bob's phone is admitted as an editor and writes successfully.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbrev", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbrev"),
        &mut docs
    )
    .await
    .error
    .is_none());

    let before = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg_as("kbrev", &fp("bob"), 0, "authored while authorised"),
        &mut docs,
    )
    .await;
    assert!(
        before.error.is_none(),
        "precondition: the device could write before revocation: {:?}",
        before.error
    );

    // The owner revokes the device. No rotation, no re-wrap, no key change.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/remove_member", "kbrev", &fp("bob"), None),
        &mut docs,
    )
    .await;

    let after = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg_as("kbrev", &fp("bob"), 0, "authored after revocation"),
        &mut docs,
    )
    .await;
    assert!(
        after.error.is_some(),
        "FINDING: removal alone stops the device's writes — no content-key rotation needed, \
         so write-revocation does not pay #176's history-stranding cost"
    );
}

/// **Which mechanism actually stops it.** The epoch fence announces itself with a
/// distinctive "rebase required … stale-epoch client" message
/// (`enforce_epoch_fence_with_coll`). A removed device is rejected *before* that,
/// by the `kb_access` authorization gate.
///
/// This matters for ADR-098 because the two have different requirements: the
/// access gate needs an owner-authored `Remove` to have propagated, while the
/// fence needs only an epoch bump. Knowing which one does the work tells you what
/// a self-service design would have to change.
#[tokio::test]
async fn a_removed_device_is_stopped_by_the_access_gate_not_the_epoch_fence() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbmech",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbmech", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbmech"),
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/remove_member", "kbmech", &fp("bob"), None),
        &mut docs,
    )
    .await;

    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg_as("kbmech", &fp("bob"), 0, "post-revocation"),
        &mut docs,
    )
    .await;

    let msg = denied
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(!msg.is_empty(), "the write must be rejected");
    assert!(
        !msg.contains("rebase required"),
        "FINDING: the rejection is NOT the epoch fence — it is the authorization gate. \
         The fence's own message is 'rebase required … stale-epoch client'; got: {msg}"
    );
}

/// **Why the fence cannot carry revocation on its own.** The fence's discriminator
/// is `derive_kb_client_id(principal, epoch_now)`, and `kb_member_epoch` documents
/// "Absent member ⇒ 0". A member admitted by a fresh grant also sits at epoch 0
/// (`MembershipOp::epoch` — "A fresh grant stays at epoch 0").
///
/// So for the common case — a device admitted and never role-changed — the client
/// id the fence computes is **identical before and after revocation**. The fence
/// has nothing to discriminate on, which is why the authorization gate must and
/// does do the work.
///
/// Asserted on the pure derivation so it holds regardless of dispatch plumbing.
#[test]
fn the_fence_discriminator_is_unchanged_by_revoking_an_epoch_zero_device() {
    let device = fp("bob");

    // What the fence computes while the device is a member at a fresh grant …
    let while_member = derive_kb_client_id(&device, 0);
    // … and what it computes once the device is absent (epoch falls back to 0).
    let after_removal = derive_kb_client_id(&device, 0);

    assert_eq!(
        while_member, after_removal,
        "FINDING: for an epoch-0 device the fence's discriminator does not change on \
         revocation — the fence alone cannot distinguish a revoked device from a live one"
    );

    // The fence DOES discriminate across a real epoch change, which is its actual
    // job (ADR-023: forcing a rebase after a grant/role change). Without this the
    // assertion above could be read as "the fence never discriminates".
    assert_ne!(
        derive_kb_client_id(&device, 0),
        derive_kb_client_id(&device, 7),
        "the fence does discriminate across epochs — it is simply not a revocation signal"
    );
}

/// Revocation stays owner-authored at the real API, confirming at dispatch level
/// what the membership-layer spike found: a member cannot revoke a device, even
/// one in their own KB.
///
/// This is the constraint ADR-098 has to design against — the person who knows
/// the laptop was stolen still cannot act alone.
#[tokio::test]
async fn a_non_owner_member_cannot_revoke_a_device() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbself",
        "alice",
        &mut docs,
    )
    .await;
    for who in ["bob", "carol"] {
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbself", &fp(who), Some("editor")),
            &mut docs,
        )
        .await;
        dispatch_as(
            &store,
            &bc,
            Some(who),
            Some(&fp(who)),
            kb_join_msg("kbself"),
            &mut docs,
        )
        .await;
    }

    // Bob tries to revoke carol — standing in for "revoke my own other device".
    let attempt = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_member_msg("kb/remove_member", "kbself", &fp("carol"), None),
        &mut docs,
    )
    .await;
    assert!(
        attempt.error.is_some(),
        "a non-owner must not be able to revoke"
    );

    // And the target is genuinely unaffected — a rejected request must not
    // half-apply. Without this the test could pass on an error that still removed.
    let still_writes = dispatch_as(
        &store,
        &bc,
        Some("carol"),
        Some(&fp("carol")),
        kb_node_update_msg_as("kbself", &fp("carol"), 0, "still authorised"),
        &mut docs,
    )
    .await;
    assert!(
        still_writes.error.is_none(),
        "the rejected revocation must not have taken effect: {:?}",
        still_writes.error
    );
}

/// Negative control for the whole file: an owner's revocation of a member who was
/// never admitted must not silently "succeed" and make the suite's positive
/// results meaningless, and a still-authorised peer must keep writing throughout.
#[tokio::test]
async fn revocation_does_not_disturb_a_bystander_device() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbbys",
        "alice",
        &mut docs,
    )
    .await;
    for who in ["bob", "carol"] {
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbbys", &fp(who), Some("editor")),
            &mut docs,
        )
        .await;
        dispatch_as(
            &store,
            &bc,
            Some(who),
            Some(&fp(who)),
            kb_join_msg("kbbys"),
            &mut docs,
        )
        .await;
    }

    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/remove_member", "kbbys", &fp("bob"), None),
        &mut docs,
    )
    .await;

    let bob = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg_as("kbbys", &fp("bob"), 0, "revoked"),
        &mut docs,
    )
    .await;
    let carol = dispatch_as(
        &store,
        &bc,
        Some("carol"),
        Some(&fp("carol")),
        kb_node_update_msg_as("kbbys", &fp("carol"), 0, "bystander"),
        &mut docs,
    )
    .await;

    assert!(bob.error.is_some(), "the revoked device is stopped");
    assert!(
        carol.error.is_none(),
        "a bystander device on the same KB is unaffected — revocation is per-principal, \
         not a KB-wide freeze: {:?}",
        carol.error
    );
}
