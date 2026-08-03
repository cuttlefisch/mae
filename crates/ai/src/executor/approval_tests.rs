//! ADR-090 tests for the **dispatch** enforcement point.
//!
//! `execute_tool_dispatch_body` is where `Ask` becomes visible to every
//! surface, as `ExecuteResult::NeedsApproval`. The properties under test:
//! nothing runs, the request carries what a prompt needs, a real `Deny` is
//! still a `Deny`, and the shared non-interactive mapping cannot produce a
//! success.

use super::*;
use crate::tools::{tools_from_registry, HardCeiling, HardCeilingSource, PermissionPolicy};
use mae_core::Editor;

fn all_tools() -> Vec<ToolDefinition> {
    let mut t = tools_from_registry(&mae_core::CommandRegistry::with_builtins());
    t.extend(crate::tools::ai_specific_tools(
        &mae_core::OptionRegistry::new(),
    ));
    t
}

/// The focused buffer's text — the oracle for "did the write actually
/// happen", which a result-string assertion alone would not catch.
fn buffer_text(editor: &Editor) -> String {
    editor.buffers[0].text()
}

fn call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("call-{name}"),
        name: name.into(),
        arguments: args,
    }
}

fn ceiling(tier: PermissionTier) -> PermissionPolicy {
    PermissionPolicy {
        auto_approve_up_to: tier,
        ..PermissionPolicy::default()
    }
}

/// A write above the ceiling comes back as `NeedsApproval` — not as a denial,
/// and (the part that matters) **without having written anything**. The oracle
/// is the buffer's own contents, not the result string: a dispatch that
/// executed and *then* reported a prompt would pass a message-only assertion.
#[test]
fn a_tool_above_the_ceiling_needs_approval_and_does_not_run() {
    let mut editor = Editor::new();
    let before = buffer_text(&editor);

    let result = execute_tool(
        &mut editor,
        &call(
            "buffer_write",
            serde_json::json!({"start_line": 1, "content": "MUTATED BY AI"}),
        ),
        &all_tools(),
        &ceiling(PermissionTier::ReadOnly),
    );

    match result {
        ExecuteResult::NeedsApproval(req) => {
            assert_eq!(req.tool_name, "buffer_write");
            assert_eq!(req.tier, PermissionTier::Write);
            assert_eq!(req.auto_approve_up_to, PermissionTier::ReadOnly);
            assert_eq!(req.tool_call_id, "call-buffer_write");
        }
        ExecuteResult::Immediate(r) => {
            panic!("expected NeedsApproval, got Immediate: {}", r.output)
        }
        ExecuteResult::Deferred { .. } => panic!("expected NeedsApproval, got Deferred"),
    }

    assert_eq!(
        buffer_text(&editor),
        before,
        "the buffer must be untouched — `Ask` means nothing ran"
    );
}

/// The same call, once approved at exactly the tier the human was shown, runs.
/// Without this the `Ask` state would be indistinguishable from a denial in
/// practice, which is the ADR's whole complaint about the old model.
#[test]
fn the_same_call_runs_once_approved_at_the_shown_tier() {
    let mut editor = Editor::new();
    let policy = ceiling(PermissionTier::ReadOnly);
    let c = call(
        "buffer_write",
        serde_json::json!({"start_line": 1, "content": "approved edit"}),
    );

    let ExecuteResult::NeedsApproval(req) = execute_tool(&mut editor, &c, &all_tools(), &policy)
    else {
        panic!("expected an approval prompt first");
    };

    let approved = policy.with_one_time_approval(req.tier);
    match execute_tool(&mut editor, &c, &all_tools(), &approved) {
        ExecuteResult::Immediate(r) => assert!(r.success, "{}", r.output),
        other => panic!("an approved call must run, got {other:?}"),
    }
    assert!(buffer_text(&editor).contains("approved edit"));
}

/// A hard ceiling still denies at dispatch — it is not converted into an
/// approval prompt on the way through. If it were, every surface with a prompt
/// would become a route around ADR-051.
#[test]
fn a_hard_ceiling_denies_at_dispatch_and_never_asks() {
    let mut editor = Editor::new();
    let policy = ceiling(PermissionTier::Privileged).with_hard_ceiling(HardCeiling {
        tier: PermissionTier::ReadOnly,
        source: HardCeilingSource::SessionDeclared,
    });
    match execute_tool(
        &mut editor,
        &call(
            "buffer_write",
            serde_json::json!({"start_line": 1, "content": "x"}),
        ),
        &all_tools(),
        &policy,
    ) {
        ExecuteResult::Immediate(r) => {
            assert!(!r.success);
            assert!(r.output.contains("declared ceiling"), "{}", r.output);
        }
        ExecuteResult::NeedsApproval(_) => {
            panic!("a session-declared ceiling must DENY, not ask")
        }
        ExecuteResult::Deferred { .. } => panic!("unexpected deferral"),
    }
}

/// A category restriction likewise denies rather than asking, including for an
/// *uncategorized* tool (fail-closed) at a tier the ceiling would have allowed.
#[test]
fn a_category_restriction_denies_at_dispatch_and_never_asks() {
    let mut editor = Editor::new();
    let mut only_knowledge = std::collections::HashSet::new();
    only_knowledge.insert(crate::tools::ToolCategory::Knowledge);
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::Privileged,
        hard_ceiling: None,
        allowed_categories: Some(only_knowledge),
    };
    // Well-formed arguments in every case: schema validation runs before the
    // permission gate, so a malformed call would never reach the decision this
    // test is about.
    for (tool, args) in [
        ("git_status", serde_json::json!({})),
        ("execute_command", serde_json::json!({"command": "save"})),
    ] {
        match execute_tool(&mut editor, &call(tool, args), &all_tools(), &policy) {
            ExecuteResult::Immediate(r) => {
                assert!(!r.success);
                assert!(r.output.contains("Category denied"), "{tool}: {}", r.output);
            }
            ExecuteResult::NeedsApproval(_) => {
                panic!("{tool}: a category restriction must DENY, not ask")
            }
            ExecuteResult::Deferred { .. } => panic!("{tool}: unexpected deferral"),
        }
    }
}

/// The shared non-interactive mapping (ADR-090 D3) can only ever produce a
/// failure. Exhaustive over every tier/ceiling pair that yields an `Ask`, so
/// no combination can slip through as a success.
#[test]
fn the_non_interactive_mapping_can_never_produce_a_success() {
    for tier in [
        PermissionTier::ReadOnly,
        PermissionTier::Write,
        PermissionTier::Shell,
        PermissionTier::Privileged,
    ] {
        for ceil in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            let req = ApprovalRequest {
                tool_call_id: "id".into(),
                tool_name: "some_tool".into(),
                tier,
                auto_approve_up_to: ceil,
            };
            let denied = req.clone().into_denied("a headless surface");
            assert!(
                !denied.success,
                "tier={tier:?} ceiling={ceil:?} produced a SUCCESS"
            );
            assert_eq!(denied.tool_call_id, "id");
            assert_eq!(denied.tool_name, "some_tool");
            assert!(denied.output.contains("no human to confirm"));
            assert!(denied.output.contains("a headless surface"));
            // The prompt line an interactive surface would show names the same
            // tier — the two presentations must not diverge.
            assert!(req.prompt_line().contains(&format!("{tier:?}")));
        }
    }
}

/// Under the shipped default, the build/test tools reach an approval prompt
/// end-to-end through real dispatch — not just through the PDP in isolation.
/// This is the regression that would otherwise push operators to
/// `auto_approve_tier = "shell"`.
#[test]
fn run_build_and_run_test_reach_an_approval_prompt_through_real_dispatch() {
    let tools = all_tools();
    for tool in ["run_build", "run_test"] {
        assert!(
            tools.iter().any(|t| t.name == tool),
            "{tool} must exist in the registry for this test to mean anything"
        );
        let mut editor = Editor::new();
        match execute_tool(
            &mut editor,
            &call(tool, serde_json::json!({})),
            &tools,
            &PermissionPolicy::default(),
        ) {
            ExecuteResult::NeedsApproval(req) => {
                assert_eq!(req.tool_name, tool);
                assert!(
                    req.tier > PermissionTier::ReadOnly,
                    "{tool} should be above the default ceiling"
                );
            }
            ExecuteResult::Immediate(r) => panic!(
                "{tool} must ASK under the shipped default, not resolve immediately: {}",
                r.output
            ),
            ExecuteResult::Deferred { .. } => panic!("{tool} unexpectedly deferred"),
        }
    }
}

/// ...and ordinary reads still dispatch with no prompt, so the lowered default
/// is not a prompt storm on navigation.
#[test]
fn reads_still_dispatch_without_a_prompt_under_the_shipped_default() {
    let mut editor = Editor::new();
    match execute_tool(
        &mut editor,
        &call("editor_state", serde_json::json!({})),
        &all_tools(),
        &PermissionPolicy::default(),
    ) {
        ExecuteResult::Immediate(r) => assert!(r.success, "{}", r.output),
        other => panic!("a read must not prompt, got {other:?}"),
    }
}

/// The sharp edge that bit `request_tools` when the default dropped: a tool
/// **absent** from `all_tools` has no declared tier, so dispatch falls back to
/// `Write` — which under the shipped default is askable, not automatic.
///
/// That fallback is correct (an unknown tool is not a trusted one), but it
/// means any tool that is *dispatchable by name* must also be *present in the
/// dispatch list*, or its own declared tier is silently ignored. `request_tools`
/// declares `ReadOnly` and was only ever appended to the **advertised** list;
/// pure discovery consequently started requiring approval. Asserted here so the
/// fallback's blast radius is written down rather than rediscovered.
#[test]
fn a_tool_missing_from_the_dispatch_list_falls_back_to_write_not_to_its_own_tier() {
    let mut editor = Editor::new();
    let tools_without_it: Vec<ToolDefinition> = all_tools()
        .into_iter()
        .filter(|t| t.name != "search_tools")
        .collect();
    assert_eq!(
        crate::tools::request_tools_definition().permission,
        Some(PermissionTier::ReadOnly),
        "precondition: the discovery meta-tools declare ReadOnly"
    );
    match execute_tool(
        &mut editor,
        &call("search_tools", serde_json::json!({"query": "buffer"})),
        &tools_without_it,
        &PermissionPolicy::default(),
    ) {
        ExecuteResult::NeedsApproval(req) => assert_eq!(
            req.tier,
            PermissionTier::Write,
            "an absent tool takes the Write fallback, NOT its own declared tier"
        ),
        other => panic!("expected the Write fallback to ask, got {other:?}"),
    }
    // ...and with the tool present, its own `ReadOnly` is honoured and no
    // approval is needed. This is the pair that makes the assertion above a
    // statement about the *list*, not about `search_tools`.
    match execute_tool(
        &mut editor,
        &call("search_tools", serde_json::json!({"query": "buffer"})),
        &all_tools(),
        &PermissionPolicy::default(),
    ) {
        ExecuteResult::Immediate(r) => assert!(r.success, "{}", r.output),
        other => panic!("a listed ReadOnly discovery tool must not prompt, got {other:?}"),
    }
}
