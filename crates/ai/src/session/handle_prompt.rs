use tracing::{debug, error, info, warn};

use crate::provider::*;
use crate::token_estimate;
use crate::types::*;

use super::AgentSession;

impl AgentSession {
    pub(super) async fn handle_prompt(&mut self, prompt: String) {
        // Session initialization: emit context info on first prompt
        if !self.initialized {
            self.initialized = true;
            let budget = self.available_message_budget();
            info!(
                model = %self.model_name,
                context_window = self.context_window,
                system_prompt_tokens = self.system_prompt_tokens,
                tools_tokens = self.tools_tokens,
                tool_count = self.tools.len(),
                available_budget = budget,
                "AI session initialized"
            );
            let status = format!(
                "AI: {}, {}K context, {}K available, {} tools",
                if self.model_name.is_empty() {
                    "unknown"
                } else {
                    &self.model_name
                },
                self.context_window / 1000,
                budget / 1000,
                self.tools.len(),
            );
            let _ = self
                .event_tx
                .send(AiEvent::TextResponse {
                    text: format!("[{}]", status),
                    target_buffer: self.target_buffer.clone(),
                })
                .await;
        }

        // Auto-detect self-test mode from prompt content (`:self-test` sends this
        // through the normal session, not the --self-test CLI path).
        if !self.is_self_test
            && prompt.contains("self_test_suite")
            && prompt.contains("MAE Self-Test")
        {
            info!("auto-detected self-test prompt — enabling self-test mode");
            self.is_self_test = true;
            self.progress = super::progress::ProgressTracker::new(15, true);
        }

        self.transaction_start_idx = Some(self.messages.len());
        // Inject current mode/profile context so the model knows its constraints
        let workflow_ctx = if self.workflow.is_active() {
            format!("\n{}", self.workflow.context_injection())
        } else {
            String::new()
        };
        let contextualized_prompt = format!(
            "[Context: mode={}, profile={}]{}\n\n{}",
            self.current_mode, self.current_profile, workflow_ctx, prompt
        );
        self.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(contextualized_prompt),
        });
        self.trim_messages();

        let mut round = 0;
        loop {
            // Check for cancellation (e.g. double-esc from user)
            if let Ok(cmd) = self.command_rx.try_recv() {
                match cmd {
                    AiCommand::Cancel => {
                        info!("AI cancel received during tool loop — interrupting");
                        self.messages.push(Message {
                            role: Role::User,
                            content: MessageContent::Text("[Interrupted by user]".into()),
                        });
                        let _ = self
                            .event_tx
                            .send(AiEvent::TextResponse {
                                text: "[Interrupted by user]".into(),
                                target_buffer: self.target_buffer.clone(),
                            })
                            .await;
                        let _ = self
                            .event_tx
                            .send(AiEvent::SessionComplete {
                                text: "[Interrupted by user]".into(),
                                target_buffer: self.target_buffer.clone(),
                                transcript_path: self.transcript_path_str.clone(),
                            })
                            .await;
                        self.finalize_transaction();
                        return;
                    }
                    AiCommand::Shutdown => {
                        info!("AI shutdown received during tool loop");
                        return;
                    }
                    AiCommand::Prompt(p) => {
                        // If we get a new prompt while busy, we'll assume it's
                        // an follow-up and append it to the context.
                        info!(
                            prompt_len = p.len(),
                            "received AI follow-up prompt during tool loop"
                        );
                        self.messages.push(Message {
                            role: Role::User,
                            content: MessageContent::Text(p),
                        });
                    }
                }
            }

            // Enforce max_rounds to prevent runaway tool loops
            if round >= self.max_rounds {
                let msg = format!(
                    "AI reached maximum rounds ({}) — stopping to prevent runaway loop",
                    self.max_rounds
                );
                warn!(max_rounds = self.max_rounds, "round limit reached");
                let _ = self
                    .event_tx
                    .send(AiEvent::Error(msg, self.transcript_path_str.clone()))
                    .await;
                self.finalize_transaction();
                return;
            }

            // Progress checkpoint evaluation
            if round > 0 && round % self.progress.checkpoint_interval == 0 {
                use super::progress::CheckpointVerdict;
                match self.progress.evaluate() {
                    CheckpointVerdict::Continue => {
                        debug!(round, "progress checkpoint: good progress");
                    }
                    CheckpointVerdict::Warn { message } => {
                        warn!(round, %message, "progress checkpoint warning");
                        let _ = self
                            .event_tx
                            .send(AiEvent::TextResponse {
                                text: format!("[{}]", message),
                                target_buffer: self.target_buffer.clone(),
                            })
                            .await;
                    }
                    CheckpointVerdict::Abort { message } => {
                        warn!(round, %message, "progress checkpoint abort");
                        let _ = self
                            .event_tx
                            .send(AiEvent::Error(message, self.transcript_path_str.clone()))
                            .await;
                        self.finalize_transaction();
                        return;
                    }
                }
            }

            self.current_round = round;
            let _ = self
                .event_tx
                .send(AiEvent::RoundUpdate {
                    round: self.current_round,
                    transaction_start_idx: self.transaction_start_idx,
                })
                .await;

            // --- Mid-Flight Context Compaction ---
            // If the active transaction has grown too large, collapse it into a reasoning summary
            // to free up tokens and prevent context overflow before the turn ends.
            // Self-test and active workflows use a higher threshold since they need
            // more rounds of tool results before summarization is safe.
            if let Some(start_idx) = self.transaction_start_idx {
                let transaction_size = self.messages.len().saturating_sub(start_idx);
                let current_tokens = token_estimate::estimate_messages_tokens(&self.messages);
                let window_usage = current_tokens as f64 / self.context_window as f64;
                let size_threshold = if self.is_self_test || self.workflow.is_active() {
                    50
                } else {
                    20
                };

                // Skip collapse when the workflow is complete — the agent just
                // needs to emit its final summary, and collapsing now would
                // erase the test results it needs to report on.
                let workflow_done = self.workflow.is_active() && self.workflow.is_complete();
                if (transaction_size > size_threshold || window_usage > 0.75)
                    && round > 0
                    && !workflow_done
                {
                    warn!(
                        transaction_size,
                        %window_usage,
                        size_threshold,
                        round,
                        messages = self.messages.len(),
                        is_self_test = self.is_self_test,
                        workflow_active = self.workflow.is_active(),
                        "mid-flight context compaction triggered"
                    );
                    self.log_transcript_event("collapse_transaction", &format!(
                        "transaction_size={}, window_usage={:.2}, threshold={}, round={}, messages={}",
                        transaction_size, window_usage, size_threshold, round, self.messages.len()
                    ));
                    self.collapse_transaction(start_idx);
                    // Point to the new summary message as the new transaction start,
                    // preserving continuity for the next round's pruning logic.
                    self.transaction_start_idx = Some(self.messages.len().saturating_sub(1));
                }
            }

            // History compaction: summarize old turns before hard trimming
            {
                let current_tokens = token_estimate::estimate_messages_tokens(&self.messages);
                let budget = self.available_message_budget();
                if current_tokens > budget * 70 / 100 && !self.is_self_test {
                    warn!(
                        current_tokens,
                        budget,
                        round,
                        messages = self.messages.len(),
                        "compact_history triggered"
                    );
                    self.log_transcript_event(
                        "compact_history",
                        &format!(
                            "tokens={}, budget={}, round={}, messages={}",
                            current_tokens,
                            budget,
                            round,
                            self.messages.len()
                        ),
                    );
                    self.compact_history();
                }
            }

            // Graceful degradation: shed tools/prompt under context pressure
            if self.check_and_degrade() {
                let warning = match self.degradation_level {
                    super::DegradationLevel::ToolsShed => {
                        "[Context pressure: extended tools disabled. Use core tools only.]"
                    }
                    super::DegradationLevel::Minimal => {
                        "[Context pressure: minimal mode. System prompt shortened.]"
                    }
                    super::DegradationLevel::Normal => unreachable!(),
                };
                self.messages.push(Message {
                    role: Role::User,
                    content: MessageContent::Text(warning.to_string()),
                });
                let _ = self
                    .event_tx
                    .send(AiEvent::TextResponse {
                        text: warning.to_string(),
                        target_buffer: self.target_buffer.clone(),
                    })
                    .await;
            }

            debug!(round, "AI provider send");

            // Dynamic context-aware loop break: stop if we are running out of context
            // before the provider even errors out.
            let current_tokens = token_estimate::estimate_messages_tokens(&self.messages);
            if current_tokens > self.context_window.saturating_sub(self.reserved_output) {
                warn!(
                    current_tokens,
                    context_window = self.context_window,
                    "breaking tool loop: context nearly full"
                );
                let _ = self
                    .event_tx
                    .send(AiEvent::Error(
                        "Context window nearly full — stopping tool calls".into(),
                        self.transcript_path_str.clone(),
                    ))
                    .await;
                self.finalize_transaction();
                return;
            }

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
                    self.finalize_transaction();
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

            // Token-aware trim before every provider call (not just the first)
            self.trim_messages();

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
                        kind = ?e.kind,
                        consecutive_errors = self.consecutive_errors,
                        "AI provider error"
                    );

                    // Context overflow recovery: aggressively prune and retry once
                    if e.kind == ErrorKind::ContextOverflow {
                        warn!("context overflow detected — pruning old messages and retrying");
                        // Reduce context window by 20% for self-healing (discovery of actual tighter limits)
                        self.context_window = (self.context_window * 4 / 5).max(4000);

                        let _ = self
                            .event_tx
                            .send(AiEvent::Error(
                                "Context window full — pruning old messages, retrying...".into(),
                                self.transcript_path_str.clone(),
                            ))
                            .await;
                        self.aggressive_prune();
                        self.trim_messages();
                        // Retry once
                        match self
                            .provider
                            .send(&self.messages, &self.tools, &self.system_prompt)
                            .await
                        {
                            Ok(r) => {
                                self.consecutive_errors = 0;
                                self.update_cost(&r).await;
                                r
                            }
                            Err(retry_err) => {
                                let _ = self
                                    .event_tx
                                    .send(AiEvent::Error(
                                        format!(
                                            "Context overflow recovery failed: {}",
                                            retry_err.message
                                        ),
                                        self.transcript_path_str.clone(),
                                    ))
                                    .await;
                                self.finalize_transaction();
                                return;
                            }
                        }
                    }
                    // Circuit breaker: if retryable and under threshold, backoff and retry
                    else if e.retryable && self.consecutive_errors <= 3 {
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
                    } else {
                        let _ = self
                            .event_tx
                            .send(AiEvent::Error(e.message, self.transcript_path_str.clone()))
                            .await;
                        self.finalize_transaction();
                        return;
                    }
                }
            };

            if !response.tool_calls.is_empty() {
                // Signature: sorted tool names + arguments for robust comparison
                let mut names: Vec<String> = response
                    .tool_calls
                    .iter()
                    .map(|c| format!("{}:{}", c.name, c.arguments))
                    .collect();
                names.sort();
                let turn_sig = names.join("|");

                // Oscillating Loop Detection: count how many times this turn signature
                // appears in the recent history window. Catches both strict repeats
                // (A->A->A) and oscillating patterns (A->B->A->B).
                // Softened: first detection is a warning; abort only after stagnant threshold.
                let repeat_count = self.turn_history.iter().filter(|s| *s == &turn_sig).count();
                if repeat_count >= 2 {
                    self.progress.increment_stagnant();
                    if self.progress.should_abort_stagnant() {
                        let _ = self
                            .event_tx
                            .send(AiEvent::Error(
                                format!(
                                    "AI got stuck in a tool loop (same pattern seen {} times in last {} turns, stagnant {}/{}) — aborting",
                                    repeat_count + 1,
                                    self.turn_history.len(),
                                    self.progress.stagnant_count(),
                                    self.progress.max_stagnant(),
                                ),
                                self.transcript_path_str.clone(),
                            ))
                            .await;
                        self.finalize_transaction();
                        return;
                    }
                    // Warning: oscillation detected but not yet at abort threshold
                    let msg = format!(
                        "AI tool loop warning: same pattern seen {} times (stagnant {}/{})",
                        repeat_count + 1,
                        self.progress.stagnant_count(),
                        self.progress.max_stagnant(),
                    );
                    warn!(%msg, "oscillation detected");
                    let _ = self
                        .event_tx
                        .send(AiEvent::TextResponse {
                            text: format!("[{}]", msg),
                            target_buffer: self.target_buffer.clone(),
                        })
                        .await;
                }

                if self.turn_history.len() >= 6 {
                    self.turn_history.pop_front();
                }
                self.turn_history.push_back(turn_sig);
                self.last_tool_calls = Some(response.tool_calls.clone());
            }

            // Auto-save turn to transcript
            if let Some(ref path) = self.transcript_path {
                if let Ok(json) = serde_json::to_string_pretty(&response) {
                    let file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path);
                    if let Ok(mut f) = file {
                        let _ = std::io::Write::write_all(&mut f, b"\n--- RESPONSE ---\n");
                        let _ = std::io::Write::write_all(&mut f, json.as_bytes());
                    }
                }
            }

            // Send text response if present
            if let Some(ref text) = response.text {
                let _ = self
                    .event_tx
                    .send(AiEvent::TextResponse {
                        text: text.clone(),
                        target_buffer: self.target_buffer.clone(),
                    })
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
                    .send(AiEvent::SessionComplete {
                        text: final_text,
                        target_buffer: self.target_buffer.clone(),
                        transcript_path: self.transcript_path_str.clone(),
                    })
                    .await;
                self.finalize_transaction();
                return;
            }

            let mut seen_calls = std::collections::HashSet::new();
            let mut deduplicated_calls = Vec::new();
            for call in &response.tool_calls {
                let sig = format!("{}:{}", call.name, call.arguments);
                if seen_calls.insert(sig) {
                    deduplicated_calls.push(call.clone());
                } else {
                    debug!(tool = %call.name, "dropped identical parallel tool call");
                }
            }

            // Record assistant message with tool calls (preserve text if present)
            let content = if let Some(text) = response.text.clone() {
                MessageContent::TextWithToolCalls {
                    text,
                    tool_calls: deduplicated_calls.clone(),
                }
            } else {
                MessageContent::ToolCalls(deduplicated_calls.clone())
            };
            self.messages.push(Message {
                role: Role::Assistant,
                content,
            });

            // Execute each tool call
            for call in &deduplicated_calls {
                // UI Notification: tool execution started
                let _ = self
                    .event_tx
                    .send(AiEvent::ToolCallStarted {
                        name: call.name.clone(),
                    })
                    .await;

                // request_tools meta-tool: extend the active tool set
                if call.name == "request_tools" {
                    let categories_str = call
                        .arguments
                        .get("categories")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let categories = crate::tools::parse_categories(categories_str);
                    let mut added_names = Vec::new();
                    for cat in &categories {
                        if self.enabled_categories.insert(*cat) {
                            // Add tools from this category
                            for tool in &self.all_tools {
                                if crate::tools::classify_tool_category(&tool.name) == Some(*cat)
                                    && !self.tools.iter().any(|t| t.name == tool.name)
                                {
                                    added_names.push(tool.name.clone());
                                    self.tools.push(tool.clone());
                                }
                            }
                        }
                    }
                    // Recache tools token estimate
                    self.tools_tokens = token_estimate::estimate_tools_tokens(&self.tools);
                    let output = if added_names.is_empty() {
                        "No new tools added (categories already enabled or not recognized).".into()
                    } else {
                        format!(
                            "Added {} tools: {}",
                            added_names.len(),
                            added_names.join(", ")
                        )
                    };
                    info!(categories = %categories_str, added = added_names.len(), "request_tools");
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: true,
                            output: output.clone(),
                        })
                        .await;
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(ToolResult {
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            success: true,
                            output,
                        }),
                    });
                    continue;
                }

                // read_transcript allows the AI to see its own raw logs
                if call.name == "read_transcript" {
                    let output = if let Some(ref path) = self.transcript_path {
                        match std::fs::read_to_string(path) {
                            Ok(s) => s,
                            Err(e) => format!("Failed to read transcript file: {}", e),
                        }
                    } else {
                        "Transcript logging is disabled for this session".into()
                    };
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(ToolResult {
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            success: true,
                            output,
                        }),
                    });
                    continue;
                }

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
                    let mut result = Self::execute_shell(call).await;
                    debug!(
                        tool = "shell_exec",
                        success = result.success,
                        "shell command complete"
                    );
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: result.success,
                            output: result.output.clone(),
                        })
                        .await;
                    self.truncate_tool_result(&mut result);
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                // web_fetch runs async on this task — no need to cross to main thread
                if call.name == "web_fetch" {
                    let mut result = Self::execute_web_fetch(call).await;
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: result.success,
                            output: if result.output.len() > 200 {
                                format!("{}...", &result.output[..200])
                            } else {
                                result.output.clone()
                            },
                        })
                        .await;
                    self.truncate_tool_result(&mut result);
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                // log_activity (UI only reasoning step)
                if call.name == "log_activity" {
                    let activity = call
                        .arguments
                        .get("activity")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Thinking...");
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: true,
                            output: activity.to_string(),
                        })
                        .await;
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(ToolResult {
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            success: true,
                            output: activity.into(),
                        }),
                    });
                    continue;
                }

                // ai_set_mode requests the editor to switch operating modes
                if call.name == "ai_set_mode" {
                    let mode = call.arguments["mode"]
                        .as_str()
                        .unwrap_or("standard")
                        .to_string();
                    self.current_mode = mode.clone();
                    let _ = self.event_tx.send(AiEvent::UpdateMode(mode.clone())).await;
                    let result_text = format!("AI mode change requested: {}", mode);
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: true,
                            output: result_text.clone(),
                        })
                        .await;
                    let result = ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: true,
                        output: result_text,
                    };
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                // ai_set_profile requests the editor to switch prompt personas
                if call.name == "ai_set_profile" {
                    let profile = call.arguments["profile"]
                        .as_str()
                        .unwrap_or("pair-programmer")
                        .to_string();
                    self.current_profile = profile.clone();
                    let _ = self
                        .event_tx
                        .send(AiEvent::UpdateProfile(profile.clone()))
                        .await;
                    let result_text = format!("AI profile change requested: {}", profile);
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: true,
                            output: result_text.clone(),
                        })
                        .await;
                    let result = ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: true,
                        output: result_text,
                    };
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                // ai_set_budget updates session state directly
                if call.name == "ai_set_budget" {
                    if let Some(warn) = call.arguments.get("warn").and_then(|v| v.as_f64()) {
                        self.budget.session_warn_usd = if warn > 0.0 { Some(warn) } else { None };
                        self.warned = false; // Reset warning state
                    }
                    if let Some(cap) = call.arguments.get("cap").and_then(|v| v.as_f64()) {
                        self.budget.session_hard_cap_usd = if cap > 0.0 { Some(cap) } else { None };
                    }
                    let result_text = format!("Budget updated: {:?}", self.budget);
                    let _ = self
                        .event_tx
                        .send(AiEvent::ToolCallFinished {
                            success: true,
                            output: result_text.clone(),
                        })
                        .await;
                    let result = ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: true,
                        output: result_text,
                    };
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: MessageContent::ToolResult(result),
                    });
                    continue;
                }

                // ask_user pauses the session and waits for a string reply
                if call.name == "ask_user" {
                    let question = call.arguments["question"]
                        .as_str()
                        .unwrap_or("No question provided")
                        .to_string();
                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                    let _ = self
                        .event_tx
                        .send(AiEvent::AskUser {
                            question,
                            reply: reply_tx,
                        })
                        .await;
                    match reply_rx.await {
                        Ok(reply) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: true,
                                    output: reply.clone(),
                                })
                                .await;
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(ToolResult {
                                    tool_call_id: call.id.clone(),
                                    tool_name: call.name.clone(),
                                    success: true,
                                    output: reply,
                                }),
                            });
                        }
                        Err(_) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: false,
                                    output: "User canceled or request failed".into(),
                                })
                                .await;
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(ToolResult {
                                    tool_call_id: call.id.clone(),
                                    tool_name: call.name.clone(),
                                    success: false,
                                    output: "User canceled or request failed".into(),
                                }),
                            });
                        }
                    }
                    continue;
                }

                // propose_changes pauses and waits for a boolean approval
                if call.name == "propose_changes" {
                    let changes = call.arguments["changes"].clone();
                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                    let _ = self
                        .event_tx
                        .send(AiEvent::ProposeChanges {
                            changes,
                            reply: reply_tx,
                        })
                        .await;
                    match reply_rx.await {
                        Ok(approved) => {
                            let output = if approved {
                                "Changes approved and applied"
                            } else {
                                "Changes rejected by user"
                            };
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: approved,
                                    output: output.into(),
                                })
                                .await;
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(ToolResult {
                                    tool_call_id: call.id.clone(),
                                    tool_name: call.name.clone(),
                                    success: approved,
                                    output: output.into(),
                                }),
                            });
                        }
                        Err(_) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: false,
                                    output: "User canceled or request failed".into(),
                                })
                                .await;
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(ToolResult {
                                    tool_call_id: call.id.clone(),
                                    tool_name: call.name.clone(),
                                    success: false,
                                    output: "User canceled or request failed".into(),
                                }),
                            });
                        }
                    }
                    continue;
                }

                // delegate pauses and waits for a ToolResult from a sub-agent
                if call.name == "delegate" {
                    let profile = call.arguments["profile"]
                        .as_str()
                        .unwrap_or("pair-programmer")
                        .to_string();
                    let objective = call.arguments["objective"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                    let _ = self
                        .event_tx
                        .send(AiEvent::Delegate {
                            profile,
                            objective,
                            reply: reply_tx,
                        })
                        .await;
                    match reply_rx.await {
                        Ok(mut result) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: result.success,
                                    output: result.output.clone(),
                                })
                                .await;
                            self.truncate_tool_result(&mut result);
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(result),
                            });
                        }
                        Err(_) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::ToolCallFinished {
                                    success: false,
                                    output: "Sub-agent delegation failed".into(),
                                })
                                .await;
                            self.messages.push(Message {
                                role: Role::Tool,
                                content: MessageContent::ToolResult(ToolResult {
                                    tool_call_id: call.id.clone(),
                                    tool_name: call.name.clone(),
                                    success: false,
                                    output: "Sub-agent delegation failed".into(),
                                }),
                            });
                        }
                    }
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
                        .send(AiEvent::Error("Event channel closed".into(), None))
                        .await;
                    self.finalize_transaction();
                    return;
                }

                match reply_rx.await {
                    Ok(mut result) => {
                        debug!(tool = %call.name, success = result.success, "tool result received");

                        // Workflow: auto-activate on first self_test_suite call,
                        // hard-block re-calls (return error instead of plan).
                        if call.name == "self_test_suite" {
                            if self.workflow.plan_request_count == 0 {
                                let steps = extract_self_test_categories(&result.output);
                                if !steps.is_empty() {
                                    self.workflow.start_workflow("self-test".into(), steps);
                                    info!("workflow tracker activated for self-test");
                                }
                            } else {
                                // Hard-block: replace the result with an error.
                                // Must be unmistakable — small models misread subtle messages.
                                result.success = false;
                                let wf = self.workflow.context_injection();
                                let directive = if self.workflow.is_complete() {
                                    "ALL TESTS ARE ALREADY COMPLETE. Do NOT re-run any tests. \
                                     Output the final === MAE Self-Test Report === now."
                                        .to_string()
                                } else {
                                    format!(
                                        "Continue testing from the CURRENT step. Do NOT restart from the beginning. \
                                         Next step: '{}'",
                                        self.workflow.current_step_name()
                                    )
                                };
                                result.output = format!(
                                    "BLOCKED: self_test_suite cannot be called again (called {} times already).\n\
                                     {}\n\n\
                                     ACTION REQUIRED: {}\n\
                                     Do NOT call self_test_suite again. It will always return this error.",
                                    self.workflow.plan_request_count,
                                    wf,
                                    directive
                                );
                                warn!(
                                    count = self.workflow.plan_request_count,
                                    "self_test_suite re-called — hard-blocked"
                                );
                            }
                            self.workflow.plan_request_count += 1;
                        }

                        let _ = self
                            .event_tx
                            .send(AiEvent::ToolCallFinished {
                                success: result.success,
                                output: result.output.clone(),
                            })
                            .await;
                        self.truncate_tool_result(&mut result);
                        self.messages.push(Message {
                            role: Role::Tool,
                            content: MessageContent::ToolResult(result),
                        });
                    }
                    Err(_) => {
                        error!(tool = %call.name, "tool result channel closed");
                        let _ = self
                            .event_tx
                            .send(AiEvent::ToolCallFinished {
                                success: false,
                                output: "Tool result channel closed".into(),
                            })
                            .await;
                        let _ = self
                            .event_tx
                            .send(AiEvent::Error("Tool result channel closed".into(), None))
                            .await;
                        self.finalize_transaction();
                        return;
                    }
                }
            }
            // Record tool calls for progress tracking
            for call in &deduplicated_calls {
                // Check the last Tool message for this call's success status
                let success = self
                    .messages
                    .iter()
                    .rev()
                    .find_map(|m| {
                        if let MessageContent::ToolResult(r) = &m.content {
                            if r.tool_name == call.name {
                                return Some(r.success);
                            }
                        }
                        None
                    })
                    .unwrap_or(false);
                self.progress
                    .record_tool_call(&call.name, &call.arguments, success);

                // --- Workflow tracking ---
                self.handle_workflow_tool_result(call, success);
            }
            self.progress.record_round();

            // Loop: provider sees tool results and may issue more calls
            round += 1;
        }
    }

    /// Collapse intermediate tool calls and results in the current transaction
    /// into a single reasoning/summary message.
    ///
    /// Preserves: System prompt, User prompt, Final response text.
    /// Condenses: Tool calls, tool results, intermediate reasoning text.
    pub(super) fn collapse_transaction(&mut self, start_idx: usize) {
        if self.messages.len() <= start_idx + 1 {
            return;
        }

        // Only treat the last message as the final response if it's an Assistant text message.
        // If the loop was aborted (context full, error, etc), the last message might be
        // a Tool result or an Assistant message with tool calls — those must be collapsed.
        let mut final_response = None;
        if let Some(last) = self.messages.last() {
            if last.role == Role::Assistant {
                if let MessageContent::Text(_) = last.content {
                    final_response = self.messages.pop();
                }
            }
        }

        let to_drain = self.messages.len().saturating_sub(start_idx + 1);
        if to_drain == 0 {
            if let Some(resp) = final_response {
                self.messages.push(resp);
            }
            return;
        }

        let drained: Vec<Message> = self.messages.drain(start_idx + 1..).collect();

        let mut tool_count = 0;
        let mut reasoning = Vec::new();
        for msg in drained {
            match &msg.content {
                MessageContent::Text(t) => {
                    if msg.role == Role::Assistant {
                        reasoning.push(t.clone());
                    }
                }
                MessageContent::TextWithToolCalls { text, tool_calls } => {
                    reasoning.push(text.clone());
                    tool_count += tool_calls.len();
                }
                MessageContent::ToolCalls(calls) => {
                    tool_count += calls.len();
                }
                MessageContent::ToolResult(_) => {}
            }
        }

        // If we have reasoning text or tool operations, insert a single compressed reasoning message
        if !reasoning.is_empty() || tool_count > 0 {
            let mut summary = String::new();
            if !reasoning.is_empty() {
                summary.push_str("Thought process:\n");
                for r in &reasoning {
                    summary.push_str("- ");
                    summary.push_str(r);
                    summary.push('\n');
                }
            }
            if tool_count > 0 {
                summary.push_str(&format!(
                    "\n(Assistant performed {} tool operations)",
                    tool_count
                ));
            }

            // Preserve workflow progress in the collapsed summary so even
            // reading back collapsed history shows the checkpoint.
            if self.workflow.is_active() {
                let next = self.workflow.current_step_name();
                let next_directive = if next.is_empty() {
                    "All steps complete. Produce your final summary report.".to_string()
                } else {
                    format!(
                        "Your next action: execute the '{}' test category. \
                         Do NOT call self_test_suite — you already have the plan.",
                        next
                    )
                };
                summary.push_str(&format!(
                    "\n\n{}\n\n{}",
                    self.workflow.context_injection(),
                    next_directive
                ));
            }

            self.messages.push(Message {
                role: Role::Assistant,
                content: MessageContent::Text(summary),
            });
        }

        let has_final = final_response.is_some();
        if let Some(resp) = final_response {
            self.messages.push(resp);
        }

        debug!(
            original_len = to_drain + (if has_final { 1 } else { 0 }),
            new_len = if reasoning.is_empty() && tool_count == 0 {
                0
            } else {
                1
            } + (if has_final { 1 } else { 0 }),
            "collapsed tool callstack"
        );
    }

    /// Log a structured event to the transcript file for debugging.
    /// Written as a JSON line so it's parseable alongside provider responses.
    pub(super) fn log_transcript_event(&self, event: &str, detail: &str) {
        if let Some(ref path) = self.transcript_path {
            let entry = format!(
                "\n--- EVENT ---\n{{\"event\": \"{}\", \"detail\": \"{}\", \"round\": {}, \"workflow\": {}}}\n",
                event,
                detail.replace('"', "'"),
                self.current_round,
                if self.workflow.is_active() {
                    self.workflow.context_injection().replace('"', "'").replace('\n', " ")
                } else {
                    "inactive".to_string()
                }
            );
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path);
            if let Ok(mut f) = file {
                let _ = std::io::Write::write_all(&mut f, entry.as_bytes());
            }
        }
    }

    /// Process a tool result for workflow step advancement.
    ///
    /// For self-test workflows, detects when the agent transitions between
    /// test categories by classifying each tool call. When >=2 tools from
    /// the current step have been called and a tool from a different step
    /// appears, the current step is auto-advanced.
    fn handle_workflow_tool_result(&mut self, call: &ToolCall, _success: bool) {
        if !self.workflow.is_active() {
            return;
        }

        let workflow_name = match &self.workflow.workflow {
            Some(name) => name.clone(),
            None => return,
        };

        // Only do auto-advancement for self-test workflows
        if workflow_name != "self-test" {
            self.workflow.record_tool(&call.name);
            return;
        }

        // Classify which step this tool belongs to
        let tool_step = super::workflow::classify_tool_to_self_test_step(&call.name);

        // Skip tools that don't map to a known step (log_activity, shell_exec, etc.)
        let tool_step = match tool_step {
            Some(s) => s,
            None => {
                self.workflow.record_tool(&call.name);
                return;
            }
        };

        let current_step_name = self.workflow.current_step_name().to_string();

        // If the tool belongs to a different step than the current one,
        // and we've called >=2 tools in the current step, auto-advance.
        if tool_step != current_step_name && self.workflow.step_tools_called.len() >= 2 {
            let summary = format!("{} tools called", self.workflow.step_tools_called.len());
            info!(
                from = %current_step_name,
                to = %tool_step,
                "workflow auto-advancing step"
            );
            self.workflow.advance(summary);

            // If the new tool's step is ahead of where we are now,
            // skip intermediate steps to catch up.
            while self.workflow.current_step < self.workflow.steps.len() {
                if self.workflow.current_step_name() == tool_step {
                    break;
                }
                let skip_name = self.workflow.current_step_name().to_string();
                info!(step = %skip_name, "workflow skipping intermediate step");
                self.workflow.skip("skipped (agent jumped ahead)".into());
            }
        }

        self.workflow.record_tool(&call.name);

        // Timeout fallback: if same step has been active for >15 tool calls, auto-advance
        if self.workflow.step_tools_called.len() > 15 {
            warn!(
                step = %self.workflow.current_step_name(),
                tools = self.workflow.step_tools_called.len(),
                "workflow step timeout — auto-advancing"
            );
            self.workflow.fail("timeout (>15 tool calls)".into());
        }
    }
}

/// Extract category names from a self_test_suite JSON result.
/// Parses the "categories" array and returns the "name" field of each.
fn extract_self_test_categories(output: &str) -> Vec<String> {
    let parsed: serde_json::Value = match serde_json::from_str(output) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let categories = match parsed.get("categories").and_then(|c| c.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };
    categories
        .iter()
        .filter_map(|cat| cat.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect()
}
