//! End-to-end dispatch tests for decision #6: the KB sharing/membership tools
//! are `Privileged`, and no surface launders them back down to `Write`.
//!
//! These go through `execute_tool` rather than reading the tool table, because
//! the table is not what an attacker calls. The attacker's test comes first
//! (principle #14): a `Write`-tier session is *refused*, on every spelling of
//! every raised operation, and the permitted case is checked second only to
//! prove the refusal was the tier and not a broken call.

use mae_core::{CommandRegistry, Editor, OptionRegistry};

use crate::executor::{execute_tool, ExecuteResult};
use crate::tools::{
    ai_specific_tools, authorization::DELIBERATELY_NOT_AUTHORIZATION_CHANGES,
    sanitize_command_name, tools_from_registry, PermissionPolicy, AUTHORIZATION_CHANGE_OPS,
};
use crate::types::{PermissionTier, ToolCall, ToolDefinition};

fn all_tools() -> Vec<ToolDefinition> {
    let mut tools = tools_from_registry(&CommandRegistry::with_builtins());
    tools.extend(ai_specific_tools(&OptionRegistry::new()));
    tools
}

fn policy(tier: PermissionTier) -> PermissionPolicy {
    PermissionPolicy {
        auto_approve_up_to: tier,
        ..PermissionPolicy::default()
    }
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "t".into(),
        name: name.into(),
        arguments: args,
    }
}

fn run(name: &str, args: serde_json::Value, tier: PermissionTier) -> (bool, String) {
    let mut editor = Editor::new();
    match execute_tool(&mut editor, &call(name, args), &all_tools(), &policy(tier)) {
        ExecuteResult::Immediate(r) => (r.success, r.output),
        ExecuteResult::Deferred { .. } => (true, "deferred".into()),
        // ADR-090: the oracle these tests care about is "the authorization
        // change did NOT happen". `NeedsApproval` satisfies it -- nothing ran
        // -- and this crate is a non-interactive surface for the purposes of a
        // unit test, so it takes the documented D3 mapping rather than
        // inventing a local one.
        ExecuteResult::NeedsApproval(req) => {
            let r = req.into_denied("this test harness");
            (r.success, r.output)
        }
    }
}

/// Plausible arguments for each raised op, so a refusal cannot be confused
/// with schema validation rejecting the call before the tier check. (Argument
/// validation runs *after* the permission check in
/// `execute_tool_dispatch_body`, but a well-formed call makes the allowed-case
/// half of each test meaningful too.)
fn args_for(op: &str) -> serde_json::Value {
    match op {
        "kb_share" | "kb_share_p2p" => serde_json::json!({"kb_id": "adversarial-fixture"}),
        "kb_add_member" | "kb_approve" => serde_json::json!({
            "kb_id": "adversarial-fixture",
            "member": "SHA256:0000000000000000000000000000000000000000000",
            "role": "editor",
        }),
        "kb_remove_member" | "kb_unblock_member" | "kb_block_member" => serde_json::json!({
            "kb_id": "adversarial-fixture",
            "member": "SHA256:0000000000000000000000000000000000000000000",
        }),
        "kb_set_policy" => {
            serde_json::json!({"kb_id": "adversarial-fixture", "policy": "permissive"})
        }
        "kb_set_encryption" => serde_json::json!({"kb_id": "adversarial-fixture", "mode": "e2e"}),
        "kb_set_ai_residency" => serde_json::json!({"kb": "primary", "policy": "open"}),
        "kb_join" | "kb_leave" => serde_json::json!({"kb_id": "adversarial-fixture"}),
        "kb_join_p2p" => serde_json::json!({"ticket": "mae://join/deadbeef"}),
        "kb_set_role" => serde_json::json!({"id": "concept:x", "role": "atom"}),
        _ => serde_json::json!({}),
    }
}

/// The attacker's case. A `Write`-tier session is exactly the configuration an
/// operator picks to allow buffer edits while withholding shell access — and it
/// must not be able to grant a third party access to a knowledge base, through
/// *any* of the three surfaces that reach the effect.
#[test]
fn a_write_tier_session_is_refused_every_authorization_change_on_every_surface() {
    for op in AUTHORIZATION_CHANGE_OPS {
        let kebab = op.replace('_', "-");
        let mirror = format!("command_{}", sanitize_command_name(&kebab));

        for (surface, name, args) in [
            ("direct tool", op.to_string(), args_for(op)),
            ("command mirror", mirror, serde_json::json!({})),
            (
                "execute_command",
                "execute_command".to_string(),
                serde_json::json!({ "command": kebab }),
            ),
        ] {
            // The command mirror only exists for ops that are registered
            // commands; skip the ones that are not, rather than asserting a
            // vacuous pass on an unknown tool.
            if surface == "command mirror" && !all_tools().iter().any(|t| t.name == name) {
                continue;
            }
            let (success, output) = run(&name, args, PermissionTier::Write);
            assert!(
                !success,
                "{surface} '{name}' SUCCEEDED at Write tier — {op} is an authorization change"
            );
            assert!(
                output.contains("Permission denied"),
                "{surface} '{name}' failed at Write tier, but not on permission: {output}"
            );
            assert!(
                output.contains("Privileged"),
                "{surface} '{name}' denial must name the tier it needs: {output}"
            );
        }
    }
}

/// "Raised to Privileged" must mean *above Shell*, not merely above ReadOnly.
/// A raise that stopped at Shell would look green in the Write test above and
/// change nothing for an operator who deliberately grants shell access — which
/// is the configuration MAE's own development runs under.
#[test]
fn a_shell_tier_policy_is_also_refused() {
    for op in AUTHORIZATION_CHANGE_OPS {
        let (success, output) = run(op, args_for(op), PermissionTier::Shell);
        assert!(!success, "{op} succeeded at Shell tier");
        assert!(
            output.contains("Permission denied"),
            "{op} at Shell tier: {output}"
        );
    }
    // ADR-090 D5: and the shipped default is now *below* every tier tested
    // above, so none of these tests is accidentally asserting the default.
    assert_eq!(
        PermissionPolicy::default().auto_approve_up_to,
        PermissionTier::ReadOnly
    );
}

/// The permitted half. A `Privileged` session must get *past* the tier gate —
/// the call may still fail on a missing daemon, which is a different failure
/// with a different message, and the oracle here is specifically "not refused
/// on permission", not "succeeded".
#[test]
fn a_privileged_session_passes_the_tier_gate() {
    for op in AUTHORIZATION_CHANGE_OPS {
        let (_success, output) = run(op, args_for(op), PermissionTier::Privileged);
        assert!(
            !output.contains("Permission denied"),
            "{op} was still refused at Privileged tier: {output}"
        );
    }
}

/// The ops deliberately left at `Write` must still be callable at `Write` —
/// otherwise the raise silently widened past what decision #6 authorised, and
/// `kb_block_member` (ADR-039 A2 self-protection, explicitly not owner-gated)
/// would be harder to reach than the attack it defends against.
#[test]
fn deliberately_unraised_ops_remain_reachable_at_write_tier() {
    for op in DELIBERATELY_NOT_AUTHORIZATION_CHANGES {
        let (_success, output) = run(op, args_for(op), PermissionTier::Write);
        assert!(
            !output.contains("Permission denied"),
            "{op} was left at Write on purpose but dispatch refused it: {output}"
        );
    }
}

/// The self-escalation path (decision #6's "related, same shape"). `set_option`
/// stays `Write` for ordinary configuration, so the guard has to be
/// argument-sensitive — and it has to hold for both registry spellings, since
/// `OptionRegistry` accepts `ai_tier` and `ai-tier` interchangeably.
#[test]
fn a_write_tier_session_cannot_raise_its_own_tier_through_set_option() {
    // Every spelling and every accepted value of the tier must be refused.
    // `ai-tier` is refused one step earlier — `set_option`'s `option` enum is
    // generated from the registry's canonical names, so the hyphenated alias
    // never reaches the tier check. That is a *second* barrier, not the one
    // under test, so the two are asserted separately rather than with one
    // loose "it failed somehow" oracle.
    for value in ["Privileged", "privileged", "full", "shell"] {
        for persist in [false, true] {
            let (success, output) = run(
                "set_option",
                serde_json::json!({"option": "ai_tier", "value": value, "persist": persist}),
                PermissionTier::Write,
            );
            assert!(
                !success,
                "set_option ai_tier={value} (persist={persist}) SUCCEEDED at Write tier \
                 — self-escalation"
            );
            assert!(
                output.contains("Permission denied") && output.contains("Privileged"),
                "set_option ai_tier={value} (persist={persist}) must be refused on \
                 permission, naming Privileged: {output}"
            );
        }
    }

    // The alias: refused, just not by the tier gate. Pinned so a future change
    // that widens the enum to accept aliases is forced to notice that the
    // permission guard — which does normalize the spelling — is what then has
    // to hold the line.
    let (success, output) = run(
        "set_option",
        serde_json::json!({"option": "ai-tier", "value": "privileged"}),
        PermissionTier::Write,
    );
    assert!(!success, "set_option ai-tier SUCCEEDED at Write tier");
    assert!(
        output.contains("not in"),
        "expected the alias to be rejected by the option enum: {output}"
    );
    assert_eq!(
        crate::tools::effective_tier(
            "set_option",
            &serde_json::json!({"option": "ai-tier", "value": "privileged"}),
            PermissionTier::Write
        ),
        PermissionTier::Privileged,
        "the tier guard must still classify the alias, so widening the enum cannot \
         silently reopen this path"
    );
}

/// ...while ordinary option setting is untouched. Sampled over the real
/// registry rather than one hand-picked option: the failure mode being guarded
/// against is "raise `set_option` wholesale", which would show up here as a
/// blanket denial.
#[test]
fn ordinary_options_are_still_settable_at_write_tier() {
    let registry = OptionRegistry::new();
    let mut checked = 0usize;
    for opt in registry.list() {
        if crate::tools::is_permission_tier_option(&opt.name) {
            continue;
        }
        let (_success, output) = run(
            "set_option",
            serde_json::json!({"option": opt.name, "value": opt.default_value}),
            PermissionTier::Write,
        );
        assert!(
            !output.contains("Permission denied"),
            "setting ordinary option '{}' was refused at Write tier: {output}",
            opt.name
        );
        checked += 1;
    }
    assert!(checked > 100, "sanity: only {checked} options exercised");
}
