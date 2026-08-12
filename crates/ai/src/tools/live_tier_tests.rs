//! ADR-084 D7 — the live permission tier, and the three invariants it must not break.
//!
//! Split out of `decision_tests.rs`, whose module doc frames three questions
//! about the *decision point* itself. These answer a different one: does a
//! change to the shared `LiveTier` cell reach a `PermissionPolicy` that was
//! already cloned into a spawned session? That is the whole claim of the live
//! feature — if only the main thread observes it, the change is worthless.
//!
//! Principle #14: these matter more than the feature. A live auto-approval line
//! is a security control that can move underneath an in-flight decision, so
//! each test below pins one way that must NOT be possible.

use super::*;
use crate::types::PermissionTier;
use std::collections::HashSet;

/// The feature itself: a change to the shared cell is observed by a policy that
/// was cloned BEFORE the change — which is the whole reason ADR-090 deferred
/// this. Oracle is a dispatch decision, not the field.
#[test]
fn a_live_tier_change_is_observed_by_an_already_cloned_policy() {
    let cell = mae_core::LiveTier::new(PermissionTier::ReadOnly);
    let policy = PermissionPolicy {
        live: Some(cell.clone()),
        ..PermissionPolicy::default()
    };
    // Clone first, exactly as `with_permission_policy` does when handing the
    // policy to the spawned session, then change the tier afterwards.
    let already_handed_over = policy.clone();
    assert_eq!(
        already_handed_over.decide_tier(PermissionTier::Shell),
        Decision::Ask
    );

    cell.set(PermissionTier::Shell);

    assert_eq!(
        already_handed_over.decide_tier(PermissionTier::Shell),
        Decision::Allow,
        "a policy cloned before the change did not observe it — the Arc is not shared"
    );
}

/// Invariant 1: a live raise cannot climb past a session-declared hard ceiling.
/// This is the regression that would turn a config change into a security
/// change, so it is asserted at the highest tier the cell can hold.
#[test]
fn a_live_tier_raise_cannot_exceed_a_hard_ceiling() {
    let cell = mae_core::LiveTier::new(PermissionTier::ReadOnly);
    let policy = PermissionPolicy {
        live: Some(cell.clone()),
        ..PermissionPolicy::default()
    }
    .with_hard_ceiling(HardCeiling {
        tier: PermissionTier::Write,
        source: HardCeilingSource::SessionDeclared,
    });

    cell.set(PermissionTier::Privileged);

    assert!(
        matches!(policy.decide_tier(PermissionTier::Shell), Decision::Deny(_)),
        "a live raise escaped the session's hard ceiling"
    );
    assert_eq!(
        policy.ambient_scheme_tier(),
        PermissionTier::Write,
        "the ambient Scheme tier must stay clamped to the ceiling, not follow the cell"
    );
}

/// Invariant 2: a one-time approval is a decision a human already made about one
/// specific call. It must not keep following the cell — in either direction.
#[test]
fn a_live_tier_change_does_not_alter_an_in_flight_one_time_approval() {
    let cell = mae_core::LiveTier::new(PermissionTier::ReadOnly);
    let policy = PermissionPolicy {
        live: Some(cell.clone()),
        ..PermissionPolicy::default()
    };
    let approved = policy.with_one_time_approval(PermissionTier::Shell);
    assert_eq!(approved.decide_tier(PermissionTier::Shell), Decision::Allow);

    // The user tightens the tier while that approved call is still in flight.
    cell.set(PermissionTier::ReadOnly);
    assert_eq!(
        approved.decide_tier(PermissionTier::Shell),
        Decision::Allow,
        "the approval evaporated when the live tier moved"
    );

    // And the reverse: raising the cell must not widen the approval either.
    cell.set(PermissionTier::Privileged);
    assert_eq!(
        approved.decide_tier(PermissionTier::Privileged),
        Decision::Ask,
        "a live raise silently widened an approval granted for a lower tier"
    );
}

/// Invariant 3: the category allowlist is orthogonal and checked first. No tier
/// value, live or fixed, may reach a tool outside it.
#[test]
fn a_live_tier_raise_cannot_bypass_the_category_allowlist() {
    let cell = mae_core::LiveTier::new(PermissionTier::ReadOnly);
    let mut allowed = HashSet::new();
    allowed.insert(ToolCategory::Knowledge);
    let policy = PermissionPolicy {
        live: Some(cell.clone()),
        allowed_categories: Some(allowed),
        ..PermissionPolicy::default()
    };

    cell.set(PermissionTier::Privileged);

    assert!(
        matches!(
            policy.decide("shell_exec", PermissionTier::ReadOnly),
            Decision::Deny(DenyReason::Category)
        ),
        "a live raise reached a tool outside the category allowlist"
    );
}
