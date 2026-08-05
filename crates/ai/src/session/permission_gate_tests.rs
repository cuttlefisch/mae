//! ADR-090 D2/D3 tests for the **embedded session** surface.
//!
//! The session is the one surface that genuinely *implements* `Ask` inside the
//! editor: it parks the turn on an `AiEvent::ConfirmToolCall` the human answers
//! with `:ai-accept`/`:ai-reject`. These tests drive the real gate
//! (`decide_and_present`) rather than a reimplementation of it, and every
//! adversarial case asks the same question — **can an `Ask` end up as
//! execution without a human saying yes?**

use super::handle_prompt::ToolCallGate;
use super::*;
use crate::tools::{Decision, HardCeiling, HardCeilingSource, PermissionPolicy};

/// A minimal provider — the gate under test never calls it, but `AgentSession`
/// requires one.
struct NeverCalled;

#[async_trait::async_trait]
impl AgentProvider for NeverCalled {
    async fn send(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        panic!("the permission gate must decide before any provider call")
    }

    fn name(&self) -> &str {
        "never-called"
    }
}

fn empty_params() -> ToolParameters {
    ToolParameters {
        schema_type: "object".into(),
        properties: Default::default(),
        required: Vec::new(),
    }
}

fn shell_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: String::new(),
        parameters: empty_params(),
        permission: Some(PermissionTier::Shell),
    }
}

/// Build a session with `policy` and a tool registry containing `shell_exec`.
/// Returns the session plus the event receiver a "human" reads prompts from.
fn session_with(policy: PermissionPolicy) -> (AgentSession, mpsc::Receiver<AiEvent>) {
    let (event_tx, event_rx) = mpsc::channel::<AiEvent>(16);
    let (_cmd_tx, cmd_rx) = mpsc::channel::<AiCommand>(4);
    let session = AgentSession::new(
        Box::new(NeverCalled),
        vec![shell_tool("shell_exec")],
        "sys".into(),
        event_tx,
        cmd_rx,
    )
    .with_permission_policy(policy);
    (session, event_rx)
}

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: name.into(),
        arguments: serde_json::json!({"command": "echo hi"}),
    }
}

/// Drive the gate while a "human" answers every prompt with `answer`.
/// Returns `(gate outcome, number of prompts the human saw)`.
async fn gate_with_human(
    policy: PermissionPolicy,
    tool: &str,
    answer: Option<bool>,
) -> (ToolCallGate, usize) {
    let (mut session, mut rx) = session_with(policy);
    let human = tokio::spawn(async move {
        let mut prompts = 0;
        while let Some(evt) = rx.recv().await {
            if let AiEvent::ConfirmToolCall { reply, .. } = evt {
                prompts += 1;
                match answer {
                    Some(a) => {
                        let _ = reply.send(a);
                    }
                    // The adversarial case: the prompt is dropped unanswered
                    // (TUI gone, editor shutting down, task cancelled).
                    None => drop(reply),
                }
            }
        }
        prompts
    });
    let outcome = session.decide_and_present(&call(tool)).await;
    drop(session);
    let prompts = human.await.unwrap();
    (outcome, prompts)
}

/// The positive control: above the ceiling the human IS asked, and a "yes"
/// carries exactly the tier they were shown — not a blanket escalation.
#[tokio::test]
async fn above_the_ceiling_the_human_is_asked_and_yes_carries_the_shown_tier() {
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::ReadOnly,
        ..PermissionPolicy::default()
    };
    let (outcome, prompts) = gate_with_human(policy, "shell_exec", Some(true)).await;
    assert_eq!(prompts, 1, "the human must actually have been asked");
    match outcome {
        ToolCallGate::Proceed(Some(tier)) => assert_eq!(tier, PermissionTier::Shell),
        other => panic!(
            "expected Proceed with the approved tier, got {}",
            describe(&other)
        ),
    }
}

/// A human "no" refuses. Trivial, but it is the other half of the state
/// machine and a gate that ignored the answer would still pass the test above.
#[tokio::test]
async fn a_human_no_refuses() {
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::ReadOnly,
        ..PermissionPolicy::default()
    };
    let (outcome, prompts) = gate_with_human(policy, "shell_exec", Some(false)).await;
    assert_eq!(prompts, 1);
    match outcome {
        ToolCallGate::Refused(msg) => assert!(msg.contains("Denied by user"), "{msg}"),
        other => panic!("a refusal must not proceed: {}", describe(&other)),
    }
}

/// **The attacker's case.** Every way an `Ask` can lose its answer must end in
/// a refusal, never in execution: the prompt dropped unanswered, and the event
/// channel closed before the prompt could even be sent (no UI attached at
/// all). A `Proceed` from either is a silent `Ask`→`Allow` promotion.
#[tokio::test]
async fn an_ask_that_loses_its_answer_never_resolves_to_proceed() {
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::ReadOnly,
        ..PermissionPolicy::default()
    };

    // (a) prompt delivered, then dropped unanswered.
    let (outcome, prompts) = gate_with_human(policy.clone(), "shell_exec", None).await;
    assert_eq!(prompts, 1);
    match outcome {
        ToolCallGate::Refused(msg) => assert!(msg.contains("Permission denied"), "{msg}"),
        other => panic!(
            "a dropped prompt must refuse, not proceed: {}",
            describe(&other)
        ),
    }

    // (b) no listener at all — the event channel is closed before the gate
    // even runs, which is what a headless embed looks like.
    let (event_tx, event_rx) = mpsc::channel::<AiEvent>(4);
    let (_cmd_tx, cmd_rx) = mpsc::channel::<AiCommand>(4);
    drop(event_rx);
    let mut session = AgentSession::new(
        Box::new(NeverCalled),
        vec![shell_tool("shell_exec")],
        "sys".into(),
        event_tx,
        cmd_rx,
    )
    .with_permission_policy(policy);
    match session.decide_and_present(&call("shell_exec")).await {
        ToolCallGate::Refused(msg) => assert!(msg.contains("no human to confirm"), "{msg}"),
        other => panic!(
            "a closed event channel must refuse, not proceed: {}",
            describe(&other)
        ),
    }
}

/// A `Deny` must never reach the human at all. Prompting on a `Deny` would be
/// worse than useless: it trains the human to approve, and offers a route
/// around a hard ceiling that the ADR says is not promptable.
#[tokio::test]
async fn a_hard_ceiling_denial_is_never_shown_to_the_human() {
    let policy = PermissionPolicy::default().with_hard_ceiling(HardCeiling {
        tier: PermissionTier::ReadOnly,
        source: HardCeilingSource::SessionDeclared,
    });
    // Answer "yes" to anything — if a prompt appeared, the gate would proceed
    // and the assertion below would catch it.
    let (outcome, prompts) = gate_with_human(policy, "shell_exec", Some(true)).await;
    assert_eq!(prompts, 0, "a Deny must not produce an approval prompt");
    match outcome {
        ToolCallGate::Refused(msg) => assert!(msg.contains("declared ceiling"), "{msg}"),
        other => panic!(
            "a hard-ceiling denial must refuse even when the human says yes: {}",
            describe(&other)
        ),
    }
}

/// An **untiered** tool is treated as `Privileged`, not as `Write`. A tool the
/// registry has no opinion about is exactly the case where trusting it is
/// unjustified — and it is reachable in practice from a server that predates
/// the `permission` wire field.
#[tokio::test]
async fn an_untiered_tool_is_treated_as_privileged_not_trusted() {
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::Shell,
        ..PermissionPolicy::default()
    };
    let (event_tx, mut rx) = mpsc::channel::<AiEvent>(8);
    let (_cmd_tx, cmd_rx) = mpsc::channel::<AiCommand>(4);
    let mut session = AgentSession::new(
        Box::new(NeverCalled),
        vec![ToolDefinition {
            name: "mystery_tool".into(),
            description: String::new(),
            parameters: empty_params(),
            permission: None,
        }],
        "sys".into(),
        event_tx,
        cmd_rx,
    )
    .with_permission_policy(policy);

    let human = tokio::spawn(async move {
        let mut shown = None;
        while let Some(evt) = rx.recv().await {
            if let AiEvent::ConfirmToolCall { tier, reply, .. } = evt {
                shown = Some(tier);
                let _ = reply.send(false);
            }
        }
        shown
    });
    let outcome = session.decide_and_present(&call("mystery_tool")).await;
    drop(session);
    let shown = human.await.unwrap();

    assert_eq!(
        shown,
        Some(PermissionTier::Privileged),
        "an untiered tool must be shown at the most restrictive tier, not Write"
    );
    assert!(matches!(outcome, ToolCallGate::Refused(_)));
}

/// Under a permissive ceiling nothing is asked — the gate must not turn into a
/// prompt storm for a session that explicitly opted into a high ceiling.
#[tokio::test]
async fn nothing_is_asked_when_the_ceiling_already_covers_the_call() {
    let policy = PermissionPolicy {
        auto_approve_up_to: PermissionTier::Privileged,
        ..PermissionPolicy::default()
    };
    let (outcome, prompts) = gate_with_human(policy, "shell_exec", Some(false)).await;
    assert_eq!(prompts, 0);
    assert!(
        matches!(outcome, ToolCallGate::Proceed(None)),
        "an auto-approved call must proceed with no carried approval"
    );
}

/// The gate's answer agrees with the PDP's, for every ceiling — the session is
/// an enforcement point, not a second decision point. If these ever diverge,
/// the session has grown policy logic of its own.
#[tokio::test]
async fn the_session_gate_agrees_with_the_pdp_at_every_ceiling() {
    for ceil in [
        PermissionTier::ReadOnly,
        PermissionTier::Write,
        PermissionTier::Shell,
        PermissionTier::Privileged,
    ] {
        let policy = PermissionPolicy {
            auto_approve_up_to: ceil,
            ..PermissionPolicy::default()
        };
        let pdp = policy.decide("shell_exec", PermissionTier::Shell);
        let (outcome, prompts) = gate_with_human(policy, "shell_exec", Some(false)).await;
        match pdp {
            Decision::Allow => {
                assert_eq!(prompts, 0, "ceiling={ceil:?}");
                assert!(matches!(outcome, ToolCallGate::Proceed(None)));
            }
            Decision::Ask => {
                assert_eq!(prompts, 1, "ceiling={ceil:?} should have prompted");
                assert!(matches!(outcome, ToolCallGate::Refused(_)));
            }
            Decision::Deny(_) => panic!("ceiling={ceil:?} unexpectedly denied"),
        }
    }
}

fn describe(g: &ToolCallGate) -> String {
    match g {
        ToolCallGate::Proceed(t) => format!("Proceed({t:?})"),
        ToolCallGate::Refused(m) => format!("Refused({m})"),
    }
}

/// The bug this file could not catch, and why it is tested this way.
///
/// Every other test here calls `decide_and_present` directly. That proves the
/// gate DECIDES correctly — and it passed throughout the period when the tool
/// loop never reached the gate at all. `handle_prompt`'s loop opened at ~line
/// 659 and the gate sat at ~1119, with ten `if call.name == ...` intercepts in
/// between, each of which `continue`d first. `shell_exec` (Shell tier),
/// `web_fetch` (Shell) and `ai_set_mode`/`ai_set_profile`/`ai_set_budget` (all
/// Privileged) therefore executed with no permission check whatsoever, from the
/// shipped `readonly` default.
///
/// The defect was positional, so the invariant is positional: within the tool
/// loop, the gate must come before the first name intercept. A behavioural test
/// would need a provider, an event loop and a human answering — and would still
/// only cover whichever tool it happened to name. This covers all ten, and any
/// eleventh someone adds later.
#[test]
fn the_permission_gate_precedes_every_tool_name_intercept() {
    let src = include_str!("handle_prompt.rs");

    let loop_head = src
        .find("for call in &deduplicated_calls {")
        .expect("the tool-execution loop should still exist");
    let body = &src[loop_head..];

    let gate = body
        .find("self.decide_and_present(call)")
        .expect("the tool loop must call decide_and_present");

    // The first per-tool intercept inside the loop.
    let first_intercept = body
        .find("if call.name == \"")
        .expect("the loop should still contain name-based intercepts");

    assert!(
        gate < first_intercept,
        "the permission gate must be the FIRST thing in the tool loop.\n\
         Found decide_and_present at +{gate} but a `if call.name == \"...\"` \
         intercept at +{first_intercept} (offsets from the loop head).\n\
         An intercept placed above the gate `continue`s past it, so that tool \
         executes with no permission check at all — which is exactly how \
         shell_exec became reachable at the readonly default."
    );
}
