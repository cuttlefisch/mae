//! ADR-084 D3 enforcement tests.
//!
//! These are written attacker-first (CLAUDE.md #14): the primary assertions are
//! that a denied primitive does **not** run, that the denial cannot be captured,
//! caught, retried, or argued out of, and that the ambient tier cannot be moved
//! upward by anything a Scheme program can say. The "it still works normally"
//! cases exist to prove the refusals are not vacuous.

use super::*;
use crate::permission::{PermissionTier, PrimitiveTier};
use crate::value::Value;
use mae_core::Editor;

const ALL_TIERS: [PermissionTier; 4] = [
    PermissionTier::ReadOnly,
    PermissionTier::Write,
    PermissionTier::Shell,
    PermissionTier::Privileged,
];

fn new_runtime() -> SchemeRuntime {
    SchemeRuntime::new().unwrap()
}

/// A runtime with editor state injected — the state-reading primitives
/// (`buffer-string`, `current-buffer-name`, …) are registered by
/// `inject_editor_state`, not by `new()`, so a test that only calls `new()`
/// silently sweeps a smaller API than the one that ships.
fn runtime_with_editor() -> (SchemeRuntime, Editor) {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    (rt, editor)
}

/// A per-test temp directory, so parallel tests never share a path.
fn temp_dir(test: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mae-perm-{}-{}-{}",
        test,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The declared tier of a registered primitive, read back out of the VM rather
/// than restated in the test — so the oracle tracks what actually shipped.
fn declared_tier(rt: &SchemeRuntime, name: &str) -> PrimitiveTier {
    match rt.vm.globals.get(name) {
        Some(Value::Foreign(ff)) => ff.tier,
        other => panic!("{name} is not a registered primitive: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The attacker's tests
// ---------------------------------------------------------------------------

/// The effect-level oracle: a process-spawning primitive denied below its tier
/// must not have spawned anything. Asserting on the error string alone would
/// pass even if the command ran and the error came from somewhere else.
#[test]
fn denied_shell_primitive_does_not_spawn_the_process() {
    let dir = temp_dir("no-spawn");
    let marker = dir.join("pwned");
    let code = format!(r#"(shell-command "touch '{}'")"#, marker.to_str().unwrap());

    // Below Shell: refused, and nothing ran.
    for ambient in [PermissionTier::ReadOnly, PermissionTier::Write] {
        let mut rt = new_runtime();
        let err = rt
            .with_ambient_tier(ambient, |rt| rt.eval(&code))
            .expect_err("shell-command must be refused below the shell tier");
        assert!(
            err.message.contains("permission denied"),
            "denial should say so plainly, got: {}",
            err.message
        );
        assert!(
            err.message.contains("shell") && err.message.contains(ambient.config_name()),
            "denial must name the required tier and the tier in force, got: {}",
            err.message
        );
        assert!(
            !marker.exists(),
            "the process ran despite the denial (ambient {:?})",
            ambient
        );
    }

    // At Shell the very same expression really does spawn — without this the
    // assertions above would be satisfied by a command that could never work.
    let mut rt = new_runtime();
    rt.with_ambient_tier(PermissionTier::Shell, |rt| rt.eval(&code))
        .expect("shell-command should run at the shell tier");
    assert!(
        marker.exists(),
        "control case failed: the command never worked, so the denials proved nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Absence is not enough: a `Value::Foreign` captured into another binding
/// outlives any environment swap and stays callable. The chokepoint is what
/// makes the captured reference worthless. Each route below is a way a real
/// program can hold onto a primitive.
#[test]
fn a_captured_primitive_reference_is_still_refused() {
    let dir = temp_dir("captured");
    let marker = dir.join("pwned");
    let touch = format!("touch '{}'", marker.to_str().unwrap());

    let routes = [
        // Aliased into a fresh global at full authority, called later.
        format!(r#"(define alias shell-command) (alias "{touch}")"#),
        // Closed over by a lambda — the classic "capability escapes the scope".
        format!(r#"(define grab (let ((f shell-command)) (lambda (c) (f c)))) (grab "{touch}")"#),
        // Smuggled through a data structure.
        format!(r#"(define box (list shell-command)) ((car box) "{touch}")"#),
        // Reached through apply, so the call is not even syntactically a call.
        format!(r#"(apply shell-command (list "{touch}"))"#),
        // Reached through a higher-order primitive.
        format!(r#"(map shell-command (list "{touch}"))"#),
        // Reached through a second evaluation layer: ADR-084 D6 — eval's own
        // tier says who may invoke the evaluator, it does not contain what the
        // evaluated code reaches.
        format!(r#"(eval (list 'shell-command "{touch}") (interaction-environment))"#),
    ];

    for route in &routes {
        let mut rt = new_runtime();
        // The capture itself happens at full authority — the attacker is
        // assumed to have gotten the reference, not to have been stopped from
        // taking it.
        let result = rt.with_ambient_tier(PermissionTier::Write, |rt| rt.eval(route));
        assert!(
            result.is_err(),
            "capture route slipped through the chokepoint: {route}"
        );
        assert!(
            !marker.exists(),
            "the process ran via a captured reference: {route}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR-084 D5: the denial aborts the evaluation. If `guard` could catch it, a
/// denied program would simply retry — and a "boundary" you can loop against is
/// not one.
#[test]
fn a_denial_cannot_be_caught_or_retried_from_scheme() {
    let dir = temp_dir("uncatchable");
    let marker = dir.join("pwned");
    let touch = format!("touch '{}'", marker.to_str().unwrap());

    let attempts = [
        format!(r#"(guard (e (#t 'caught)) (shell-command "{touch}"))"#),
        format!(
            r#"(with-exception-handler (lambda (e) 'swallowed) (lambda () (shell-command "{touch}")))"#
        ),
        // Retry loop: if the first denial were catchable, this would spin until
        // one got through.
        format!(
            r#"(define (try n) (guard (e (#t (if (> n 0) (try (- n 1)) 'gave-up))) (shell-command "{touch}"))) (try 5)"#
        ),
    ];

    for attempt in &attempts {
        let mut rt = new_runtime();
        let result = rt.with_ambient_tier(PermissionTier::Write, |rt| rt.eval(attempt));
        match result {
            Ok(v) => panic!("denial was swallowed by a handler, evaluated to {v}: {attempt}"),
            Err(e) => assert!(
                e.message.contains("permission denied"),
                "aborted for the wrong reason: {}",
                e.message
            ),
        }
        assert!(!marker.exists(), "the process ran: {attempt}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The transitive route inside this crate: a lower-tier program cannot even
/// *enqueue* work for the editor to run later. `run-command` hands a command
/// name to the editor's own dispatch, which is how Scheme reaches effects it
/// has no primitive for; refusing it means the queue stays empty rather than
/// draining into the effect one loop iteration later.
#[test]
fn a_denied_dispatcher_never_reaches_the_editors_queue() {
    for (code, queue_name) in [
        (r#"(run-command "shell-command")"#, "pending_commands"),
        (r#"(execute-ex "shell-command ls")"#, "pending_ex_commands"),
    ] {
        let mut rt = new_runtime();
        let err = rt
            .with_ambient_tier(PermissionTier::Write, |rt| rt.eval(code))
            .expect_err("command dispatch from Scheme must be refused below its tier");
        assert!(
            err.message.contains("permission denied"),
            "unexpected error for {code}: {}",
            err.message
        );
        let st = rt.shared.lock();
        assert!(
            st.pending_commands.is_empty() && st.pending_ex_commands.is_empty(),
            "{queue_name} was populated despite the denial ({code})"
        );
    }
}

/// Registry-driven, so it cannot fall behind what ships: at the lowest tier,
/// *every* primitive classified above ReadOnly is refused. Only the ones that
/// must be refused are actually invoked, so no permitted primitive is executed
/// as a side effect of the sweep.
#[test]
fn every_primitive_above_readonly_is_refused_at_readonly() {
    let (rt, _editor) = runtime_with_editor();
    let names: Vec<String> = rt
        .vm
        .globals
        .iter()
        .filter_map(|(name, value)| match value {
            Value::Foreign(ff) => match ff.tier.required() {
                Some(t) if t > PermissionTier::ReadOnly => Some(name.clone()),
                Some(_) => None,
                None => None,
            },
            _ => None,
        })
        .collect();
    drop(rt);

    assert!(
        names.len() > 50,
        "expected the sweep to cover a meaningful slice of the API, got {}",
        names.len()
    );

    let mut escaped = Vec::new();
    for name in &names {
        let (mut rt, _editor) = runtime_with_editor();
        let result =
            rt.with_ambient_tier(PermissionTier::ReadOnly, |rt| rt.eval(&format!("({name})")));
        match result {
            Err(e) if e.message.contains("permission denied") => {}
            other => escaped.push(format!("{name}: {other:?}")),
        }
    }
    assert!(
        escaped.is_empty(),
        "primitives above ReadOnly that were not refused at ReadOnly: {escaped:#?}"
    );
}

/// The ordering property itself, over every (primitive, ambient) pair rather
/// than one hand-picked combination: refused exactly when `ambient < required`.
#[test]
fn refusal_follows_the_lattice_for_every_tier_pair() {
    // One representative per classification, chosen because their effects are
    // observable and harmless to attempt.
    let probes = [
        "buffer-string",
        "create-buffer",
        "shell-command",
        "run-command",
    ];

    for probe in probes {
        let (rt, _editor) = runtime_with_editor();
        let required = declared_tier(&rt, probe)
            .required()
            .unwrap_or_else(|| panic!("{probe} should declare a required tier"));
        drop(rt);

        for ambient in ALL_TIERS {
            let (mut rt, _editor) = runtime_with_editor();
            let result = rt.with_ambient_tier(ambient, |rt| rt.eval(&format!("({probe})")));
            let denied = matches!(&result, Err(e) if e.message.contains("permission denied"));
            assert_eq!(
                denied,
                ambient < required,
                "{probe} (requires {:?}) at ambient {:?}: denied={denied}, result={result:?}",
                required,
                ambient
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The ambient tier itself
// ---------------------------------------------------------------------------

/// Nothing a Scheme program can say moves the ambient tier. This is the
/// property that separates a defensible ambient check from a confused deputy.
#[test]
fn the_ambient_tier_cannot_be_raised_from_scheme() {
    let hostile = [
        // Direct attempts at a name that might exist.
        "(set-ambient-tier! 'privileged)",
        "(ambient-tier)",
        "(define ambient-tier 'privileged)",
        // Through the option system, which is how tiers are configured.
        r#"(set-option! "ai_tier" "full")"#,
        r#"(set-option! "ai-tier" "privileged")"#,
        // Through redefinition, which principle #6 guarantees is allowed.
        "(define shell-command (lambda (c) 'redefined))",
        // Through a second evaluation layer.
        "(eval '(define ambient-tier 'privileged) (interaction-environment))",
    ];

    let dir = temp_dir("no-raise");
    let marker = dir.join("pwned");
    let touch = format!(r#"(shell-command "touch '{}'")"#, marker.to_str().unwrap());

    for attempt in hostile {
        let (mut rt, _editor) = runtime_with_editor();
        rt.with_ambient_tier(PermissionTier::ReadOnly, |rt| {
            // The attempt may fail (denied, undefined, whatever) — what matters
            // is only that the tier is unchanged and the effect stays out of
            // reach afterwards.
            let _ = rt.eval(attempt);
            assert_eq!(
                rt.ambient_tier(),
                PermissionTier::ReadOnly,
                "ambient tier moved after: {attempt}"
            );
            // The effect-level oracle, not the error string: shadowing
            // `shell-command` with a lambda (principle #6 guarantees that is
            // allowed) legitimately makes the *call* succeed as a no-op. Hiding
            // the primitive is not the same as acquiring its authority, and the
            // marker file is what tells the two apart.
            let _ = rt.eval(&touch);
        });
        assert!(!marker.exists(), "the process ran after: {attempt}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Monotonicity as a property over all ordered pairs, not one nesting.
#[test]
fn nesting_the_tier_is_monotone_non_increasing() {
    for outer in ALL_TIERS {
        for inner in ALL_TIERS {
            let mut rt = new_runtime();
            rt.with_ambient_tier(outer, |rt| {
                assert_eq!(rt.ambient_tier(), outer.min(PermissionTier::Privileged));
                rt.with_ambient_tier(inner, |rt| {
                    assert_eq!(
                        rt.ambient_tier(),
                        outer.min(inner),
                        "nesting {inner:?} inside {outer:?} must yield the minimum"
                    );
                });
                assert_eq!(
                    rt.ambient_tier(),
                    outer,
                    "the inner scope must restore, not leak, the tier"
                );
            });
        }
    }
}

/// A fresh runtime carries the human's own authority — the drop is the host's
/// job at the entry point that knows whose code is about to run. If this ever
/// reads as something lower, config loading silently half-works instead of
/// failing loudly, which is worse than either extreme.
#[test]
fn a_fresh_runtime_runs_at_full_authority() {
    let rt = new_runtime();
    assert_eq!(rt.ambient_tier(), PermissionTier::Privileged);
}

// ---------------------------------------------------------------------------
// Not-vacuous: ordinary use must survive
// ---------------------------------------------------------------------------

/// A `write`-tier session is the common case. If arithmetic or buffer reads
/// stopped working there, the mechanism would be traded for a regression.
#[test]
fn ordinary_scheme_still_evaluates_at_the_write_tier() {
    let (mut rt, _editor) = runtime_with_editor();
    rt.with_ambient_tier(PermissionTier::Write, |rt| {
        assert_eq!(rt.eval("(+ 1 2 3)").unwrap(), "6");
        assert_eq!(
            rt.eval(r#"(string-append "hello" " " "world")"#).unwrap(),
            "hello world"
        );
        assert_eq!(rt.eval("(length (list 1 2 3))").unwrap(), "3");
        // Editor state: a read and an ordinary edit, both of which a write-tier
        // agent is expected to be able to do.
        rt.eval("(current-buffer-name)")
            .expect("reading the buffer name must work at write tier");
        rt.eval("(buffer-string)")
            .expect("reading buffer text must work at write tier");
        rt.eval(r#"(create-buffer "*perm-test*")"#)
            .expect("creating a buffer must work at write tier");
        rt.eval(r#"(buffer-insert "hello")"#)
            .expect("inserting text must work at write tier");
    });
}

/// A denial must leave the VM in a usable state. It unwinds out of the middle
/// of a call, so a stack imbalance here would show up as corruption on the next
/// expression rather than as a failed test.
#[test]
fn the_runtime_survives_a_denial() {
    let (mut rt, _editor) = runtime_with_editor();
    rt.with_ambient_tier(PermissionTier::Write, |rt| {
        for _ in 0..3 {
            assert!(rt.eval(r#"(shell-command "echo hi")"#).is_err());
            assert_eq!(
                rt.eval("(+ 40 2)").unwrap(),
                "42",
                "the VM should still evaluate after a denial"
            );
        }
        // Deep inside a call chain, too.
        assert!(rt
            .eval(r#"(define (f) (g)) (define (g) (shell-command "echo hi")) (f)"#)
            .is_err());
        assert_eq!(rt.eval("(* 6 7)").unwrap(), "42");
    });
}

/// Classification sanity: the distribution must not have drifted to the top.
/// If every primitive were Privileged the mechanism would "pass" every test
/// above while making the editor unusable for any bounded session.
#[test]
fn the_classification_is_not_uniformly_at_the_top() {
    let (rt, _editor) = runtime_with_editor();
    let mut counts = std::collections::HashMap::new();
    let mut total = 0usize;
    for (_, value) in rt.vm.globals.iter() {
        if let Value::Foreign(ff) = value {
            total += 1;
            *counts.entry(ff.tier.label()).or_insert(0usize) += 1;
        }
    }
    assert!(total > 300, "expected the full primitive set, got {total}");
    let privileged = *counts.get("privileged").unwrap_or(&0);
    assert!(
        privileged * 4 < total,
        "over a quarter of primitives are Privileged ({privileged}/{total}) — \
         the classification has drifted to the top: {counts:?}"
    );
    for label in ["unrestricted", "readonly", "write", "shell", "privileged"] {
        assert!(
            counts.get(label).copied().unwrap_or(0) > 0,
            "no primitive classified {label}: {counts:?}"
        );
    }
}
