//! End-to-end tests for decision #9 / ADR-091: the six session-scoped tools
//! are reachable over MCP, they act on the *right* session, and the three
//! inherently-interactive ones are absent from every external discovery
//! surface.
//!
//! The oracle throughout is editor state after the call, not the returned
//! string. "It returned success" would have been satisfied by the old
//! behaviour for several of these, and would not distinguish "affected this
//! session" from "affected some session".

use mae_core::{CommandRegistry, Editor, OptionRegistry};

use crate::executor::{execute_tool_with_requester, ExecuteResult};
use crate::tools::{
    ai_specific_tools, tools_from_registry, PermissionPolicy, EMBEDDED_SESSION_ONLY_TOOLS,
    SESSION_SCOPED_DISPATCHABLE_TOOLS,
};
use crate::types::{PermissionTier, ToolCall, ToolDefinition, ToolResult};

fn all_tools() -> Vec<ToolDefinition> {
    let mut tools = tools_from_registry(&CommandRegistry::with_builtins());
    tools.extend(ai_specific_tools(&OptionRegistry::new()));
    tools
}

fn privileged() -> PermissionPolicy {
    PermissionPolicy {
        auto_approve_up_to: PermissionTier::Privileged,
        ..PermissionPolicy::default()
    }
}

/// Dispatch as an external MCP client (`session_id = Some(sid)`) or as the
/// embedded agent (`None`) — the one discriminator that distinguishes them.
fn call_as(
    editor: &mut Editor,
    session_id: Option<u64>,
    name: &str,
    args: serde_json::Value,
) -> ToolResult {
    let call = ToolCall {
        id: "t".into(),
        name: name.into(),
        arguments: args,
    };
    match execute_tool_with_requester(editor, &call, &all_tools(), &privileged(), None, session_id)
    {
        ExecuteResult::Immediate(r) => r,
        ExecuteResult::Deferred { .. } => panic!("{name} unexpectedly deferred"),
        ExecuteResult::NeedsApproval(_) => {
            panic!("{name} needed approval under a Privileged policy")
        }
    }
}

/// Arguments that make each of the six a well-formed call. `web_fetch` uses a
/// deliberately invalid scheme so the test never touches the network: the
/// point here is that the call is *routed*, and a routed `web_fetch` answers
/// with the scheme rejection, while an unrouted one answers `Unknown tool`.
fn args_for(name: &str) -> serde_json::Value {
    match name {
        "ai_set_mode" => serde_json::json!({"mode": "plan"}),
        "ai_set_profile" => serde_json::json!({"profile": "explorer"}),
        "ai_set_budget" => serde_json::json!({"warn": 1.5, "cap": 9.0}),
        "log_activity" => serde_json::json!({"activity": "reticulating splines"}),
        "read_transcript" => serde_json::json!({}),
        "web_fetch" => serde_json::json!({"url": "gopher://example.com"}),
        _ => serde_json::json!({}),
    }
}

/// The regression the whole task exists for: none of the six may answer
/// `Unknown tool` to an external MCP client. Asserted for BOTH an external
/// session and the embedded (`None`) path, since the routing is shared.
#[test]
fn none_of_the_six_answer_unknown_tool_over_mcp() {
    for session_id in [Some(7u64), None] {
        for name in SESSION_SCOPED_DISPATCHABLE_TOOLS {
            let mut editor = Editor::new();
            let result = call_as(&mut editor, session_id, name, args_for(name));
            assert!(
                !result.output.contains("Unknown tool"),
                "{name} (session {session_id:?}) is still unroutable: {}",
                result.output
            );
        }
    }
}

/// Session isolation — the property that makes the handle a *session* handle
/// and not a global. Three concurrent sessions, not two, so an
/// off-by-one/last-writer-wins bug cannot hide (principle #14's N-way rule).
#[test]
fn three_concurrent_sessions_keep_independent_agent_state() {
    let mut editor = Editor::new();
    let cases = [
        (11u64, "plan", "explorer", 1.0, 2.0, "alpha step"),
        (22u64, "auto-accept", "reviewer", 3.0, 4.0, "beta step"),
        (33u64, "standard", "planner", 5.0, 6.0, "gamma step"),
    ];

    // Interleave rather than run each session to completion: a bug where one
    // session's write lands on another's record only shows up under
    // interleaving.
    for (sid, mode, _, _, _, _) in &cases {
        call_as(
            &mut editor,
            Some(*sid),
            "ai_set_mode",
            serde_json::json!({ "mode": mode }),
        );
    }
    for (sid, _, profile, _, _, _) in &cases {
        call_as(
            &mut editor,
            Some(*sid),
            "ai_set_profile",
            serde_json::json!({ "profile": profile }),
        );
    }
    for (sid, _, _, warn, cap, _) in &cases {
        call_as(
            &mut editor,
            Some(*sid),
            "ai_set_budget",
            serde_json::json!({ "warn": warn, "cap": cap }),
        );
    }
    for (sid, _, _, _, _, activity) in &cases {
        call_as(
            &mut editor,
            Some(*sid),
            "log_activity",
            serde_json::json!({ "activity": activity }),
        );
    }

    for (sid, mode, profile, warn, cap, activity) in &cases {
        let state = &editor
            .ai
            .mcp_sessions
            .get(sid)
            .unwrap_or_else(|| panic!("session {sid} has no record"))
            .agent;
        assert_eq!(state.mode.as_deref(), Some(*mode), "session {sid} mode");
        assert_eq!(
            state.profile.as_deref(),
            Some(*profile),
            "session {sid} profile"
        );
        assert_eq!(state.budget_warn_usd, Some(*warn), "session {sid} warn");
        assert_eq!(state.budget_cap_usd, Some(*cap), "session {sid} cap");
        assert_eq!(
            state.activity,
            vec![activity.to_string()],
            "session {sid} activity"
        );
    }

    // ...and none of it leaked into the process-wide record, which belongs to
    // the embedded agent.
    assert_eq!(editor.ai.agent_session, Default::default());
    assert_eq!(
        editor.ai.mode, "standard",
        "a per-session mode must not move the editor-global option"
    );
}

/// The embedded path is the *other* half of "the right session": with no MCP
/// session in scope the handle is the editor's own, so the global option must
/// move — matching what `AiEvent::UpdateMode` does when the embedded session
/// runs the same tool. Without this, an MCP `ai_set_mode` would be a strictly
/// weaker version of the embedded one.
#[test]
fn the_embedded_path_writes_through_to_the_editor_options() {
    let mut editor = Editor::new();
    assert_eq!(editor.ai.mode, "standard");
    assert_eq!(editor.ai.profile, "pair-programmer");

    let r = call_as(
        &mut editor,
        None,
        "ai_set_mode",
        serde_json::json!({"mode": "plan"}),
    );
    assert!(r.success, "{}", r.output);
    assert_eq!(editor.ai.mode, "plan");

    let r = call_as(
        &mut editor,
        None,
        "ai_set_profile",
        serde_json::json!({"profile": "reviewer"}),
    );
    assert!(r.success, "{}", r.output);
    assert_eq!(editor.ai.profile, "reviewer");
}

/// Values the editor would reject must be rejected per-session too, or a
/// session-scoped mode could be one no prompt exists for. Attacker-shaped:
/// the rejections are the assertion, and each must leave the record unchanged.
#[test]
fn invalid_modes_and_profiles_are_refused_on_both_paths() {
    for session_id in [Some(5u64), None] {
        for bad_mode in ["", "PLAN", "yolo", "standard ", "plan;rm -rf /"] {
            let mut editor = Editor::new();
            let r = call_as(
                &mut editor,
                session_id,
                "ai_set_mode",
                serde_json::json!({ "mode": bad_mode }),
            );
            assert!(
                !r.success,
                "mode {bad_mode:?} was accepted (session {session_id:?})"
            );
            assert_eq!(
                editor.agent_session().mode,
                None,
                "refused mode {bad_mode:?} still mutated the record"
            );
            assert_eq!(editor.ai.mode, "standard");
        }
        for bad_profile in ["", "PAIR-PROGRAMMER", "not-a-profile"] {
            let mut editor = Editor::new();
            let r = call_as(
                &mut editor,
                session_id,
                "ai_set_profile",
                serde_json::json!({ "profile": bad_profile }),
            );
            assert!(
                !r.success,
                "profile {bad_profile:?} was accepted (session {session_id:?})"
            );
            assert_eq!(editor.ai.profile, "pair-programmer");
        }
    }
}

/// `ai_set_budget`'s "0 clears the threshold" semantics, matching
/// `handle_prompt.rs` exactly — a divergence here would mean the same tool
/// behaves differently depending on which agent called it.
#[test]
fn budget_zero_and_negative_clear_rather_than_cap_at_zero() {
    let mut editor = Editor::new();
    call_as(
        &mut editor,
        Some(1),
        "ai_set_budget",
        serde_json::json!({"warn": 2.0, "cap": 5.0}),
    );
    assert_eq!(editor.ai.mcp_sessions[&1].agent.budget_cap_usd, Some(5.0));

    for clearing in [0.0, -1.0] {
        call_as(
            &mut editor,
            Some(1),
            "ai_set_budget",
            serde_json::json!({"warn": clearing, "cap": clearing}),
        );
        let agent = &editor.ai.mcp_sessions[&1].agent;
        assert_eq!(agent.budget_warn_usd, None, "warn={clearing}");
        assert_eq!(agent.budget_cap_usd, None, "cap={clearing}");
    }

    // Neither argument is a refusal, not a silent no-op.
    let r = call_as(&mut editor, Some(1), "ai_set_budget", serde_json::json!({}));
    assert!(!r.success, "empty ai_set_budget should be refused");
}

/// The activity log is bounded — an agent that narrates every round on a
/// long-lived headless instance (ADR-055) must not grow the record without
/// limit.
#[test]
fn the_activity_log_is_bounded_and_keeps_the_most_recent() {
    let cap = mae_core::editor::ai_state::MAX_SESSION_ACTIVITY_ENTRIES;
    let mut editor = Editor::new();
    for i in 0..(cap + 25) {
        call_as(
            &mut editor,
            Some(3),
            "log_activity",
            serde_json::json!({ "activity": format!("step {i}") }),
        );
    }
    let activity = &editor.ai.mcp_sessions[&3].agent.activity;
    assert_eq!(activity.len(), cap);
    assert_eq!(activity.last().unwrap(), &format!("step {}", cap + 24));
    assert_eq!(activity.first().unwrap(), &format!("step {}", 25));
}

/// `read_transcript` must say *why* there is nothing to read, not report a
/// bare success with empty output — ADR-086's refusal contract.
#[test]
fn read_transcript_refuses_informatively_when_none_is_recorded() {
    let mut editor = Editor::new();
    let r = call_as(
        &mut editor,
        Some(9),
        "read_transcript",
        serde_json::json!({}),
    );
    assert!(!r.success, "expected a refusal, got: {}", r.output);
    assert!(
        r.output.contains("No transcript"),
        "unhelpful refusal: {}",
        r.output
    );

    // With a transcript recorded for the session, the same call returns it.
    let dir = std::env::temp_dir().join(format!("mae-adr091-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("transcript.json");
    std::fs::write(&path, "{\"type\":\"metadata\"}\n").unwrap();
    editor
        .ai
        .mcp_sessions
        .entry(9)
        .or_default()
        .agent
        .transcript_path = Some(path);
    let r = call_as(
        &mut editor,
        Some(9),
        "read_transcript",
        serde_json::json!({}),
    );
    assert!(r.success, "{}", r.output);
    assert!(r.output.contains("metadata"), "{}", r.output);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `web_fetch`'s scheme allow-list must hold on the dispatch path too — the
/// blocking transport is new code, and a permissive scheme check there would
/// be a local-file read reachable from any Shell-tier MCP client. No network
/// is touched: every URL here is rejected before a request is made.
#[test]
fn web_fetch_rejects_non_http_schemes_on_the_dispatch_path() {
    for bad in [
        "file:///etc/passwd",
        "ftp://example.com",
        "data:text/html,x",
        "javascript:alert(1)",
        "",
    ] {
        let mut editor = Editor::new();
        let r = call_as(
            &mut editor,
            Some(1),
            "web_fetch",
            serde_json::json!({ "url": bad }),
        );
        assert!(!r.success, "web_fetch accepted {bad:?}");
        assert!(
            !r.output.contains("Unknown tool"),
            "web_fetch is unrouted, not validating: {}",
            r.output
        );
    }
}

/// Discovery, external side: neither `search_tools` nor `request_tools` may
/// surface the interactive three to an external MCP client.
#[test]
fn external_discovery_surfaces_never_mention_the_interactive_three() {
    let mut editor = Editor::new();
    for name in EMBEDDED_SESSION_ONLY_TOOLS {
        let searched = call_as(
            &mut editor,
            Some(1),
            "search_tools",
            serde_json::json!({ "query": *name, "limit": 50 }),
        );
        assert!(
            !searched.output.contains(&format!("\"name\": \"{name}\"")),
            "search_tools leaked {name} to an external session:\n{}",
            searched.output
        );
        // By exact name — the most direct request an external client can
        // make, and the one a discovery filter is most likely to miss.
        let requested = call_as(
            &mut editor,
            Some(1),
            "request_tools",
            serde_json::json!({ "categories": "ai", "tools": *name }),
        );
        assert!(
            !requested.output.contains(&format!("\"name\": \"{name}\"")),
            "request_tools leaked {name} to an external session:\n{}",
            requested.output
        );
    }
}

/// Discovery, embedded side: the same filter must NOT apply to MAE's own
/// agent, which is the one context where these tools work. A filter applied
/// unconditionally would silently disable `ask_user` for the human's agent —
/// a regression dressed as a fix.
#[test]
fn the_embedded_agent_can_still_discover_the_interactive_three() {
    let mut editor = Editor::new();
    for name in EMBEDDED_SESSION_ONLY_TOOLS {
        let requested = call_as(
            &mut editor,
            None,
            "request_tools",
            serde_json::json!({ "categories": "", "tools": *name }),
        );
        assert!(
            requested.output.contains(&format!("\"name\": \"{name}\"")),
            "the embedded agent lost access to {name}:\n{}",
            requested.output
        );
    }
}

/// A nested dispatch must restore the OUTER session's id, not clear it —
/// otherwise a tool that dispatches another tool would silently redirect the
/// rest of the outer session's work at the process-wide fallback record.
#[test]
fn a_nested_dispatch_scope_restores_the_outer_session() {
    let mut editor = Editor::new();
    editor.with_ai_dispatch_scope_for_session(Some(42), |editor| {
        assert_eq!(editor.dispatch_session_id(), Some(42));
        editor.with_ai_dispatch_scope_for_session(Some(43), |editor| {
            assert_eq!(editor.dispatch_session_id(), Some(43));
        });
        assert_eq!(
            editor.dispatch_session_id(),
            Some(42),
            "inner scope clobbered the outer session id"
        );
        // A nested *embedded* dispatch must also restore, not leave `None`.
        editor.with_ai_dispatch_scope(|editor| {
            assert_eq!(editor.dispatch_session_id(), None);
        });
        assert_eq!(editor.dispatch_session_id(), Some(42));
    });
    assert_eq!(
        editor.dispatch_session_id(),
        None,
        "the session id outlived its dispatch"
    );
}
