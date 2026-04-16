use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::provider::*;
use crate::types::*;

/// AgentSession runs the agentic loop on a spawned tokio task:
///   1. Receive user prompt via channel
///   2. Call provider with conversation history + tools
///   3. For each tool call: send to main thread, await result via oneshot
///   4. Feed tool results back to provider
///   5. Repeat until EndTurn or max rounds
///
/// The session never touches Editor directly — all mutations flow through
/// the main thread's event loop via AiEvent/ToolResult channels.
///
/// Emacs lesson: process.c conflates I/O, lifecycle, and buffering in 7k lines.
/// We separate transport (provider), protocol (types), and orchestration (session).
pub struct AgentSession {
    provider: Box<dyn AgentProvider>,
    tools: Vec<ToolDefinition>,
    messages: Vec<Message>,
    system_prompt: String,
    event_tx: mpsc::Sender<AiEvent>,
    command_rx: mpsc::Receiver<AiCommand>,
    max_rounds: usize,
    /// Maximum messages to keep in conversation history.
    /// Older messages are trimmed (keeping the first user message for context).
    max_messages: usize,
    /// Consecutive provider error count for circuit breaker.
    consecutive_errors: usize,
    /// Cached pricing for the session's model. Resolved once in
    /// `with_budget` so every `update_cost` skips the pricing-table scan
    /// and the `to_ascii_lowercase()` allocation. `None` for unpriced
    /// models (e.g. Ollama) — the tracker treats that as free.
    price: Option<crate::pricing::ModelPrice>,
    /// Per-session cost guardrails.
    budget: crate::BudgetConfig,
    /// Cumulative USD cost for this session. Zero-initialized on
    /// construction; incremented after every successful round.
    session_cost_usd: f64,
    /// Cumulative token counters, forwarded to the editor on every
    /// round so the status line can surface both "dollars" and "tokens"
    /// (the latter matters for Ollama/unpriced models).
    session_tokens_in: u64,
    session_tokens_out: u64,
    /// One-shot flag so `BudgetWarning` is emitted at most once per
    /// session. Users don't want a warn per round after crossing the
    /// threshold.
    warned: bool,
}

impl AgentSession {
    pub fn new(
        provider: Box<dyn AgentProvider>,
        tools: Vec<ToolDefinition>,
        system_prompt: String,
        event_tx: mpsc::Sender<AiEvent>,
        command_rx: mpsc::Receiver<AiCommand>,
    ) -> Self {
        AgentSession {
            provider,
            tools,
            messages: Vec::new(),
            system_prompt,
            event_tx,
            command_rx,
            max_rounds: 20,
            max_messages: 200,
            consecutive_errors: 0,
            price: None,
            budget: crate::BudgetConfig::default(),
            session_cost_usd: 0.0,
            session_tokens_in: 0,
            session_tokens_out: 0,
            warned: false,
        }
    }

    /// Configure model + budget for this session. Called once by the
    /// editor bootstrap after the session is constructed but before
    /// it starts running. Separated from `new` so tests can exercise
    /// the session without a real `ProviderConfig`.
    ///
    /// The model name is resolved to a `ModelPrice` immediately and
    /// cached — the pricing table doesn't change at runtime, so every
    /// subsequent round can skip the prefix-scan + lowercase alloc.
    pub fn with_budget(mut self, model: impl AsRef<str>, budget: crate::BudgetConfig) -> Self {
        self.price = crate::pricing::lookup(model.as_ref());
        self.budget = budget;
        self
    }

    /// Execute shell_exec tool asynchronously on the AI task thread.
    ///
    /// Emacs lesson: Emacs's `shell-command` blocks the entire editor because
    /// process.c runs synchronously on the main thread. We run shell commands
    /// on the AI's spawned tokio task, so the editor remains responsive.
    ///
    /// Security: rejects commands containing dangerous patterns (rm -rf /,
    /// fork bombs, etc.) and caps timeout at 120 seconds.
    async fn execute_shell(call: &ToolCall) -> ToolResult {
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if command.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                success: false,
                output: "Missing 'command' argument".into(),
            };
        }

        // Reject obviously dangerous commands
        let blocked_patterns = [
            "rm -rf /", "rm -fr /", "mkfs.", "dd if=", ":(){", // fork bomb
            ">(){ :",
        ];
        for pattern in &blocked_patterns {
            if command.contains(pattern) {
                warn!(command, pattern, "blocked dangerous shell command");
                return ToolResult {
                    tool_call_id: call.id.clone(),
                    success: false,
                    output: format!("Command blocked: contains dangerous pattern '{}'", pattern),
                };
            }
        }

        let timeout_secs = call
            .arguments
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .min(120); // Cap at 2 minutes

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let status = output.status.code().unwrap_or(-1);

                let mut out = format!("exit_code: {}\n", status);
                if !stdout.is_empty() {
                    // Truncate to 10k chars to avoid blowing up context
                    let stdout_str = if stdout.len() > 10_000 {
                        format!("{}...[truncated]", &stdout[..10_000])
                    } else {
                        stdout.to_string()
                    };
                    out.push_str(&format!("stdout:\n{}\n", stdout_str));
                }
                if !stderr.is_empty() {
                    let stderr_str = if stderr.len() > 5_000 {
                        format!("{}...[truncated]", &stderr[..5_000])
                    } else {
                        stderr.to_string()
                    };
                    out.push_str(&format!("stderr:\n{}\n", stderr_str));
                }

                ToolResult {
                    tool_call_id: call.id.clone(),
                    success: output.status.success(),
                    output: out,
                }
            }
            Ok(Err(e)) => ToolResult {
                tool_call_id: call.id.clone(),
                success: false,
                output: format!("Failed to execute command: {}", e),
            },
            Err(_) => ToolResult {
                tool_call_id: call.id.clone(),
                success: false,
                output: format!("Command timed out after {} seconds", timeout_secs),
            },
        }
    }

    /// Main loop: wait for prompts, run agentic loop, send results.
    pub async fn run(mut self) {
        info!("AI session started, waiting for prompts");
        loop {
            match self.command_rx.recv().await {
                Some(AiCommand::Prompt(prompt)) => {
                    info!(prompt_len = prompt.len(), "received AI prompt");
                    self.handle_prompt(prompt).await;
                }
                Some(AiCommand::Cancel) => {
                    info!("AI cancel received");
                    continue;
                }
                Some(AiCommand::Shutdown) | None => {
                    info!("AI session shutting down");
                    break;
                }
            }
        }
    }

    /// Update cost tallies from a successful provider response. Emits
    /// `AiEvent::CostUpdate` (always, so the status line reflects token
    /// counters even for unpriced models) and, on the first crossing,
    /// `AiEvent::BudgetWarning`.
    ///
    /// Unpriced models (Ollama / unknown ids): tokens accumulate, USD
    /// stays at zero. This is intentional — local models are free and
    /// the user should still see throughput info.
    async fn update_cost(&mut self, response: &ProviderResponse) {
        let Some(usage) = response.usage else { return };
        self.session_tokens_in += usage.prompt_tokens;
        self.session_tokens_out += usage.completion_tokens;
        let last_call_usd = match self.price {
            Some(price) => {
                let c = price.cost_usd(usage.prompt_tokens, usage.completion_tokens);
                self.session_cost_usd += c;
                c
            }
            None => 0.0,
        };
        let _ = self
            .event_tx
            .send(AiEvent::CostUpdate {
                session_usd: self.session_cost_usd,
                last_call_usd,
                tokens_in: self.session_tokens_in,
                tokens_out: self.session_tokens_out,
            })
            .await;
        if !self.warned {
            if let Some(threshold) = self.budget.session_warn_usd {
                if self.session_cost_usd >= threshold {
                    self.warned = true;
                    let _ = self
                        .event_tx
                        .send(AiEvent::BudgetWarning {
                            session_usd: self.session_cost_usd,
                            threshold_usd: threshold,
                        })
                        .await;
                }
            }
        }
    }

    /// Trim message history to stay within bounds.
    /// Keeps the first message (initial user context) and the most recent messages.
    fn trim_messages(&mut self) {
        if self.messages.len() <= self.max_messages {
            return;
        }
        let excess = self.messages.len() - self.max_messages;
        // Keep first message for context, trim from position 1
        if self.messages.len() > 1 {
            let trim_count = excess.min(self.messages.len() - 1);
            self.messages.drain(1..1 + trim_count);
            debug!(
                trimmed = trim_count,
                remaining = self.messages.len(),
                "trimmed conversation history"
            );
        }
    }

    async fn handle_prompt(&mut self, prompt: String) {
        self.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(prompt),
        });
        self.trim_messages();

        for round in 0..self.max_rounds {
            debug!(round, max_rounds = self.max_rounds, "AI provider send");

            // Cost circuit breaker: refuse the send if the session is
            // already over budget. Checked *before* every round so a
            // spiral stops as soon as the cap is crossed, not after one
            // more (potentially expensive) round.
            if let Some(cap) = self.budget.session_hard_cap_usd {
                if self.session_cost_usd >= cap {
                    warn!(
                        session_usd = self.session_cost_usd,
                        cap_usd = cap,
                        round,
                        "AI session hard budget cap reached — aborting"
                    );
                    let _ = self
                        .event_tx
                        .send(AiEvent::BudgetExceeded {
                            session_usd: self.session_cost_usd,
                            cap_usd: cap,
                        })
                        .await;
                    return;
                }
            }

            // Backpressure warning: check event channel capacity
            let capacity = self.event_tx.capacity();
            if capacity < 4 {
                warn!(
                    capacity,
                    "AI event channel near capacity — editor may be falling behind"
                );
            }

            let response = match self
                .provider
                .send(&self.messages, &self.tools, &self.system_prompt)
                .await
            {
                Ok(r) => {
                    debug!(
                        round,
                        stop_reason = ?r.stop_reason,
                        tool_calls = r.tool_calls.len(),
                        has_text = r.text.is_some(),
                        "AI provider response received"
                    );
                    self.consecutive_errors = 0; // Reset circuit breaker on success
                    self.update_cost(&r).await;
                    r
                }
                Err(e) => {
                    self.consecutive_errors += 1;
                    error!(
                        round,
                        error = %e.message,
                        retryable = e.retryable,
                        consecutive_errors = self.consecutive_errors,
                        "AI provider error"
                    );

                    // Circuit breaker: if retryable and under threshold, backoff and retry
                    if e.retryable && self.consecutive_errors <= 3 {
                        let backoff = std::time::Duration::from_millis(
                            500 * (1 << (self.consecutive_errors - 1)),
                        );
                        warn!(
                            backoff_ms = backoff.as_millis(),
                            attempt = self.consecutive_errors,
                            "retrying after backoff"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    let _ = self.event_tx.send(AiEvent::Error(e.message)).await;
                    return;
                }
            };

            // Send text response if present
            if let Some(ref text) = response.text {
                let _ = self
                    .event_tx
                    .send(AiEvent::TextResponse(text.clone()))
                    .await;
            }

            // If no tool calls or EndTurn, we're done
            if response.tool_calls.is_empty() || response.stop_reason == StopReason::EndTurn {
                let final_text = response.text.unwrap_or_default();
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: MessageContent::Text(final_text.clone()),
                });
                let _ = self
                    .event_tx
                    .send(AiEvent::SessionComplete(final_text))
                    .await;
                return;
            }

            // Record assistant message with tool calls
            self.messages.push(Message {
                role: Role::Assistant,
                content: MessageContent::ToolCalls(response.tool_calls.clone()),
            });

            // Execute each tool call
            for call in &response.tool_calls {
                // shell_exec runs async on this task — no need to cross to main thread
                if call.name == "shell_exec" {
                    let command_arg = call
                        .arguments
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    debug!(
                        tool = "shell_exec",
                        command = command_arg,
                        "executing shell command on AI task"
                    );
                    let result = Self::execute_shell(call).await;
                    debug!(
                        tool = "shell_exec",
                        success = result.success,
                        "shell command complete"
                    );
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                debug!(tool = %call.name, call_id = %call.id, "requesting tool execution from main thread");
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let send_result = self
                    .event_tx
                    .send(AiEvent::ToolCallRequest {
                        call: call.clone(),
                        reply: reply_tx,
                    })
                    .await;

                if send_result.is_err() {
                    error!("event channel closed — cannot send tool call request");
                    let _ = self
                        .event_tx
                        .send(AiEvent::Error("Event channel closed".into()))
                        .await;
                    return;
                }

                match reply_rx.await {
                    Ok(result) => {
                        debug!(tool = %call.name, success = result.success, "tool result received");
                        self.messages.push(Message {
                            role: Role::Tool,
                            content: MessageContent::ToolResult(result),
                        });
                    }
                    Err(_) => {
                        error!(tool = %call.name, "tool result channel closed");
                        let _ = self
                            .event_tx
                            .send(AiEvent::Error("Tool result channel closed".into()))
                            .await;
                        return;
                    }
                }
            }
            // Loop: provider sees tool results and may issue more calls
        }

        warn!(
            max_rounds = self.max_rounds,
            "AI exceeded maximum tool call rounds"
        );
        let _ = self
            .event_tx
            .send(AiEvent::Error(format!(
                "AI exceeded maximum tool call rounds ({})",
                self.max_rounds
            )))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock provider that returns pre-configured responses.
    struct MockProvider {
        responses: std::sync::Mutex<Vec<ProviderResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            MockProvider {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for MockProvider {
        async fn send(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system_prompt: &str,
        ) -> Result<ProviderResponse, ProviderError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Err(ProviderError {
                    message: "No more mock responses".into(),
                    retryable: false,
                })
            } else {
                Ok(responses.remove(0))
            }
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn text_only_response() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![ProviderResponse {
            text: Some("Hello!".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        }]));

        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

        tokio::spawn(session.run());

        cmd_tx.send(AiCommand::Prompt("hi".into())).await.unwrap();

        // Should get TextResponse then SessionComplete
        match event_rx.recv().await.unwrap() {
            AiEvent::TextResponse(t) => assert_eq!(t, "Hello!"),
            other => panic!("expected TextResponse, got {:?}", other),
        }
        match event_rx.recv().await.unwrap() {
            AiEvent::SessionComplete(t) => assert_eq!(t, "Hello!"),
            other => panic!("expected SessionComplete, got {:?}", other),
        }

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn single_tool_call_round_trip() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![
            // First response: tool call
            ProviderResponse {
                text: Some("Let me check.".into()),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "cursor_info".into(),
                    arguments: serde_json::json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Second response: final text after getting tool result
            ProviderResponse {
                text: Some("You're on line 1.".into()),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]));

        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

        tokio::spawn(session.run());

        cmd_tx
            .send(AiCommand::Prompt("where am i".into()))
            .await
            .unwrap();

        // TextResponse from first response
        match event_rx.recv().await.unwrap() {
            AiEvent::TextResponse(t) => assert_eq!(t, "Let me check."),
            other => panic!("expected TextResponse, got {:?}", other),
        }

        // ToolCallRequest
        match event_rx.recv().await.unwrap() {
            AiEvent::ToolCallRequest { call, reply } => {
                assert_eq!(call.name, "cursor_info");
                reply
                    .send(ToolResult {
                        tool_call_id: "call_1".into(),
                        success: true,
                        output: r#"{"cursor_row":1}"#.into(),
                    })
                    .unwrap();
            }
            other => panic!("expected ToolCallRequest, got {:?}", other),
        }

        // TextResponse from second response
        match event_rx.recv().await.unwrap() {
            AiEvent::TextResponse(t) => assert_eq!(t, "You're on line 1."),
            other => panic!("expected TextResponse, got {:?}", other),
        }

        // SessionComplete
        match event_rx.recv().await.unwrap() {
            AiEvent::SessionComplete(t) => assert_eq!(t, "You're on line 1."),
            other => panic!("expected SessionComplete, got {:?}", other),
        }

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn provider_error_sends_error_event() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // Empty responses = will return error
        let provider = Box::new(MockProvider::new(vec![]));
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

        tokio::spawn(session.run());

        cmd_tx.send(AiCommand::Prompt("hi".into())).await.unwrap();

        match event_rx.recv().await.unwrap() {
            AiEvent::Error(msg) => assert!(msg.contains("No more mock responses")),
            other => panic!("expected Error, got {:?}", other),
        }

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn max_rounds_exceeded() {
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // Provider always returns a tool call — will hit max rounds
        let mut responses = Vec::new();
        for i in 0..25 {
            responses.push(ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: format!("call_{}", i),
                    name: "cursor_info".into(),
                    arguments: serde_json::json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            });
        }

        let provider = Box::new(MockProvider::new(responses));
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

        tokio::spawn(session.run());

        cmd_tx.send(AiCommand::Prompt("loop".into())).await.unwrap();

        // Drain events until we get the error
        let mut found_error = false;
        for _ in 0..100 {
            match event_rx.recv().await {
                Some(AiEvent::Error(msg)) => {
                    assert!(msg.contains("exceeded maximum"));
                    found_error = true;
                    break;
                }
                Some(AiEvent::ToolCallRequest { reply, .. }) => {
                    let _ = reply.send(ToolResult {
                        tool_call_id: "x".into(),
                        success: true,
                        output: "ok".into(),
                    });
                }
                _ => continue,
            }
        }
        assert!(found_error, "should have received max rounds error");

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn shell_exec_runs_command() {
        let call = ToolCall {
            id: "shell_1".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({"command": "echo hello"}),
        };
        let result = AgentSession::execute_shell(&call).await;
        assert!(result.success);
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("exit_code: 0"));
    }

    #[tokio::test]
    async fn shell_exec_missing_command() {
        let call = ToolCall {
            id: "shell_2".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({}),
        };
        let result = AgentSession::execute_shell(&call).await;
        assert!(!result.success);
        assert!(result.output.contains("Missing"));
    }

    #[tokio::test]
    async fn shell_exec_timeout() {
        let call = ToolCall {
            id: "shell_3".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({"command": "sleep 60", "timeout_secs": 1}),
        };
        let result = AgentSession::execute_shell(&call).await;
        assert!(!result.success);
        assert!(result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn shell_exec_nonzero_exit() {
        let call = ToolCall {
            id: "shell_4".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({"command": "exit 42"}),
        };
        let result = AgentSession::execute_shell(&call).await;
        assert!(!result.success);
        assert!(result.output.contains("exit_code: 42"));
    }

    #[tokio::test]
    async fn shell_exec_handled_in_session() {
        // Verify shell_exec is handled locally in session, not sent to main thread
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call_shell".into(),
                    name: "shell_exec".into(),
                    arguments: serde_json::json!({"command": "echo fromshell"}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            ProviderResponse {
                text: Some("Done.".into()),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]));

        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
        tokio::spawn(session.run());

        cmd_tx
            .send(AiCommand::Prompt("run something".into()))
            .await
            .unwrap();

        // Should NOT get a ToolCallRequest — shell_exec is handled locally.
        // We should get TextResponse then SessionComplete.
        match event_rx.recv().await.unwrap() {
            AiEvent::TextResponse(t) => assert_eq!(t, "Done."),
            AiEvent::SessionComplete(t) => assert_eq!(t, "Done."),
            other => panic!("expected TextResponse or SessionComplete, got {:?}", other),
        }

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_exits_loop() {
        let (event_tx, _event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![]));
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

        let handle = tokio::spawn(session.run());

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();

        // Should complete without hanging
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("session should exit")
            .expect("session should not panic");
    }

    #[tokio::test]
    async fn shell_exec_blocks_dangerous_commands() {
        let dangerous_commands = vec![
            "rm -rf /",
            "rm -fr /home",
            "mkfs.ext4 /dev/sda",
            "dd if=/dev/zero of=/dev/sda",
            ":(){:|:&};:",
        ];
        for cmd in dangerous_commands {
            let call = ToolCall {
                id: "shell_blocked".into(),
                name: "shell_exec".into(),
                arguments: serde_json::json!({"command": cmd}),
            };
            let result = AgentSession::execute_shell(&call).await;
            assert!(!result.success, "should block: {}", cmd);
            assert!(
                result.output.contains("blocked"),
                "should mention 'blocked' for: {}",
                cmd
            );
        }
    }

    #[tokio::test]
    async fn shell_exec_caps_timeout() {
        // Timeout should be capped at 120s even if requesting more
        let call = ToolCall {
            id: "shell_cap".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({"command": "echo ok", "timeout_secs": 9999}),
        };
        let result = AgentSession::execute_shell(&call).await;
        assert!(result.success);
        assert!(result.output.contains("ok"));
    }

    #[test]
    fn message_trimming() {
        let (event_tx, _rx) = mpsc::channel(32);
        let (_tx, cmd_rx) = mpsc::channel(8);
        let provider = Box::new(MockProvider::new(vec![]));
        let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
        session.max_messages = 5;

        // Add 10 messages
        for i in 0..10 {
            session.messages.push(Message {
                role: Role::User,
                content: MessageContent::Text(format!("msg{}", i)),
            });
        }
        assert_eq!(session.messages.len(), 10);

        session.trim_messages();
        assert_eq!(session.messages.len(), 5);
        // First message should be preserved
        match &session.messages[0].content {
            MessageContent::Text(t) => assert_eq!(t, "msg0"),
            _ => panic!("expected text"),
        }
        // Last message should be the most recent
        match &session.messages[4].content {
            MessageContent::Text(t) => assert_eq!(t, "msg9"),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn circuit_breaker_retries_on_retryable_error() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // First two responses are retryable errors, third succeeds
        struct RetryProvider {
            call_count: std::sync::Mutex<usize>,
        }
        #[async_trait::async_trait]
        impl AgentProvider for RetryProvider {
            async fn send(
                &self,
                _messages: &[Message],
                _tools: &[ToolDefinition],
                _system_prompt: &str,
            ) -> Result<ProviderResponse, ProviderError> {
                let mut count = self.call_count.lock().unwrap();
                *count += 1;
                if *count <= 2 {
                    Err(ProviderError {
                        message: format!("rate limited (attempt {})", count),
                        retryable: true,
                    })
                } else {
                    Ok(ProviderResponse {
                        text: Some("recovered!".into()),
                        tool_calls: vec![],
                        stop_reason: StopReason::EndTurn,
                        usage: None,
                    })
                }
            }
            fn name(&self) -> &str {
                "retry-mock"
            }
        }

        let provider = Box::new(RetryProvider {
            call_count: std::sync::Mutex::new(0),
        });
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
        tokio::spawn(session.run());

        cmd_tx.send(AiCommand::Prompt("hi".into())).await.unwrap();

        // Should eventually get a successful response after retries
        let mut got_response = false;
        for _ in 0..10 {
            match tokio::time::timeout(std::time::Duration::from_secs(10), event_rx.recv()).await {
                Ok(Some(AiEvent::TextResponse(t))) => {
                    assert_eq!(t, "recovered!");
                    got_response = true;
                    break;
                }
                Ok(Some(AiEvent::SessionComplete(_))) => {
                    got_response = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(got_response, "should have recovered after retries");

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    // ---- Budget / cost tracking ----

    /// Helper: drain all events with a timeout, collecting them into a Vec.
    async fn drain_events(rx: &mut mpsc::Receiver<AiEvent>) -> Vec<AiEvent> {
        let mut out = Vec::new();
        while let Ok(Some(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
        {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn cost_update_emitted_when_usage_present() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![ProviderResponse {
            text: Some("hi".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Some(Usage {
                prompt_tokens: 1_000,
                completion_tokens: 500,
            }),
        }]));

        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx)
            .with_budget("claude-sonnet-4-5", crate::BudgetConfig::default());
        tokio::spawn(session.run());
        cmd_tx.send(AiCommand::Prompt("hi".into())).await.unwrap();

        let events = drain_events(&mut event_rx).await;
        let cost = events.iter().find_map(|e| match e {
            AiEvent::CostUpdate {
                session_usd,
                tokens_in,
                tokens_out,
                ..
            } => Some((*session_usd, *tokens_in, *tokens_out)),
            _ => None,
        });
        let (usd, tin, tout) = cost.expect("expected CostUpdate event");
        // Sonnet: $3/Mtok in, $15/Mtok out -> 1000 * 3/1M + 500 * 15/1M = 0.003 + 0.0075 = 0.0105
        assert!((usd - 0.0105).abs() < 1e-9, "got ${}", usd);
        assert_eq!(tin, 1000);
        assert_eq!(tout, 500);

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn cost_update_zero_for_unpriced_model() {
        // Ollama / local models aren't in the pricing table — tokens
        // should still count but USD stays at zero.
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        let provider = Box::new(MockProvider::new(vec![ProviderResponse {
            text: Some("hi".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Some(Usage {
                prompt_tokens: 1_000,
                completion_tokens: 500,
            }),
        }]));

        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx)
            .with_budget("llama3:latest", crate::BudgetConfig::default());
        tokio::spawn(session.run());
        cmd_tx.send(AiCommand::Prompt("hi".into())).await.unwrap();

        let events = drain_events(&mut event_rx).await;
        let usd = events.iter().find_map(|e| match e {
            AiEvent::CostUpdate { session_usd, .. } => Some(*session_usd),
            _ => None,
        });
        assert_eq!(usd, Some(0.0));

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn budget_warning_fires_once_on_crossing() {
        let (event_tx, mut event_rx) = mpsc::channel(64);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // Two rounds, each with 1000 prompt + 500 output = $0.0105 per round on sonnet.
        // Warn threshold $0.005 is crossed after the first round only.
        let provider = Box::new(MockProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "cursor_info".into(),
                    arguments: serde_json::json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: Some(Usage {
                    prompt_tokens: 1_000,
                    completion_tokens: 500,
                }),
            },
            ProviderResponse {
                text: Some("done".into()),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Some(Usage {
                    prompt_tokens: 1_000,
                    completion_tokens: 500,
                }),
            },
        ]));

        let budget = crate::BudgetConfig {
            session_warn_usd: Some(0.005),
            session_hard_cap_usd: None,
        };
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx)
            .with_budget("claude-sonnet-4-5", budget);
        tokio::spawn(session.run());
        cmd_tx.send(AiCommand::Prompt("go".into())).await.unwrap();

        let events = drain_events(&mut event_rx).await;
        let warn_count = events
            .iter()
            .filter(|e| matches!(e, AiEvent::BudgetWarning { .. }))
            .count();
        assert_eq!(warn_count, 1, "warning should fire exactly once");

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }

    #[tokio::test]
    async fn hard_cap_aborts_before_provider_call() {
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);

        // Use a provider that records how many times it was called.
        // Round 0 returns a tool call with usage that pushes cost past
        // the cap — round 1 must refuse to call the provider.
        struct CountingProvider {
            calls: std::sync::Arc<std::sync::Mutex<usize>>,
        }
        #[async_trait::async_trait]
        impl AgentProvider for CountingProvider {
            async fn send(
                &self,
                _: &[Message],
                _: &[ToolDefinition],
                _: &str,
            ) -> Result<ProviderResponse, ProviderError> {
                *self.calls.lock().unwrap() += 1;
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "c1".into(),
                        name: "cursor_info".into(),
                        arguments: serde_json::json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    usage: Some(Usage {
                        prompt_tokens: 10_000,
                        completion_tokens: 2_000,
                    }),
                })
            }
            fn name(&self) -> &str {
                "counting"
            }
        }
        let calls = std::sync::Arc::new(std::sync::Mutex::new(0));
        let provider = Box::new(CountingProvider {
            calls: calls.clone(),
        });

        // 10k in + 2k out on Sonnet = 0.03 + 0.03 = $0.06. Cap is $0.02
        // so round 1 must be refused after round 0 pushes us over.
        let budget = crate::BudgetConfig {
            session_warn_usd: None,
            session_hard_cap_usd: Some(0.02),
        };
        let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx)
            .with_budget("claude-sonnet-4-5", budget);
        tokio::spawn(session.run());
        cmd_tx.send(AiCommand::Prompt("go".into())).await.unwrap();

        // Manually drive the event loop: reply to the tool call so the
        // session unblocks and reaches the round-1 cap check. Without
        // this the session hangs on the oneshot awaiting a reply.
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, event_rx.recv()).await {
                Ok(Some(AiEvent::ToolCallRequest { call, reply })) => {
                    let _ = reply.send(ToolResult {
                        tool_call_id: call.id.clone(),
                        success: true,
                        output: "ok".into(),
                    });
                    events.push(AiEvent::ToolCallRequest {
                        call,
                        reply: tokio::sync::oneshot::channel().0,
                    });
                }
                Ok(Some(ev)) => events.push(ev),
                _ => break,
            }
        }

        let saw_budget_err = events
            .iter()
            .any(|e| matches!(e, AiEvent::BudgetExceeded { .. }));
        assert!(saw_budget_err, "expected BudgetExceeded event: {events:?}");
        // Provider was called exactly once — the round that pushed us
        // over the cap. Round 1 never reached the provider.
        assert_eq!(*calls.lock().unwrap(), 1);

        cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    }
}
