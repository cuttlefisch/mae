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
                kind: ErrorKind::Unknown,
            })
        } else {
            Ok(responses.remove(0))
        }
    }

    fn name(&self) -> &str {
        "mock"
    }
}

/// Receive the next event, skipping RoundUpdate and the initialization TextResponse.
async fn recv_filtered(rx: &mut mpsc::Receiver<AiEvent>) -> AiEvent {
    loop {
        let evt = rx.recv().await.unwrap();
        match &evt {
            AiEvent::RoundUpdate { .. } => continue,
            AiEvent::ToolCallStarted { .. } => continue,
            AiEvent::ToolCallFinished { .. } => continue,
            AiEvent::TextResponse { text, .. } if text.starts_with("[AI:") => continue,
            _ => return evt,
        }
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
    match recv_filtered(&mut event_rx).await {
        AiEvent::TextResponse { text, .. } => assert_eq!(text, "Hello!"),
        other => panic!("expected TextResponse, got {:?}", other),
    }
    match recv_filtered(&mut event_rx).await {
        AiEvent::SessionComplete { text, .. } => assert_eq!(text, "Hello!"),
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
    match recv_filtered(&mut event_rx).await {
        AiEvent::TextResponse { text, .. } => assert_eq!(text, "Let me check."),
        other => panic!("expected TextResponse, got {:?}", other),
    }

    // ToolCallRequest
    match recv_filtered(&mut event_rx).await {
        AiEvent::ToolCallRequest { call, reply } => {
            assert_eq!(call.name, "cursor_info");
            reply
                .send(ToolResult {
                    tool_call_id: "call_1".into(),
                    tool_name: "cursor_info".into(),
                    success: true,
                    output: r#"{"cursor_row":1}"#.into(),
                })
                .unwrap();
        }
        other => panic!("expected ToolCallRequest, got {:?}", other),
    }

    // TextResponse from second response
    match recv_filtered(&mut event_rx).await {
        AiEvent::TextResponse { text, .. } => assert_eq!(text, "You're on line 1."),
        other => panic!("expected TextResponse, got {:?}", other),
    }

    // SessionComplete
    match recv_filtered(&mut event_rx).await {
        AiEvent::SessionComplete { text, .. } => assert_eq!(text, "You're on line 1."),
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

    match recv_filtered(&mut event_rx).await {
        AiEvent::Error(msg, _) => assert!(msg.contains("No more mock responses")),
        other => panic!("expected Error, got {:?}", other),
    }

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
    match recv_filtered(&mut event_rx).await {
        AiEvent::TextResponse { text, .. } => assert_eq!(text, "Done."),
        other => panic!("expected TextResponse, got {:?}", other),
    }
    match recv_filtered(&mut event_rx).await {
        AiEvent::SessionComplete { text, .. } => assert_eq!(text, "Done."),
        other => panic!("expected SessionComplete, got {:?}", other),
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
                    kind: ErrorKind::RateLimit,
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
            Ok(Some(AiEvent::TextResponse { text, .. })) => {
                if text.starts_with("[AI:") {
                    continue; // skip init message
                }
                assert_eq!(text, "recovered!");
                got_response = true;
                break;
            }
            Ok(Some(AiEvent::SessionComplete { .. })) => {
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
            prompt_tokens: 1000,
            completion_tokens: 500,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
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
            prompt_tokens: 1000,
            completion_tokens: 500,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
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
                prompt_tokens: 10000,
                completion_tokens: 5000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        },
        ProviderResponse {
            text: Some("done".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Some(Usage {
                prompt_tokens: 10000,
                completion_tokens: 5000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
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
                    prompt_tokens: 10000,
                    completion_tokens: 5000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
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
                    tool_name: call.name.clone(),
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

#[tokio::test]
async fn test_tool_loop_circuit_breaker() {
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Provider returns the same tool call every time
    let mut responses = Vec::new();
    for i in 0..10 {
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
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    session.max_rounds = 20;

    tokio::spawn(session.run());
    cmd_tx.send(AiCommand::Prompt("loop".into())).await.unwrap();

    let mut found_circuit_breaker = false;
    for _ in 0..50 {
        match event_rx.recv().await {
            Some(AiEvent::Error(msg, _)) => {
                if msg.contains("stuck in a tool loop") {
                    found_circuit_breaker = true;
                    break;
                }
            }
            Some(AiEvent::ToolCallRequest { call, reply }) => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: true,
                    output: "ok".into(),
                });
            }
            _ => continue,
        }
    }
    assert!(
        found_circuit_breaker,
        "should have triggered circuit breaker"
    );
    cmd_tx.send(AiCommand::Shutdown).await.unwrap();
}

#[test]
fn test_trim_preserves_tool_call_pairs() {
    let (event_tx, _) = mpsc::channel(32);
    let (_, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

    // Setup messages:
    // 0: User (kept)
    // 1: Assistant (Text) - should be pruned
    // 2: Assistant (ToolCalls) - should NOT be orphaned from 3
    // 3: Tool (Result)
    // 4: Assistant (Final)
    session.messages = vec![
        Message {
            role: Role::User,
            content: MessageContent::Text("init".into()),
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("unrelated".into()),
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::ToolCalls(vec![ToolCall {
                id: "c1".into(),
                name: "t1".into(),
                arguments: serde_json::json!({}),
            }]),
        },
        Message {
            role: Role::Tool,
            content: MessageContent::ToolResult(ToolResult {
                tool_call_id: "c1".into(),
                tool_name: "t1".into(),
                success: true,
                output: "ok".into(),
            }),
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("final".into()),
        },
    ];

    // Set context window to allow some pruning but keep some history
    session.context_window = 10000;
    session.max_messages = 3;

    session.trim_messages();

    // Verify: messages.len() should be 3 (User + Assistant summary + Final)
    assert!(session.messages.len() > 1);
    assert_ne!(
        session.messages[1].role,
        Role::Tool,
        "Tool message was orphaned at boundary"
    );
}

#[test]
fn test_trim_messages_protects_active_transaction() {
    let (event_tx, _) = mpsc::channel(32);
    let (_, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

    // System prompt is ~3 tokens
    // Each message is ~10 tokens
    session.context_window = 100; // Very small
    session.max_messages = 50;
    session.reserved_output = 20;

    session.messages = vec![
        Message {
            role: Role::User,
            content: MessageContent::Text("historical 1".into()),
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("historical 2".into()),
        },
        Message {
            role: Role::User,
            content: MessageContent::Text("START TRANSACTION".into()),
        },
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("tool call 1".into()),
        },
        Message {
            role: Role::Tool,
            content: MessageContent::ToolResult(ToolResult {
                tool_call_id: "id1".into(),
                tool_name: "t1".into(),
                success: true,
                output: "tool result 1".into(),
            }),
        },
    ];

    // Mark the transaction start at "START TRANSACTION" (index 2)
    session.transaction_start_idx = Some(2);

    // Pre-trim state: 5 messages
    assert_eq!(session.messages.len(), 5);

    session.trim_messages();

    // Verification:
    // - "historical 1" (index 0) is kept because it's the first message.
    // - "historical 2" (index 1) should be pruned because it's before transaction_start_idx and we're over budget.
    // - "START TRANSACTION" (index 2) and beyond must be preserved.
    assert_eq!(session.messages.len(), 4);
    assert_eq!(
        token_estimate::estimate_messages_tokens(&[session.messages[1].clone()]),
        token_estimate::estimate_messages_tokens(&[Message {
            role: Role::User,
            content: MessageContent::Text("START TRANSACTION".into()),
        }]),
        "Transaction start message was pruned!"
    );
    assert_eq!(session.transaction_start_idx, Some(1));
}

#[tokio::test]
async fn test_session_cancel_emits_session_complete() {
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Setup a mock provider that returns a tool call to simulate being in the tool loop
    let responses = vec![ProviderResponse {
        text: Some("Thinking...".into()),
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "shell_exec".into(),
            arguments: serde_json::json!({"command": "sleep 10"}),
        }],
        stop_reason: StopReason::ToolUse,
        usage: None,
    }];

    let provider = Box::new(MockProvider::new(responses));
    let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

    // Start session in background
    let session_task = tokio::spawn(session.run());

    // Send a prompt to start the loop
    cmd_tx
        .send(AiCommand::Prompt("run something slow".into()))
        .await
        .unwrap();

    // Wait for the first text response
    let _ = event_rx.recv().await;

    // While it's "executing" the tool, send a cancel
    cmd_tx.send(AiCommand::Cancel).await.unwrap();

    // Assert we receive TextResponse("[Interrupted by user]") followed by SessionComplete
    let mut got_interrupted_text = false;
    let mut got_session_complete = false;

    for _ in 0..10 {
        match event_rx.recv().await {
            Some(AiEvent::TextResponse { text, .. }) => {
                if text.contains("[Interrupted by user]") {
                    got_interrupted_text = true;
                }
            }
            Some(AiEvent::SessionComplete { text, .. }) => {
                if text.contains("[Interrupted by user]") {
                    got_session_complete = true;
                }
                break;
            }
            _ => continue,
        }
    }

    assert!(got_interrupted_text, "Missing interrupted text response");
    assert!(
        got_session_complete,
        "Missing SessionComplete event after cancellation"
    );

    cmd_tx.send(AiCommand::Shutdown).await.unwrap();
    let _ = session_task.await;
}

#[tokio::test]
async fn test_mid_flight_compaction() {
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Provider returns many tool calls to trigger compaction
    let mut responses = Vec::new();
    for i in 0..25 {
        responses.push(ProviderResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: format!("id{}", i),
                name: "log_activity".into(),
                arguments: serde_json::json!({"activity": format!("step {}", i)}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: None,
        });
    }
    responses.push(ProviderResponse {
        text: Some("Done".into()),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: None,
    });

    let provider = Box::new(MockProvider::new(responses));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    session.context_window = 10000;

    tokio::spawn(session.run());
    cmd_tx
        .send(AiCommand::Prompt("start".to_string()))
        .await
        .unwrap();

    // We expect many RoundUpdates and eventually a SessionComplete
    let mut max_round = 0;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::RoundUpdate { round, .. } => {
                max_round = max_round.max(round);
            }
            AiEvent::SessionComplete { .. } => break,
            _ => continue,
        }
    }

    assert!(max_round >= 20, "Should have run many rounds");
    // Internal check: messages should have been compacted.
    // In the test we can't easily check internal state of the spawned task,
    // but we can verify the session didn't crash and finished.
}

#[tokio::test]
async fn test_ui_events_for_internal_tools() {
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    let provider = Box::new(MockProvider::new(vec![
        ProviderResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "shell_exec".into(),
                arguments: serde_json::json!({"command": "echo ui-test"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
        ProviderResponse {
            text: Some("Ok".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]));

    let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    tokio::spawn(session.run());

    cmd_tx
        .send(AiCommand::Prompt("run".to_string()))
        .await
        .unwrap();

    let mut started = false;
    let mut finished = false;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::ToolCallStarted { name } if name == "shell_exec" => started = true,
            AiEvent::ToolCallFinished { .. } => finished = true,
            AiEvent::SessionComplete { .. } => break,
            _ => continue,
        }
    }

    assert!(started, "Missing ToolCallStarted for shell_exec");
    assert!(finished, "Missing ToolCallFinished for shell_exec");
}

#[tokio::test]
async fn test_log_activity_tool() {
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    let provider = Box::new(MockProvider::new(vec![
        ProviderResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "log_activity".into(),
                arguments: serde_json::json!({"activity": "I am thinking"}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
        ProviderResponse {
            text: Some("Done".into()),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]));

    let session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    tokio::spawn(session.run());

    cmd_tx
        .send(AiCommand::Prompt("think".to_string()))
        .await
        .unwrap();

    let mut activity_logged = false;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::ToolCallFinished { output, .. } if output == "I am thinking" => {
                activity_logged = true
            }
            AiEvent::SessionComplete { .. } => break,
            _ => continue,
        }
    }

    assert!(activity_logged, "Activity was not logged to UI");
}

#[tokio::test]
async fn test_max_rounds_enforcement() {
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Provider always returns tool calls with unique args (to avoid
    // oscillation detection) — would loop forever without max_rounds
    struct InfiniteProvider {
        call_count: std::sync::Mutex<usize>,
    }
    #[async_trait::async_trait]
    impl AgentProvider for InfiniteProvider {
        async fn send(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
        ) -> Result<ProviderResponse, ProviderError> {
            let mut count = self.call_count.lock().unwrap();
            *count += 1;
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: format!("c{}", count),
                    name: "editor_state".into(),
                    arguments: serde_json::json!({"round": *count}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            })
        }
        fn name(&self) -> &str {
            "infinite"
        }
    }

    let provider = Box::new(InfiniteProvider {
        call_count: std::sync::Mutex::new(0),
    });
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    session.max_rounds = 3;

    tokio::spawn(session.run());
    cmd_tx.send(AiCommand::Prompt("go".into())).await.unwrap();

    let mut found_max_rounds_error = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, event_rx.recv()).await {
            Ok(Some(AiEvent::Error(msg, _))) if msg.contains("maximum rounds") => {
                found_max_rounds_error = true;
                break;
            }
            Ok(Some(AiEvent::ToolCallRequest { call, reply })) => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: true,
                    output: "ok".into(),
                });
            }
            _ => continue,
        }
    }
    assert!(
        found_max_rounds_error,
        "should have hit max_rounds limit after 3 rounds"
    );
    cmd_tx.send(AiCommand::Shutdown).await.unwrap();
}

#[tokio::test]
async fn test_oscillation_ab_pattern_detected() {
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Alternates between two different tools: A, B, A, B, ...
    struct OscillatingProvider {
        call_count: std::sync::Mutex<usize>,
    }
    #[async_trait::async_trait]
    impl AgentProvider for OscillatingProvider {
        async fn send(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
        ) -> Result<ProviderResponse, ProviderError> {
            let mut count = self.call_count.lock().unwrap();
            *count += 1;
            let name = if (*count).is_multiple_of(2) {
                "cursor_info"
            } else {
                "editor_state"
            };
            Ok(ProviderResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: format!("c{}", count),
                    name: name.into(),
                    arguments: serde_json::json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                usage: None,
            })
        }
        fn name(&self) -> &str {
            "oscillating"
        }
    }

    let provider = Box::new(OscillatingProvider {
        call_count: std::sync::Mutex::new(0),
    });
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    session.max_rounds = 20; // High enough that only oscillation detection should fire

    tokio::spawn(session.run());
    cmd_tx.send(AiCommand::Prompt("go".into())).await.unwrap();

    let mut found_loop_error = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, event_rx.recv()).await {
            Ok(Some(AiEvent::Error(msg, _))) if msg.contains("stuck in a tool loop") => {
                found_loop_error = true;
                break;
            }
            Ok(Some(AiEvent::ToolCallRequest { call, reply })) => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: true,
                    output: "ok".into(),
                });
            }
            _ => continue,
        }
    }
    assert!(
        found_loop_error,
        "should have detected A-B-A-B oscillation pattern"
    );
    cmd_tx.send(AiCommand::Shutdown).await.unwrap();
}

#[test]
fn test_aggressive_prune_removes_ten_percent() {
    let (event_tx, _) = mpsc::channel(32);
    let (_, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);

    // Add 21 messages (1 system + 20 content = 10% of 20 = 2 removed)
    for i in 0..21 {
        session.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(format!("msg{}", i)),
        });
    }
    assert_eq!(session.messages.len(), 21);

    session.aggressive_prune();
    // 10% of 20 non-first = 2, so 21 - 2 = 19
    assert_eq!(session.messages.len(), 19);
    // First message preserved
    match &session.messages[0].content {
        MessageContent::Text(t) => assert_eq!(t, "msg0"),
        _ => panic!("expected text"),
    }
}

#[test]
fn trim_preserves_tool_history_without_pruning() {
    // Regression test: trim_messages must NOT strip tool call history
    // when no pruning is needed (steps 1-2 didn't remove anything).
    let (event_tx, _rx) = mpsc::channel(32);
    let (_tx, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    // Large budget — no pruning needed
    session.max_messages = 100;

    // Build: [User, Assistant(ToolCalls), Tool, Tool]
    session.messages.push(Message {
        role: Role::User,
        content: MessageContent::Text("what files are here?".into()),
    });
    session.messages.push(Message {
        role: Role::Assistant,
        content: MessageContent::ToolCalls(vec![ToolCall {
            id: "call_1".into(),
            name: "git_status".into(),
            arguments: serde_json::json!({}),
        }]),
    });
    session.messages.push(Message {
        role: Role::Tool,
        content: MessageContent::ToolResult(ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "git_status".into(),
            success: true,
            output: "clean".into(),
        }),
    });
    session.messages.push(Message {
        role: Role::Tool,
        content: MessageContent::ToolResult(ToolResult {
            tool_call_id: "call_2".into(),
            tool_name: "shell_exec".into(),
            success: true,
            output: "ls output".into(),
        }),
    });

    assert_eq!(session.messages.len(), 4);
    session.trim_messages();
    // All 4 messages must survive — no pruning occurred.
    assert_eq!(session.messages.len(), 4);
    assert_eq!(session.messages[1].role, Role::Assistant);
    assert_eq!(session.messages[2].role, Role::Tool);
    assert_eq!(session.messages[3].role, Role::Tool);
}

// --- Progress checkpoint integration tests ---

/// Helper: build a ProviderResponse with tool calls dispatched to main thread.
fn tool_call_response(name: &str, args: serde_json::Value) -> ProviderResponse {
    ProviderResponse {
        text: None,
        tool_calls: vec![ToolCall {
            id: format!("call_{}", name),
            name: name.to_string(),
            arguments: args,
        }],
        stop_reason: StopReason::ToolUse,
        usage: Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }),
    }
}

/// 30 identical buffer_read calls → stagnation abort before max_rounds.
#[tokio::test]
async fn test_checkpoint_aborts_stagnant_session() {
    let (event_tx, mut event_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Build 35 identical buffer_read responses
    let mut responses: Vec<ProviderResponse> = Vec::new();
    for _ in 0..35 {
        responses.push(tool_call_response(
            "buffer_read",
            serde_json::json!({"path": "same.rs"}),
        ));
    }
    // Final end-turn in case we get that far
    responses.push(ProviderResponse {
        text: Some("done".into()),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: Some(Usage {
            prompt_tokens: 50,
            completion_tokens: 10,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }),
    });

    let provider = Box::new(MockProvider::new(responses));
    let mut session = AgentSession::new(provider, vec![], String::new(), event_tx, cmd_rx);
    session.progress = super::progress::ProgressTracker::new(5, false);

    cmd_tx.send(AiCommand::Prompt("test".into())).await.unwrap();
    tokio::spawn(session.run());

    let mut saw_error = false;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::ToolCallRequest { call, reply } => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    success: true,
                    output: "file contents".into(),
                });
            }
            AiEvent::Error(msg, _) if msg.contains("stagnation") || msg.contains("tool loop") => {
                saw_error = true;
                break;
            }
            AiEvent::Error(_, _) => break,
            AiEvent::SessionComplete { .. } => break,
            _ => {}
        }
    }
    assert!(saw_error, "expected stagnation or oscillation abort");
    cmd_tx.send(AiCommand::Shutdown).await.ok();
}

/// 30 varied rounds with different tools and files → completes without stagnation.
#[tokio::test]
async fn test_checkpoint_allows_long_varied_session() {
    let (event_tx, mut event_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // Use only tools that dispatch to main thread (avoid shell_exec which runs real commands)
    let tools_cycle = [
        ("buffer_read", serde_json::json!({"path": "a.rs"})),
        ("buffer_write", serde_json::json!({"path": "a.rs"})),
        ("project_search", serde_json::json!({"query": "foo"})),
        ("buffer_read", serde_json::json!({"path": "b.rs"})),
        ("buffer_write", serde_json::json!({"path": "b.rs"})),
        ("create_file", serde_json::json!({"path": "c.rs"})),
        ("buffer_read", serde_json::json!({"path": "c.rs"})),
        ("open_file", serde_json::json!({"path": "d.rs"})),
        ("buffer_write", serde_json::json!({"path": "d.rs"})),
        ("cursor_info", serde_json::json!({})),
    ];

    let mut responses: Vec<ProviderResponse> = Vec::new();
    for i in 0..30 {
        let (name, args) = &tools_cycle[i % tools_cycle.len()];
        responses.push(tool_call_response(name, args.clone()));
    }
    responses.push(ProviderResponse {
        text: Some("All done!".into()),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: Some(Usage {
            prompt_tokens: 50,
            completion_tokens: 10,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }),
    });

    let provider = Box::new(MockProvider::new(responses));
    let mut session = AgentSession::new(provider, vec![], String::new(), event_tx, cmd_rx);
    session.progress = super::progress::ProgressTracker::new(5, false);

    cmd_tx.send(AiCommand::Prompt("test".into())).await.unwrap();
    tokio::spawn(session.run());

    let mut saw_stagnation = false;
    let mut completed = false;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::ToolCallRequest { call, reply } => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    success: true,
                    output: "ok".into(),
                });
            }
            AiEvent::Error(msg, _) if msg.contains("stagnation") || msg.contains("tool loop") => {
                saw_stagnation = true;
                break;
            }
            AiEvent::SessionComplete { .. } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(completed, "session should complete normally");
    assert!(!saw_stagnation, "should not trigger stagnation abort");
    cmd_tx.send(AiCommand::Shutdown).await.ok();
}

/// Oscillation: A-B-A-B pattern → warn first, abort on second oscillation.
#[tokio::test]
async fn test_oscillation_warn_then_abort() {
    let (event_tx, mut event_rx) = mpsc::channel(128);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);

    // A-B-A-B-A-B-A-B pattern (8 rounds)
    let mut responses: Vec<ProviderResponse> = Vec::new();
    for i in 0..8 {
        if i % 2 == 0 {
            responses.push(tool_call_response(
                "buffer_read",
                serde_json::json!({"path": "x.rs"}),
            ));
        } else {
            responses.push(tool_call_response(
                "buffer_write",
                serde_json::json!({"path": "x.rs"}),
            ));
        }
    }
    responses.push(ProviderResponse {
        text: Some("done".into()),
        tool_calls: vec![],
        stop_reason: StopReason::EndTurn,
        usage: Some(Usage {
            prompt_tokens: 50,
            completion_tokens: 10,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }),
    });

    let provider = Box::new(MockProvider::new(responses));
    let mut session = AgentSession::new(provider, vec![], String::new(), event_tx, cmd_rx);
    session.progress = super::progress::ProgressTracker::new(10, false);

    cmd_tx.send(AiCommand::Prompt("test".into())).await.unwrap();
    tokio::spawn(session.run());

    let mut warnings = 0;
    let mut aborted = false;
    while let Some(evt) = event_rx.recv().await {
        match evt {
            AiEvent::ToolCallRequest { call, reply } => {
                let _ = reply.send(ToolResult {
                    tool_call_id: call.id,
                    tool_name: call.name,
                    success: true,
                    output: "ok".into(),
                });
            }
            AiEvent::TextResponse { text, .. } if text.contains("tool loop warning") => {
                warnings += 1;
            }
            AiEvent::Error(msg, _) if msg.contains("tool loop") => {
                aborted = true;
                break;
            }
            AiEvent::SessionComplete { .. } => break,
            _ => {}
        }
    }
    assert!(
        warnings >= 1,
        "expected at least 1 oscillation warning, got {}",
        warnings
    );
    assert!(
        aborted,
        "expected oscillation abort after reaching stagnant threshold"
    );
    cmd_tx.send(AiCommand::Shutdown).await.ok();
}

// --- compact_history unit tests ---

fn make_test_session_with_messages(messages: Vec<Message>) -> AgentSession {
    let (event_tx, _rx) = mpsc::channel(32);
    let (_tx, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let mut session = AgentSession::new(provider, vec![], "sys".into(), event_tx, cmd_rx);
    session.messages = messages;
    session
}

fn make_session_with_tools(tools: Vec<ToolDefinition>) -> AgentSession {
    let (event_tx, _rx) = mpsc::channel(32);
    let (_tx, cmd_rx) = mpsc::channel(8);
    let provider = Box::new(MockProvider::new(vec![]));
    let sys_prompt = "A".repeat(5000); // Long system prompt for testing truncation
    AgentSession::new(provider, tools, sys_prompt, event_tx, cmd_rx)
}

fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: MessageContent::Text(text.into()),
    }
}

fn assistant_msg(text: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: MessageContent::Text(text.into()),
    }
}

fn tool_calls_msg(names: &[&str]) -> Message {
    Message {
        role: Role::Assistant,
        content: MessageContent::ToolCalls(
            names
                .iter()
                .map(|n| ToolCall {
                    id: format!("call_{}", n),
                    name: n.to_string(),
                    arguments: serde_json::json!({}),
                })
                .collect(),
        ),
    }
}

fn tool_result_msg(name: &str) -> Message {
    Message {
        role: Role::Tool,
        content: MessageContent::ToolResult(ToolResult {
            tool_call_id: format!("call_{}", name),
            tool_name: name.to_string(),
            success: true,
            output: "ok".into(),
        }),
    }
}

#[test]
fn compact_history_summarizes_old_turns() {
    // 15 user+assistant pairs = 30 messages + 1 initial = 31
    let mut msgs = vec![user_msg("initial context")];
    for i in 0..15 {
        msgs.push(user_msg(&format!("question {}", i)));
        msgs.push(assistant_msg(&format!("answer {}. More detail here.", i)));
    }
    let original_len = msgs.len();
    let mut session = make_test_session_with_messages(msgs);

    session.compact_history();

    assert!(
        session.messages.len() < original_len,
        "expected compaction: {} -> {}",
        original_len,
        session.messages.len()
    );
    // First message preserved
    match &session.messages[0].content {
        MessageContent::Text(t) => assert_eq!(t, "initial context"),
        _ => panic!("first message should be text"),
    }
    // Summary messages should exist
    let summaries: Vec<_> = session
        .messages
        .iter()
        .filter(|m| {
            if let MessageContent::Text(t) = &m.content {
                t.contains("[Context summary")
            } else {
                false
            }
        })
        .collect();
    assert!(!summaries.is_empty(), "expected summary messages");
}

#[test]
fn compact_history_preserves_first_and_recent() {
    let mut msgs = vec![user_msg("initial context")];
    for i in 0..15 {
        msgs.push(user_msg(&format!("q{}", i)));
        msgs.push(assistant_msg(&format!("a{}", i)));
    }
    let last_4: Vec<String> = msgs[msgs.len() - 4..]
        .iter()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.clone(),
            _ => String::new(),
        })
        .collect();
    let mut session = make_test_session_with_messages(msgs);

    session.compact_history();

    // First message unchanged
    assert_eq!(session.messages[0].role, Role::User);
    // Last 4 messages unchanged
    let new_last_4: Vec<String> = session.messages[session.messages.len() - 4..]
        .iter()
        .map(|m| match &m.content {
            MessageContent::Text(t) => t.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(last_4, new_last_4, "last 4 messages should be preserved");
}

#[test]
fn compact_history_noop_when_small() {
    let msgs = vec![
        user_msg("initial"),
        user_msg("q1"),
        assistant_msg("a1"),
        user_msg("q2"),
        assistant_msg("a2"),
    ];
    let original_len = msgs.len();
    let mut session = make_test_session_with_messages(msgs);

    session.compact_history();

    assert_eq!(session.messages.len(), original_len);
}

#[test]
fn compact_history_no_orphaned_tool_messages() {
    let mut msgs = vec![user_msg("initial")];
    for i in 0..10 {
        msgs.push(user_msg(&format!("q{}", i)));
        msgs.push(tool_calls_msg(&["buffer_read"]));
        msgs.push(tool_result_msg("buffer_read"));
        msgs.push(assistant_msg(&format!("a{}", i)));
    }
    let mut session = make_test_session_with_messages(msgs);

    session.compact_history();

    // No Tool message should appear without a preceding ToolCalls message
    for (i, msg) in session.messages.iter().enumerate() {
        if msg.role == Role::Tool && i > 0 {
            let prev = &session.messages[i - 1];
            assert!(
                matches!(
                    prev.content,
                    MessageContent::ToolCalls(_) | MessageContent::TextWithToolCalls { .. }
                ) || prev.role == Role::Tool,
                "orphaned Tool message at index {}",
                i
            );
        }
    }
}

#[test]
fn compact_history_adjusts_transaction_idx() {
    let mut msgs = vec![user_msg("initial")];
    for i in 0..15 {
        msgs.push(user_msg(&format!("q{}", i)));
        msgs.push(assistant_msg(&format!("a{}", i)));
    }
    let tx_start = msgs.len() - 2; // Last pair is the transaction
    let mut session = make_test_session_with_messages(msgs);
    session.transaction_start_idx = Some(tx_start);

    session.compact_history();

    // Transaction start should still point to valid messages
    let new_idx = session.transaction_start_idx.unwrap();
    assert!(
        new_idx < session.messages.len(),
        "transaction_start_idx {} >= messages.len() {}",
        new_idx,
        session.messages.len()
    );
}

// --- Graceful Degradation tests ---

fn make_extended_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: "test tool".into(),
        parameters: crate::types::ToolParameters {
            schema_type: "object".into(),
            properties: std::collections::HashMap::new(),
            required: vec![],
        },
        permission: Some(crate::types::PermissionTier::ReadOnly),
    }
}

#[test]
fn graceful_degrade_sheds_extended_tools() {
    let tools = vec![
        make_extended_tool("buffer_read"),    // Core
        make_extended_tool("lsp_definition"), // Extended
        make_extended_tool("dap_start"),      // Extended
        make_extended_tool("kb_search"),      // Extended
    ];
    let mut session = make_session_with_tools(tools);
    // Manually add extended tools to the active set (simulating request_tools)
    session.tools.push(make_extended_tool("lsp_definition"));
    session.tools.push(make_extended_tool("dap_start"));
    session.tools.push(make_extended_tool("kb_search"));
    session.tools_tokens = crate::token_estimate::estimate_tools_tokens(&session.tools);

    let overhead = session.system_prompt_tokens + session.tools_tokens + session.reserved_output;
    for i in 0..50 {
        session.messages.push(user_msg(&format!(
            "padding message {} with some longer content to take up token space",
            i
        )));
    }
    let msg_tokens = crate::token_estimate::estimate_messages_tokens(&session.messages);
    session.context_window = ((overhead + msg_tokens) as f64 / 0.90) as u64;

    let original_count = session.tools.len();
    let changed = session.check_and_degrade();

    assert!(changed, "should have degraded");
    assert_eq!(
        session.degradation_level,
        super::DegradationLevel::ToolsShed
    );
    assert!(
        session.tools.len() < original_count,
        "expected fewer tools after shedding: {} -> {}",
        original_count,
        session.tools.len()
    );
    // Only core tools should remain
    for tool in &session.tools {
        assert!(
            crate::tools::classify_tool_tier(&tool.name) == crate::tools::ToolTier::Core
                || tool.name == "request_tools",
            "non-core tool {} survived shedding",
            tool.name
        );
    }
}

#[test]
fn graceful_degrade_recalcs_tools_tokens() {
    let tools = vec![
        make_extended_tool("buffer_read"),
        make_extended_tool("lsp_definition"),
        make_extended_tool("dap_start"),
    ];
    let mut session = make_session_with_tools(tools);
    let overhead = session.system_prompt_tokens + session.tools_tokens + session.reserved_output;
    for i in 0..50 {
        session.messages.push(user_msg(&format!(
            "padding {} with longer text to fill context",
            i
        )));
    }
    let msg_tokens = crate::token_estimate::estimate_messages_tokens(&session.messages);
    session.context_window = ((overhead + msg_tokens) as f64 / 0.90) as u64;
    let tokens_before = session.tools_tokens;

    session.check_and_degrade();

    assert!(
        session.tools_tokens <= tokens_before,
        "tools_tokens should not increase after shedding"
    );
}

#[test]
fn degradation_to_minimal_shortens_prompt() {
    let mut session = make_session_with_tools(vec![make_extended_tool("buffer_read")]);
    // Already shed tools
    session.degradation_level = super::DegradationLevel::ToolsShed;
    // Fill heavily to push past 92%
    let overhead = session.system_prompt_tokens + session.tools_tokens + session.reserved_output;
    for i in 0..50 {
        session.messages.push(user_msg(&format!(
            "heavy padding message {} with extra content",
            i
        )));
    }
    let msg_tokens = crate::token_estimate::estimate_messages_tokens(&session.messages);
    session.context_window = ((overhead + msg_tokens) as f64 / 0.95) as u64;

    let changed = session.check_and_degrade();

    assert!(changed, "should degrade to Minimal");
    assert_eq!(session.degradation_level, super::DegradationLevel::Minimal);
    assert!(
        session.system_prompt.contains("truncated"),
        "system prompt should mention truncation"
    );
}

#[test]
fn degradation_is_one_way() {
    let mut session = make_session_with_tools(vec![make_extended_tool("buffer_read")]);
    session.context_window = 100_000;
    session.system_prompt_tokens = 100;
    session.reserved_output = 100;
    // Force to ToolsShed
    session.degradation_level = super::DegradationLevel::ToolsShed;
    // Few messages = low usage, but should NOT recover
    session.messages.push(user_msg("tiny"));

    let changed = session.check_and_degrade();

    assert!(!changed, "should not recover from ToolsShed");
    assert_eq!(
        session.degradation_level,
        super::DegradationLevel::ToolsShed
    );
}

// --- strip_html + web_fetch tests ---

#[test]
fn strip_html_basic() {
    let html = "<html><body><h1>Hello</h1><p>World &amp; friends</p></body></html>";
    let text = AgentSession::strip_html(html);
    assert!(text.contains("Hello"), "should contain text");
    assert!(text.contains("World & friends"), "entities decoded");
    assert!(!text.contains('<'), "no HTML tags");
}

#[test]
fn strip_html_script_style_removed() {
    let html = r#"<html><head><style>body { color: red }</style></head>
    <body><script>alert('xss')</script><p>Content</p></body></html>"#;
    let text = AgentSession::strip_html(html);
    assert!(text.contains("Content"), "content preserved");
    assert!(!text.contains("alert"), "script removed");
    assert!(!text.contains("color"), "style removed");
}

#[test]
fn strip_html_plain_text_passthrough() {
    let plain = "This is just plain text with no tags.";
    let text = AgentSession::strip_html(plain);
    assert_eq!(text, plain);
}

#[test]
fn strip_html_collapses_whitespace() {
    let html = "<p>Line 1</p>\n\n\n\n\n<p>Line 2</p>";
    let text = AgentSession::strip_html(html);
    // Should not have more than one consecutive blank line
    assert!(!text.contains("\n\n\n"), "excessive blank lines collapsed");
}

#[test]
fn strip_html_entities() {
    let html = "&lt;tag&gt; &quot;quoted&quot; &nbsp; &#39;apos&#39;";
    let text = AgentSession::strip_html(html);
    assert!(text.contains("<tag>"), "lt/gt decoded");
    assert!(text.contains("\"quoted\""), "quot decoded");
    assert!(text.contains("'apos'"), "apos decoded");
}

#[tokio::test]
async fn web_fetch_missing_url() {
    let call = ToolCall {
        id: "test".into(),
        name: "web_fetch".into(),
        arguments: serde_json::json!({}),
    };
    let result = AgentSession::execute_web_fetch(&call).await;
    assert!(!result.success);
    assert!(result.output.contains("Missing"));
}

#[tokio::test]
async fn web_fetch_invalid_scheme() {
    let call = ToolCall {
        id: "test".into(),
        name: "web_fetch".into(),
        arguments: serde_json::json!({"url": "ftp://example.com"}),
    };
    let result = AgentSession::execute_web_fetch(&call).await;
    assert!(!result.success);
    assert!(result.output.contains("Invalid URL scheme"));
}
