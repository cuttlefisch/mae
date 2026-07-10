//! The harness's own client-side tool-calling turn loop (ADR-046).
//!
//! `crates/ai/src/session/handle_prompt.rs`'s loop is tightly coupled to
//! `Editor`/`AiEvent`/`AiCommand` channels and can't be reused by an external
//! process — this reimplements the same *shape*: send → tool_calls → call each
//! via MCP → append results → repeat until `EndTurn` or a round cap. Keep this
//! loop's shape behaviorally compatible with `handle_prompt.rs`'s if that ever
//! changes; there is no compiler-enforced link between the two, only this note.

use anyhow::Result;
use mae_ai::{
    AgentProvider, Message, MessageContent, Role, StopReason, ToolCall, ToolDefinition, ToolResult,
    Usage,
};

use crate::mcp_client::ToolExecutor;

/// Emitted as the turn progresses; the TUI layer consumes these to render the
/// transcript live rather than waiting for the whole turn to finish.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    ToolCallStarted {
        name: String,
        arguments: serde_json::Value,
    },
    ToolCallFinished {
        name: String,
        success: bool,
        output: String,
    },
    /// Assistant text produced this round (may arrive alongside tool calls —
    /// reasoning models commonly do both in one turn).
    Text(String),
    RoundLimitReached,
    /// Emitted once per round, before any tool calls execute -- what the
    /// provider actually reported for this round, independent of what the
    /// harness does with it. This is the introspection signal needed to
    /// distinguish "the model correctly decided it's done" from "the model
    /// went silent because the tool list overwhelmed it" (confirmed
    /// empirically: MAE's real ~730-tool surface reliably causes a real
    /// tool-use-tuned 8B Ollama model to return zero tool calls and a refusal
    /// text, while the same model tool-calls correctly the moment the
    /// offered set drops to 1-2 -- see ADR-045). Without this, both look
    /// identical from the outside.
    RoundDiagnostics {
        round: usize,
        tools_offered: usize,
        stop_reason: StopReason,
        tool_calls_returned: usize,
        text_len: usize,
        usage: Option<Usage>,
    },
}

pub struct TurnConfig {
    pub max_rounds: usize,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self { max_rounds: 50 }
    }
}

/// Session-scoped parts of a turn that don't change call to call — grouped
/// mainly to keep `run_turn`'s parameter count sane.
pub struct TurnContext<'a> {
    pub provider: &'a dyn AgentProvider,
    pub executor: &'a mut dyn ToolExecutor,
    pub tools: &'a [ToolDefinition],
    pub system_prompt: &'a str,
}

/// Run one full user turn: push `user_input`, loop provider↔tools until the
/// model stops asking for tools or the round cap is hit. `messages` is the
/// conversation history, mutated in place so the caller keeps it across turns.
pub async fn run_turn(
    ctx: TurnContext<'_>,
    messages: &mut Vec<Message>,
    config: &TurnConfig,
    user_input: &str,
    mut on_event: impl FnMut(AgentEvent),
) -> Result<()> {
    let TurnContext {
        provider,
        executor,
        tools,
        system_prompt,
    } = ctx;

    messages.push(Message {
        role: Role::User,
        content: MessageContent::Text(user_input.to_string()),
    });

    let mut round = 0;
    loop {
        if round >= config.max_rounds {
            on_event(AgentEvent::RoundLimitReached);
            return Ok(());
        }

        let response = provider
            .send(messages, tools, system_prompt)
            .await
            .map_err(|e| anyhow::anyhow!(e.message))?;

        on_event(AgentEvent::RoundDiagnostics {
            round,
            tools_offered: tools.len(),
            stop_reason: response.stop_reason.clone(),
            tool_calls_returned: response.tool_calls.len(),
            text_len: response.text.as_deref().map(str::len).unwrap_or(0),
            usage: response.usage,
        });

        let text = response.text.clone().filter(|t| !t.is_empty());
        if let Some(t) = &text {
            on_event(AgentEvent::Text(t.clone()));
        }

        if response.tool_calls.is_empty() || matches!(response.stop_reason, StopReason::EndTurn) {
            messages.push(Message {
                role: Role::Assistant,
                content: MessageContent::Text(text.unwrap_or_default()),
            });
            return Ok(());
        }

        messages.push(Message {
            role: Role::Assistant,
            content: match text {
                Some(text) => MessageContent::TextWithToolCalls {
                    text,
                    tool_calls: response.tool_calls.clone(),
                },
                None => MessageContent::ToolCalls(response.tool_calls.clone()),
            },
        });

        for call in &response.tool_calls {
            on_event(AgentEvent::ToolCallStarted {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            let (success, output) = execute_one(executor, call).await;
            on_event(AgentEvent::ToolCallFinished {
                name: call.name.clone(),
                success,
                output: output.clone(),
            });
            messages.push(Message {
                role: Role::Tool,
                content: MessageContent::ToolResult(ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success,
                    output,
                }),
            });
        }

        round += 1;
    }
}

async fn execute_one(executor: &mut dyn ToolExecutor, call: &ToolCall) -> (bool, String) {
    match executor.call_tool(&call.name, call.arguments.clone()).await {
        Ok(outcome) => (outcome.success, outcome.text),
        Err(e) => (false, format!("MCP tool call failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_client::ToolCallOutcome;
    use async_trait::async_trait;
    use mae_ai::{ProviderError, ProviderResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A scripted provider: returns a fixed sequence of responses, one per
    /// `send()` call, so the loop's round-by-round behavior is deterministic.
    struct ScriptedProvider {
        responses: Mutex<Vec<ProviderResponse>>,
    }

    #[async_trait::async_trait]
    impl AgentProvider for ScriptedProvider {
        async fn send(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system_prompt: &str,
        ) -> Result<ProviderResponse, ProviderError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(ProviderError {
                    message: "scripted provider exhausted".into(),
                    retryable: false,
                    kind: mae_ai::ErrorKind::Unknown,
                });
            }
            Ok(responses.remove(0))
        }
        fn name(&self) -> &str {
            "scripted"
        }
    }

    /// Records every tool call it receives; always succeeds with a canned output.
    struct RecordingExecutor {
        calls: Vec<(String, serde_json::Value)>,
    }

    #[async_trait]
    impl ToolExecutor for RecordingExecutor {
        async fn call_tool(
            &mut self,
            name: &str,
            arguments: serde_json::Value,
        ) -> Result<ToolCallOutcome> {
            self.calls.push((name.to_string(), arguments));
            Ok(ToolCallOutcome {
                success: true,
                text: format!("result for {name}"),
            })
        }
    }

    fn text_response(text: &str) -> ProviderResponse {
        ProviderResponse {
            text: Some(text.to_string()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        }
    }

    fn tool_call_response(name: &str, args: serde_json::Value) -> ProviderResponse {
        ProviderResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: name.to_string(),
                arguments: args,
            }],
            stop_reason: StopReason::ToolUse,
            usage: None,
        }
    }

    #[tokio::test]
    async fn single_text_turn_ends_immediately() {
        let provider = ScriptedProvider {
            responses: Mutex::new(vec![text_response("hello")]),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "hi",
            |e| events.push(e),
        )
        .await
        .unwrap();

        assert!(executor.calls.is_empty(), "no tool call expected");
        // user + assistant
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::User));
        assert!(matches!(messages[1].role, Role::Assistant));
        let text_events: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Text(_)))
            .collect();
        assert!(matches!(
            text_events.as_slice(),
            [AgentEvent::Text(t)] if t == "hello"
        ));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::RoundDiagnostics { .. })),
            "expected a RoundDiagnostics event alongside the text"
        );
    }

    #[tokio::test]
    async fn round_diagnostics_reports_tools_offered_and_stop_reason() {
        let tool_def = ToolDefinition {
            name: "kb_search".to_string(),
            description: "search".to_string(),
            parameters: mae_ai::ToolParameters {
                schema_type: "object".to_string(),
                properties: Default::default(),
                required: vec![],
            },
            permission: None,
        };
        let provider = ScriptedProvider {
            responses: Mutex::new(vec![text_response("hi there")]),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: std::slice::from_ref(&tool_def),
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "hi",
            |e| events.push(e),
        )
        .await
        .unwrap();

        let diag = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::RoundDiagnostics {
                    tools_offered,
                    stop_reason,
                    tool_calls_returned,
                    text_len,
                    ..
                } => Some((
                    *tools_offered,
                    stop_reason.clone(),
                    *tool_calls_returned,
                    *text_len,
                )),
                _ => None,
            })
            .expect("expected a RoundDiagnostics event");
        assert_eq!(diag.0, 1, "tools_offered should reflect the tools slice");
        assert_eq!(diag.1, StopReason::EndTurn);
        assert_eq!(diag.2, 0);
        assert_eq!(diag.3, "hi there".len());
    }

    #[tokio::test]
    async fn tool_call_then_final_text_round_trips_correctly() {
        let provider = ScriptedProvider {
            responses: Mutex::new(vec![
                tool_call_response("kb_search", serde_json::json!({"query": "x"})),
                text_response("done"),
            ]),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "search for x",
            |e| events.push(e),
        )
        .await
        .unwrap();

        assert_eq!(executor.calls.len(), 1);
        assert_eq!(executor.calls[0].0, "kb_search");
        // user, assistant(tool_calls), tool(result), assistant(final text)
        assert_eq!(messages.len(), 4);
        assert!(matches!(
            &messages[2].content,
            MessageContent::ToolResult(r) if r.success && r.output == "result for kb_search"
        ));

        let started = events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallStarted { name, .. } if name == "kb_search"));
        let finished = events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCallFinished { name, success: true, .. } if name == "kb_search"));
        assert!(started && finished);
    }

    #[tokio::test]
    async fn round_limit_is_enforced() {
        // Every response asks for another tool call — never EndTurn — so this
        // must stop at the round cap, not loop forever.
        let responses: Vec<ProviderResponse> = (0..10)
            .map(|_| tool_call_response("kb_search", serde_json::json!({})))
            .collect();
        let provider = ScriptedProvider {
            responses: Mutex::new(responses),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();
        let call_count = AtomicUsize::new(0);

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig { max_rounds: 3 },
            "loop forever",
            |e| {
                if matches!(e, AgentEvent::ToolCallStarted { .. }) {
                    call_count.fetch_add(1, Ordering::SeqCst);
                }
                events.push(e);
            },
        )
        .await
        .unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 3);
        assert!(matches!(events.last(), Some(AgentEvent::RoundLimitReached)));
    }

    #[tokio::test]
    async fn failed_tool_call_is_reported_not_panicked() {
        struct FailingExecutor;
        #[async_trait]
        impl ToolExecutor for FailingExecutor {
            async fn call_tool(
                &mut self,
                _name: &str,
                _arguments: serde_json::Value,
            ) -> Result<ToolCallOutcome> {
                anyhow::bail!("socket dropped")
            }
        }

        let provider = ScriptedProvider {
            responses: Mutex::new(vec![
                tool_call_response("kb_get", serde_json::json!({"id": "x"})),
                text_response("recovered"),
            ]),
        };
        let mut executor = FailingExecutor;
        let mut messages = Vec::new();
        let mut events = Vec::new();

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "get x",
            |e| events.push(e),
        )
        .await
        .unwrap();

        assert!(matches!(
            &messages[2].content,
            MessageContent::ToolResult(r) if !r.success && r.output.contains("socket dropped")
        ));
    }

    #[tokio::test]
    async fn provider_error_mid_turn_leaves_dangling_tool_message() {
        // Round 0 gets a real tool-call response (so a Tool message gets
        // appended to history). Round 1 (not round 0) hits
        // `ScriptedProvider`'s own "responses exhausted" branch, which
        // returns `Err(ProviderError { .. })` -- standing in for a real
        // provider erroring out partway through a multi-round turn.
        let provider = ScriptedProvider {
            responses: Mutex::new(vec![tool_call_response(
                "kb_search",
                serde_json::json!({"query": "x"}),
            )]),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();

        let result = run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "search for x",
            |e| events.push(e),
        )
        .await;

        assert!(result.is_err(), "provider error must propagate as Err");
        assert_eq!(executor.calls.len(), 1, "round 0's tool call did execute");

        // Traced from `run_turn`: push(User) -> round 0 send() succeeds ->
        // push(Assistant, ToolCalls) -> execute_one -> push(Tool, ToolResult)
        // -> round 1's `provider.send(...).await.map_err(...)?` errors
        // *before* anything else is pushed. So `messages` ends up exactly
        // [User, Assistant(ToolCalls), Tool(ToolResult)] -- 3 entries, last
        // one a dangling `Role::Tool` message with no matching final
        // `Assistant` reply.
        //
        // This is NOT a corrupted/unsafe-to-resume state, and it's worth
        // saying so plainly rather than glossing over it: it's the exact
        // same shape as the normal boundary between any two rounds mid-turn
        // (tool results appended, next `provider.send()` not yet issued). A
        // caller that retries `run_turn` by re-invoking it with this same
        // `messages` Vec resumes cleanly -- the next `provider.send()` call
        // simply receives the pending tool result and continues the turn.
        assert_eq!(messages.len(), 3);
        assert!(matches!(messages[0].role, Role::User));
        assert!(matches!(messages[1].role, Role::Assistant));
        assert!(matches!(messages[2].role, Role::Tool));
    }

    fn text_and_tool_call_response(
        text: &str,
        name: &str,
        args: serde_json::Value,
    ) -> ProviderResponse {
        ProviderResponse {
            text: Some(text.to_string()),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: name.to_string(),
                arguments: args,
            }],
            stop_reason: StopReason::ToolUse,
            usage: None,
        }
    }

    #[tokio::test]
    async fn text_and_tool_calls_in_same_round_both_fire() {
        // Reasoning models commonly emit reasoning text *and* a tool call in
        // the same response -- `AgentEvent::Text`'s own doc comment calls
        // this out. The loop must not drop one in favor of the other.
        let provider = ScriptedProvider {
            responses: Mutex::new(vec![
                text_and_tool_call_response(
                    "some reasoning",
                    "kb_search",
                    serde_json::json!({"query": "x"}),
                ),
                text_response("done"),
            ]),
        };
        let mut executor = RecordingExecutor { calls: vec![] };
        let mut messages = Vec::new();
        let mut events = Vec::new();

        run_turn(
            TurnContext {
                provider: &provider,
                executor: &mut executor,
                tools: &[],
                system_prompt: "system",
            },
            &mut messages,
            &TurnConfig::default(),
            "search for x",
            |e| events.push(e),
        )
        .await
        .unwrap();

        // The reasoning text fired as its own event...
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::Text(t) if t == "some reasoning")));
        // ...AND the tool call actually executed -- neither suppressed the other.
        assert_eq!(executor.calls.len(), 1);
        assert_eq!(executor.calls[0].0, "kb_search");
    }
}
