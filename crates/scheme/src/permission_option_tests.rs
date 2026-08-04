//! Decision #6, Scheme half: the option-setting primitives are the Scheme
//! surface's self-escalation path, and must stay closed.
//!
//! `set_option` (MCP) is `Write` tier, so decision #6 installs an
//! argument-sensitive guard there: setting `ai_tier` — the option ADR-084 D7
//! makes reach the enforced policy — requires `Privileged`. The Scheme surface
//! reaches the identical effect through `(set-option! "ai_tier" "privileged")`,
//! `(set-option-save! …)` (which additionally writes `init.scm`, i.e. code the
//! editor evaluates at next startup), and `(set-local-option! …)`.
//!
//! These are classified `tier::PRIVILEGED` today. That is not an accident of
//! the D3 sweep and it is not "config primitives are vaguely sensitive" — it is
//! specifically what stops a `Write`-tier session raising its own ceiling, and
//! `Privileged` is the ceiling of the lattice, so there is nothing stricter
//! available to fall back on if it were relaxed. Hence this file: an explicit,
//! reasoned pin, so a future "it's just an option, `WRITE` is fine" change
//! fails a test that says why rather than silently reopening the path.
//!
//! Attacker-first per principle #14: the primary assertions are refusals, and
//! the effect-level oracle is the pending-mutation queue — an assertion on the
//! error string alone would pass even if the option change had been queued.

use super::*;
use crate::permission::{PermissionTier, PrimitiveTier};
use crate::value::Value;

/// Every primitive that can set an editor option, i.e. every Scheme route to
/// the permission-tier option. Enumerated here rather than derived, because
/// the point of the test is to notice when a *new* route appears: a primitive
/// that queues into `pending_options` / `pending_option_saves` /
/// `pending_local_options` and is not in this list will fail
/// `no_unlisted_scheme_route_reaches_the_option_queues`.
const OPTION_SETTING_PRIMITIVES: &[&str] =
    &["set-option!", "set-option-save!", "set-local-option!"];

/// A runtime with editor state injected. `inject_editor_state` is what
/// registers the state-reading primitives AND populates `option_values` (which
/// `set-option-save!` validates the option name against), so a bare `new()`
/// runtime would sweep a smaller API than the one that ships and would make
/// the control case fail for a reason unrelated to permissions.
fn new_runtime() -> SchemeRuntime {
    let mut rt = SchemeRuntime::new().unwrap();
    rt.inject_editor_state(&mae_core::Editor::new());
    rt
}

fn declared_tier(rt: &SchemeRuntime, name: &str) -> PrimitiveTier {
    match rt.vm.globals.get(name) {
        Some(Value::Foreign(ff)) => ff.tier,
        other => panic!("{name} is not a registered primitive: {other:?}"),
    }
}

/// Total pending option mutations of every kind — the effect-level oracle.
fn queued_option_mutations(rt: &SchemeRuntime) -> usize {
    let s = rt.shared.lock();
    s.pending_options.len() + s.pending_option_saves.len() + s.pending_local_options.len()
}

/// The pin. `Privileged` is load-bearing, not decorative.
#[test]
fn every_option_setting_primitive_requires_privileged() {
    let rt = new_runtime();
    for name in OPTION_SETTING_PRIMITIVES {
        assert_eq!(
            declared_tier(&rt, name),
            PrimitiveTier::Requires(PermissionTier::Privileged),
            "{name} must require Privileged: it can set `ai_tier`, which ADR-084 D7 makes \
             the enforced policy, so anything weaker is a self-escalation path. See \
             docs/DECISIONS_FOR_REVIEW.md #6."
        );
    }
}

/// The attacker's case: a session below `Privileged` tries every Scheme route
/// to raise its own tier, and none of them queue anything.
#[test]
fn a_sub_privileged_session_cannot_set_the_permission_tier_option() {
    for ambient in [
        PermissionTier::ReadOnly,
        PermissionTier::Write,
        PermissionTier::Shell,
    ] {
        for name in OPTION_SETTING_PRIMITIVES {
            // Both registry spellings, and several accepted tier values —
            // no single hand-picked call that might dodge the edge.
            for option in ["ai_tier", "ai-tier"] {
                for value in ["privileged", "Privileged", "full"] {
                    let mut rt = new_runtime();
                    let code = format!(r#"({name} "{option}" "{value}")"#);
                    let err = rt
                        .with_ambient_tier(ambient, |rt| rt.eval(&code))
                        .expect_err(&format!("{code} must be refused at {ambient:?}"));
                    assert!(
                        err.message.contains("permission denied"),
                        "{code} at {ambient:?} failed for the wrong reason: {}",
                        err.message
                    );
                    assert_eq!(
                        queued_option_mutations(&rt),
                        0,
                        "{code} at {ambient:?} was refused but still queued the change"
                    );
                }
            }
        }
    }
}

/// The control. Without this the refusals above could be satisfied by calls
/// that never worked at any tier.
#[test]
fn a_privileged_session_can_still_set_options() {
    for name in OPTION_SETTING_PRIMITIVES {
        let mut rt = new_runtime();
        let code = format!(r#"({name} "ai_tier" "privileged")"#);
        rt.with_ambient_tier(PermissionTier::Privileged, |rt| rt.eval(&code))
            .unwrap_or_else(|e| panic!("{code} should work at Privileged: {}", e.message));
        assert_eq!(
            queued_option_mutations(&rt),
            1,
            "{code} at Privileged should have queued exactly one option mutation"
        );
    }
}

/// Refusal must not depend on the *shape* of the attempt. A denied primitive
/// stays denied when it is captured into another binding, aliased, applied, or
/// reached through `map` — the same evasions `permission_tests.rs` covers for
/// `shell-command`, re-run against the escalation path specifically, since this
/// is the one where success would hand the attacker every other primitive too.
#[test]
fn the_refusal_survives_capture_aliasing_and_apply() {
    let routes = [
        r#"(let ((f set-option!)) (f "ai_tier" "privileged"))"#,
        r#"(begin (define g set-option!) (g "ai_tier" "privileged"))"#,
        r#"(apply set-option! (list "ai_tier" "privileged"))"#,
        r#"(map (lambda (v) (set-option! "ai_tier" v)) (list "privileged"))"#,
        r#"((lambda () (set-option-save! "ai_tier" "privileged")))"#,
    ];
    for route in routes {
        let mut rt = new_runtime();
        let result = rt.with_ambient_tier(PermissionTier::Write, |rt| rt.eval(route));
        assert!(
            result.is_err(),
            "{route} escaped the chokepoint at Write tier"
        );
        assert_eq!(
            queued_option_mutations(&rt),
            0,
            "{route} queued an option change despite the denial"
        );
    }
}

/// The coverage ratchet. If a new primitive learns to queue an option
/// mutation, it must be in `OPTION_SETTING_PRIMITIVES` (and therefore pinned at
/// `Privileged` by the first test) — otherwise it is a new, unpinned route to
/// the same escalation.
///
/// Source-text based, deliberately, and *not* by calling every registered
/// primitive: a runtime sweep at `Privileged` would have to actually invoke
/// ~550 primitives — including the ones that spawn processes, open sockets, and
/// block on external servers — to observe whether each queued an option change.
/// Reading the registration sites answers the same question without executing
/// anything.
///
/// Approach: split each runtime source on `register_fn`, take the first quoted
/// string in each chunk (the primitive's name) and check whether that chunk
/// pushes onto one of the three option queues. It is a heuristic over source
/// text, like `mae-ai`'s `dispatch_contract_tests`; a surprising result here is
/// a prompt to look at the registration site, not to loosen the check.
#[test]
fn no_unlisted_scheme_route_reaches_the_option_queues() {
    const QUEUE_PUSHES: &[&str] = &[
        "pending_options.push",
        "pending_option_saves.push",
        "pending_local_options.push",
    ];
    // Every module that registers primitives. A new module that registers an
    // option setter and is missing from this list would go unchecked, so the
    // list is asserted non-trivial below.
    const SOURCES: &[(&str, &str)] = &[
        ("editor_ops.rs", include_str!("runtime/editor_ops.rs")),
        ("io_packages.rs", include_str!("runtime/io_packages.rs")),
        ("kb_crud.rs", include_str!("runtime/kb_crud.rs")),
        ("kb_export.rs", include_str!("runtime/kb_export.rs")),
        ("kb_graph_view.rs", include_str!("runtime/kb_graph_view.rs")),
        ("kb_preview.rs", include_str!("runtime/kb_preview.rs")),
        ("kb_primitives.rs", include_str!("runtime/kb_primitives.rs")),
        ("kb_queries.rs", include_str!("runtime/kb_queries.rs")),
        ("keybindings.rs", include_str!("runtime/keybindings.rs")),
        ("lsp_dap.rs", include_str!("runtime/lsp_dap.rs")),
        (
            "misc_primitives.rs",
            include_str!("runtime/misc_primitives.rs"),
        ),
        ("shell_agenda.rs", include_str!("runtime/shell_agenda.rs")),
        (
            "test_primitives.rs",
            include_str!("runtime/test_primitives.rs"),
        ),
        (
            "state_sync_inject.rs",
            include_str!("runtime/state_sync_inject.rs"),
        ),
        (
            "state_sync_inject_kb.rs",
            include_str!("runtime/state_sync_inject_kb.rs"),
        ),
        (
            "state_sync_apply.rs",
            include_str!("runtime/state_sync_apply.rs"),
        ),
        (
            "state_sync_apply2.rs",
            include_str!("runtime/state_sync_apply2.rs"),
        ),
    ];

    let mut seen_registrations = 0usize;
    let mut unlisted: Vec<String> = Vec::new();
    for (path, src) in SOURCES {
        for chunk in src.split("register_fn").skip(1) {
            let Some(open) = chunk.find('"') else {
                continue;
            };
            let Some(close) = chunk[open + 1..].find('"') else {
                continue;
            };
            let name = &chunk[open + 1..open + 1 + close];
            seen_registrations += 1;
            // Bound the chunk at the next registration so a push belonging to
            // a later primitive is not attributed to this one — `split` already
            // does that, since each chunk ends where the next `register_fn`
            // begins.
            if QUEUE_PUSHES.iter().any(|p| chunk.contains(p))
                && !OPTION_SETTING_PRIMITIVES.contains(&name)
            {
                unlisted.push(format!("{path}: {name}"));
            }
        }
    }

    assert!(
        seen_registrations > 200,
        "sanity: only {seen_registrations} registration sites scanned — SOURCES or the \
         `register_fn` convention drifted"
    );
    assert!(
        unlisted.is_empty(),
        "these primitives queue an editor-option change but are not pinned at Privileged \
         by OPTION_SETTING_PRIMITIVES: {unlisted:?}"
    );
    // ...and the detector really fires, so the empty result above is not
    // vacuous: the three known setters must all have been found this way.
    let mut found_known = 0usize;
    for (_path, src) in SOURCES {
        for chunk in src.split("register_fn").skip(1) {
            let Some(open) = chunk.find('"') else {
                continue;
            };
            let Some(close) = chunk[open + 1..].find('"') else {
                continue;
            };
            let name = &chunk[open + 1..open + 1 + close];
            if OPTION_SETTING_PRIMITIVES.contains(&name)
                && QUEUE_PUSHES.iter().any(|p| chunk.contains(p))
            {
                found_known += 1;
            }
        }
    }
    assert_eq!(
        found_known,
        OPTION_SETTING_PRIMITIVES.len(),
        "the scan found {found_known} of {} known option setters — it is not detecting \
         what it claims to",
        OPTION_SETTING_PRIMITIVES.len()
    );
}
