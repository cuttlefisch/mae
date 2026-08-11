//! ADR-090 adversarial tests for the single decision point.
//!
//! Principle #14: these are written to falsify the model, not to congratulate
//! it. The three questions each test is trying to break:
//!
//! 1. Can something that merely exceeds `auto_approve_up_to` come back as
//!    `Deny`? (That is the regression that forces `auto_approve_tier = shell`.)
//! 2. Can something policy genuinely forbids come back as `Ask`? (That would
//!    make every hard gate promptable.)
//! 3. Can an approval promote a `Deny` into an `Allow`?

use super::*;
use crate::types::PermissionTier;
use std::collections::HashSet;

const ALL_TIERS: [PermissionTier; 4] = [
    PermissionTier::ReadOnly,
    PermissionTier::Write,
    PermissionTier::Shell,
    PermissionTier::Privileged,
];

fn ceiling(tier: PermissionTier) -> PermissionPolicy {
    PermissionPolicy {
        auto_approve_up_to: tier,
        ..PermissionPolicy::default()
    }
}

// ---------------------------------------------------------------------------
// 1. Above the auto-approval ceiling is ASK — never Deny, never Allow.
// ---------------------------------------------------------------------------

/// The whole ADR in one property, over the full 4x4 grid rather than a
/// hand-picked pair: for every auto-approval ceiling and every tier, the
/// answer is exactly `Allow` at-or-below and exactly `Ask` above. No `Deny`
/// appears anywhere in the grid, because an auto-approval ceiling is not a
/// prohibition.
#[test]
fn the_auto_approval_ceiling_produces_allow_below_and_ask_above_never_deny() {
    for ceil in ALL_TIERS {
        let policy = ceiling(ceil);
        for tier in ALL_TIERS {
            let got = policy.decide("some_tool", tier);
            let expected = if tier <= ceil {
                Decision::Allow
            } else {
                Decision::Ask
            };
            assert_eq!(
                got, expected,
                "ceiling={ceil:?} tier={tier:?}: expected {expected:?}, got {got:?}"
            );
            assert!(
                !got.is_deny(),
                "ceiling={ceil:?} tier={tier:?} must never be a denial"
            );
        }
    }
}

/// The concrete regression the ADR names: under the shipped default, the tools
/// the AI needs to build and test must reach `Ask`. If any of them came back
/// `Deny`, the predictable operator response is `auto_approve_tier = "shell"`,
/// which undoes the entire change.
///
/// Uses the real registry tiers via `effective_tier`, not a hand-asserted
/// constant, so a future retier of `run_build` is caught here too.
#[test]
fn run_build_and_run_test_reach_ask_under_the_shipped_default() {
    let policy = PermissionPolicy::default();
    for tool in ["run_build", "run_test", "shell_exec"] {
        // These are Shell-tier tools; `effective_tier` never lowers, so the
        // floor used here is the honest one.
        let tier = effective_tier(tool, &serde_json::json!({}), PermissionTier::Shell);
        let decision = policy.decide(tool, tier);
        assert_eq!(
            decision,
            Decision::Ask,
            "{tool} must be ASKABLE under the shipped default, not denied — a denial \
             here is what makes users pin auto_approve_tier = \"shell\""
        );
    }
}

/// ...and reads still just work, so the default is usable without a prompt
/// storm on ordinary navigation.
#[test]
fn reads_are_auto_approved_under_the_shipped_default() {
    let policy = PermissionPolicy::default();
    for tool in ["buffer_read", "kb_search", "project_search"] {
        assert_eq!(
            policy.decide(tool, PermissionTier::ReadOnly),
            Decision::Allow,
            "{tool} must not prompt"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. What policy forbids stays DENY — it must not be softened into Ask.
// ---------------------------------------------------------------------------

/// A session-declared ceiling (ADR-051) is binding at every tier above it, for
/// every declared value — not just the one convenient case. Asserting over the
/// full grid catches a "hard ceiling only bites at ReadOnly" bug that a single
/// hand-picked pair would miss.
#[test]
fn a_session_declared_ceiling_denies_and_is_never_softened_to_ask() {
    for declared in ALL_TIERS {
        let policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            hard_ceiling: Some(HardCeiling {
                tier: declared,
                source: HardCeilingSource::SessionDeclared,
            }),
            allowed_categories: None,
            live: None,
        }
        // The constructor is what production uses; go through it so the test
        // exercises the clamping rule too.
        .with_hard_ceiling(HardCeiling {
            tier: declared,
            source: HardCeilingSource::SessionDeclared,
        });

        for tier in ALL_TIERS {
            let got = policy.decide("some_tool", tier);
            if tier > declared {
                assert_eq!(
                    got,
                    Decision::Deny(DenyReason::HardCeiling(HardCeiling {
                        tier: declared,
                        source: HardCeilingSource::SessionDeclared,
                    })),
                    "declared={declared:?} tier={tier:?} must DENY, not ask"
                );
            } else {
                assert!(
                    !got.is_deny(),
                    "declared={declared:?} tier={tier:?} must not be denied"
                );
            }
        }
    }
}

/// An unparseable declaration (ADR-084 D4) is the same shape, and must stay a
/// denial: prompting past a typo is how a typo becomes an escalation.
#[test]
fn an_unparseable_declaration_denies_and_is_never_softened_to_ask() {
    let policy = PermissionPolicy::default().with_hard_ceiling(HardCeiling {
        tier: PermissionTier::ReadOnly,
        source: HardCeilingSource::UnparseableDeclaration,
    });
    for tier in [
        PermissionTier::Write,
        PermissionTier::Shell,
        PermissionTier::Privileged,
    ] {
        let got = policy.decide("some_tool", tier);
        assert!(got.is_deny(), "{tier:?} must be denied, got {got:?}");
        assert!(
            !got.is_ask(),
            "{tier:?} must not be promptable behind a failed parse"
        );
    }
    // The message must name the parse failure, not read like an ordinary
    // ceiling — an operator has to know to fix the value, not raise a tier.
    let msg = deny_message(
        "shell_exec",
        PermissionTier::Shell,
        DenyReason::HardCeiling(HardCeiling {
            tier: PermissionTier::ReadOnly,
            source: HardCeilingSource::UnparseableDeclaration,
        }),
    );
    assert!(msg.contains("could not be parsed"), "{msg}");
}

/// A category restriction (ADR-085/056) denies regardless of tier, including
/// at `ReadOnly` where the tier axis would have said `Allow` — the two axes
/// are independent and the category one is not promptable.
#[test]
fn a_category_restriction_denies_at_every_tier_including_ones_the_tier_axis_allows() {
    let mut only_knowledge = HashSet::new();
    only_knowledge.insert(ToolCategory::Knowledge);
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::Privileged,
        hard_ceiling: None,
        allowed_categories: Some(only_knowledge),
        live: None,
    };

    // Out-of-category, and uncategorized (fail-closed), at every tier.
    for tool in ["lsp_hover", "git_status", "execute_command", "shell_exec"] {
        for tier in ALL_TIERS {
            let got = policy.decide(tool, tier);
            assert_eq!(
                got,
                Decision::Deny(DenyReason::Category),
                "{tool} at {tier:?} must be a CATEGORY denial, got {got:?}"
            );
        }
    }
    // ...and an in-category tool is unaffected by the restriction.
    assert_eq!(
        policy.decide("kb_search", PermissionTier::ReadOnly),
        Decision::Allow
    );
}

/// Deny-first ordering: when a call trips BOTH a category restriction and the
/// auto-approval ceiling, the answer is the denial. If ordering were reversed
/// the answer would be `Ask`, and a prompt would become a route around a
/// category restriction.
#[test]
fn a_category_denial_wins_over_an_ask_when_both_apply() {
    let mut only_knowledge = HashSet::new();
    only_knowledge.insert(ToolCategory::Knowledge);
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::ReadOnly,
        hard_ceiling: None,
        allowed_categories: Some(only_knowledge),
        live: None,
    };
    assert_eq!(
        policy.decide("shell_exec", PermissionTier::Shell),
        Decision::Deny(DenyReason::Category),
        "a call that is both out-of-category AND above the ceiling must DENY, not ask"
    );
}

// ---------------------------------------------------------------------------
// 3. An approval may resolve an Ask. It may never promote a Deny.
// ---------------------------------------------------------------------------

/// The attacker's case for the approval path: a forged or over-broad
/// `approved_tier` (the value carried on `AiEvent::ToolCallRequest`) must not
/// cross a hard ceiling or a category restriction. Tries the maximum possible
/// approval — `Privileged` — against every hard ceiling.
#[test]
fn approval_can_never_promote_a_deny() {
    for declared in ALL_TIERS {
        let policy = PermissionPolicy::default().with_hard_ceiling(HardCeiling {
            tier: declared,
            source: HardCeilingSource::SessionDeclared,
        });
        let approved = policy.with_one_time_approval(PermissionTier::Privileged);
        for tier in ALL_TIERS {
            if tier > declared {
                assert!(
                    approved.decide("some_tool", tier).is_deny(),
                    "approval at Privileged crossed a {declared:?} hard ceiling for {tier:?}"
                );
            }
        }
        assert_eq!(
            approved.hard_ceiling.map(|hc| hc.tier),
            Some(declared),
            "approval must not clear the hard ceiling"
        );
    }

    // Same for the category axis.
    let mut only_knowledge = HashSet::new();
    only_knowledge.insert(ToolCategory::Knowledge);
    let restricted = PermissionPolicy {
        auto_approve_up_to: PermissionTier::ReadOnly,
        hard_ceiling: None,
        allowed_categories: Some(only_knowledge),
        live: None,
    };
    assert_eq!(
        restricted
            .with_one_time_approval(PermissionTier::Privileged)
            .decide("shell_exec", PermissionTier::Shell),
        Decision::Deny(DenyReason::Category),
        "approval must not clear a category restriction"
    );
}

/// An approval resolves the `Ask` it was granted for, and does not silently
/// widen the session: it raises the ceiling to exactly the approved tier, so
/// a `Write` approval does not also auto-approve `Shell`.
#[test]
fn an_approval_grants_exactly_the_tier_it_was_shown() {
    let policy = ceiling(PermissionTier::ReadOnly);
    let approved = policy.with_one_time_approval(PermissionTier::Write);

    assert_eq!(
        approved.decide("buffer_write", PermissionTier::Write),
        Decision::Allow
    );
    assert_eq!(
        approved.decide("shell_exec", PermissionTier::Shell),
        Decision::Ask,
        "approving a Write call must not also auto-approve Shell"
    );
    // And the original policy is untouched — approval is per-call, not a
    // mutation of the session's policy.
    assert_eq!(
        policy.decide("buffer_write", PermissionTier::Write),
        Decision::Ask
    );
}

// ---------------------------------------------------------------------------
// Composition properties
// ---------------------------------------------------------------------------

/// `with_hard_ceiling` only ever lowers, on both axes — the never-escalate
/// property ADR-051 depends on. Exhaustive over (existing, declared) pairs
/// rather than the two orderings someone happened to think of.
#[test]
fn with_hard_ceiling_only_ever_lowers() {
    for start in ALL_TIERS {
        for existing in ALL_TIERS {
            for declared in ALL_TIERS {
                let base = PermissionPolicy {
                    auto_approve_up_to: start,
                    hard_ceiling: Some(HardCeiling {
                        tier: existing,
                        source: HardCeilingSource::SessionDeclared,
                    }),
                    allowed_categories: None,
                    live: None,
                };
                let tightened = base.with_hard_ceiling(HardCeiling {
                    tier: declared,
                    source: HardCeilingSource::UnparseableDeclaration,
                });
                let hc = tightened.hard_ceiling.expect("must keep a hard ceiling");
                assert_eq!(
                    hc.tier,
                    existing.min(declared),
                    "start={start:?} existing={existing:?} declared={declared:?}"
                );
                assert!(
                    tightened.auto_approve_up_to <= start,
                    "the auto-approval ceiling must never rise"
                );
                assert!(
                    tightened.auto_approve_up_to <= hc.tier,
                    "there must be no Allow band above the Deny line"
                );
            }
        }
    }
}

/// Order-independence: tightening with two ceilings converges on the same
/// policy regardless of which is applied first. A composition rule that is not
/// commutative would make a session's effective policy depend on wire ordering.
#[test]
fn hard_ceiling_composition_is_order_independent() {
    for a in ALL_TIERS {
        for b in ALL_TIERS {
            let base = ceiling(PermissionTier::Privileged);
            let ab = base
                .with_hard_ceiling(HardCeiling {
                    tier: a,
                    source: HardCeilingSource::SessionDeclared,
                })
                .with_hard_ceiling(HardCeiling {
                    tier: b,
                    source: HardCeilingSource::SessionDeclared,
                });
            let ba = base
                .with_hard_ceiling(HardCeiling {
                    tier: b,
                    source: HardCeilingSource::SessionDeclared,
                })
                .with_hard_ceiling(HardCeiling {
                    tier: a,
                    source: HardCeilingSource::SessionDeclared,
                });
            assert_eq!(
                ab.auto_approve_up_to, ba.auto_approve_up_to,
                "a={a:?} b={b:?}"
            );
            assert_eq!(
                ab.hard_ceiling.map(|h| h.tier),
                ba.hard_ceiling.map(|h| h.tier),
                "a={a:?} b={b:?}"
            );
        }
    }
}

/// The ambient Scheme tier (ADR-084 D2/D7) is the `Allow` line, never the hard
/// ceiling. Guest Scheme cannot be prompted mid-evaluation, so anything merely
/// *askable* must not be ambiently granted — that would be the silent
/// `Ask`-as-`Allow` promotion D3 forbids, in the one place with no UI to catch
/// it.
#[test]
fn the_ambient_scheme_tier_is_the_allow_line_not_the_deny_line() {
    for auto in ALL_TIERS {
        for hard in ALL_TIERS {
            let policy = PermissionPolicy {
                auto_approve_up_to: auto,
                hard_ceiling: Some(HardCeiling {
                    tier: hard,
                    source: HardCeilingSource::SessionDeclared,
                }),
                allowed_categories: None,
                live: None,
            };
            let ambient = policy.ambient_scheme_tier();
            assert!(
                ambient <= auto,
                "ambient {ambient:?} exceeded the Allow line {auto:?}"
            );
            assert!(
                ambient <= hard,
                "ambient {ambient:?} exceeded the hard ceiling {hard:?}"
            );
            // Everything the ambient tier grants must have been an `Allow`.
            assert_eq!(
                policy.decide_tier(ambient),
                Decision::Allow,
                "the ambient tier must itself be auto-approved"
            );
        }
    }
    // With no hard ceiling it is exactly the Allow line — not Privileged,
    // which is what the VM defaults to when nothing lowers it.
    assert_eq!(
        ceiling(PermissionTier::ReadOnly).ambient_scheme_tier(),
        PermissionTier::ReadOnly
    );
    assert_eq!(
        PermissionPolicy::default().ambient_scheme_tier(),
        PermissionTier::ReadOnly,
        "the shipped default must not ambiently grant Scheme more than reads"
    );
}

/// The two messages must be distinguishable. An operator needs to tell
/// "policy forbids this, fix the policy" from "nobody was around to approve
/// it, raise the ceiling or use an interactive surface".
#[test]
fn an_ask_denial_reads_differently_from_a_real_denial() {
    let real = deny_message(
        "shell_exec",
        PermissionTier::Shell,
        DenyReason::HardCeiling(HardCeiling {
            tier: PermissionTier::ReadOnly,
            source: HardCeilingSource::SessionDeclared,
        }),
    );
    let asked = ask_denied_message(
        "shell_exec",
        PermissionTier::Shell,
        PermissionTier::ReadOnly,
        "--prompt mode",
    );
    assert_ne!(real, asked);
    assert!(real.contains("declared ceiling"), "{real}");
    assert!(
        !real.contains("no human"),
        "a real denial is not about a missing human: {real}"
    );
    assert!(asked.contains("no human to confirm"), "{asked}");
    assert!(asked.contains("--prompt mode"), "{asked}");
}

// ---------------------------------------------------------------------------
// ADR-084 D7 — Gate 2: the live tier, and the three invariants it must not break.
//
// These matter more than the feature. A live auto-approval line is a security
// control that can move underneath an in-flight decision, so each test below
// pins one way that must NOT be possible.
// ---------------------------------------------------------------------------

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
