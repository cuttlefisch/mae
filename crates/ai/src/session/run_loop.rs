use tracing::{info, warn};

use crate::provider::*;
use crate::types::*;

use super::AgentSession;

impl AgentSession {
    /// Execute shell_exec tool asynchronously on the AI task thread.
    ///
    /// Emacs lesson: Emacs's `shell-command` blocks the entire editor because
    /// process.c runs synchronously on the main thread. We run shell commands
    /// on the AI's spawned tokio task, so the editor remains responsive.
    ///
    /// Security: rejects commands containing dangerous patterns (rm -rf /,
    /// fork bombs, etc.) and caps timeout at 120 seconds.
    pub(super) async fn execute_shell(call: &ToolCall) -> ToolResult {
        let command = call
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if command.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: "Missing 'command' argument".into(),
            };
        }

        // Shared with the sync/MCP version — see `crate::shell_policy`.
        if let Some(pattern) = crate::shell_policy::blocked_pattern(command) {
            warn!(command, pattern, "blocked dangerous shell command");
            return ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: format!("Command blocked: contains dangerous pattern '{}'", pattern),
            };
        }

        let timeout = crate::shell_policy::timeout_from_args(&call.arguments);

        let result = tokio::time::timeout(
            timeout,
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
                    // Truncate to 10k bytes to avoid blowing up context. Shell
                    // command output is arbitrary UTF-8 (unicode filenames, git
                    // log messages, colored box-drawing, ...) -- ADR-087 /
                    // audit #594: a fixed byte cut can land mid-character and
                    // panic. `floor_char_boundary` rounds down.
                    let stdout_str = if stdout.len() > 10_000 {
                        let cut = mae_core::grapheme::floor_char_boundary(&stdout, 10_000);
                        format!("{}...[truncated]", &stdout[..cut])
                    } else {
                        stdout.to_string()
                    };
                    out.push_str(&format!("stdout:\n{}\n", stdout_str));
                }
                if !stderr.is_empty() {
                    let stderr_str = if stderr.len() > 5_000 {
                        let cut = mae_core::grapheme::floor_char_boundary(&stderr, 5_000);
                        format!("{}...[truncated]", &stderr[..cut])
                    } else {
                        stderr.to_string()
                    };
                    out.push_str(&format!("stderr:\n{}\n", stderr_str));
                }

                ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: output.status.success(),
                    output: out,
                }
            }
            Ok(Err(e)) => ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: format!("Failed to execute command: {}", e),
            },
            Err(_) => ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: format!(
                    "Command timed out after {}",
                    crate::shell_policy::describe_timeout(timeout)
                ),
            },
        }
    }

    /// Execute web_fetch tool asynchronously on the AI task thread.
    pub(super) async fn execute_web_fetch(call: &ToolCall) -> ToolResult {
        let url = call
            .arguments
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // ADR-091: scheme allow-list shared with the blocking transport
        // (`executor::session_exec::execute_web_fetch`) so the two cannot
        // disagree about what is fetchable.
        if let Err(e) = crate::web::validate_url(url) {
            return ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: e,
            };
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(crate::web::TIMEOUT_SECS))
            .user_agent(crate::web::USER_AGENT)
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    success: false,
                    output: format!("Failed to create HTTP client: {}", e),
                };
            }
        };

        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown")
                    .to_string();

                match response.text().await {
                    Ok(body) => ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: true,
                        // ADR-091: HTML stripping + truncation shared with
                        // the blocking transport.
                        output: crate::web::shape_body(status, &content_type, body),
                    },
                    Err(e) => ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: false,
                        output: format!("Failed to read response body: {}", e),
                    },
                }
            }
            Err(e) => ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: if e.is_timeout() {
                    "Request timed out after 30 seconds".into()
                } else {
                    format!("HTTP request failed: {}", e)
                },
            },
        }
    }

    /// Strip HTML tags, script/style blocks, and decode common entities.
    /// Main loop: wait for prompts, run agentic loop, send results.
    pub async fn run(mut self) {
        info!("AI session started, waiting for prompts");
        loop {
            match self.command_rx.recv().await {
                Some(AiCommand::Prompt(prompt)) => {
                    info!(prompt_len = prompt.len(), "received AI prompt");
                    self.handle_prompt(prompt).await;
                }
                Some(AiCommand::Delegate { profile, objective }) => {
                    // Direct delegate: emit as a Delegate event for the main
                    // thread to spawn a sub-agent. No LLM round-trip needed.
                    info!(%profile, "direct delegate command");
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
                        Ok(result) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::SessionComplete {
                                    text: result.output,
                                    target_buffer: self.target_buffer.clone(),
                                    transcript_path: self.transcript_path_str.clone(),
                                })
                                .await;
                        }
                        Err(_) => {
                            let _ = self
                                .event_tx
                                .send(AiEvent::Error(
                                    "Delegate sub-agent failed".into(),
                                    self.transcript_path_str.clone(),
                                ))
                                .await;
                        }
                    }
                }
                Some(AiCommand::PingNetwork { base_url }) => {
                    let provider =
                        crate::context_limits::ProviderHint::from_model(&self.model_name);
                    let result =
                        crate::connectivity::connectivity_check(base_url.as_deref(), provider)
                            .await;
                    let _ = self.event_tx.send(AiEvent::NetworkDiagnostic(result)).await;
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
    pub(super) async fn update_cost_with_latency(
        &mut self,
        response: &ProviderResponse,
        latency_ms: u64,
    ) {
        self.last_latency_ms = latency_ms;
        self.update_cost(response).await;
    }

    pub(super) async fn update_cost(&mut self, response: &ProviderResponse) {
        let Some(usage) = response.usage else { return };
        self.session_tokens_in += usage.prompt_tokens;
        self.session_tokens_out += usage.completion_tokens;
        self.session_cache_read += usage.cache_read_tokens;
        self.session_cache_creation += usage.cache_creation_tokens;
        let last_call_usd = match self.price {
            Some(price) => {
                let c = price.cost_usd(&usage);
                self.session_cost_usd += c;
                c
            }
            None => 0.0,
        };
        // Estimate current context usage for the dashboard
        let messages_tokens = crate::token_estimate::estimate_messages_tokens(&self.messages);
        let context_used_tokens =
            messages_tokens + self.system_prompt_tokens + self.tools_tokens + self.reserved_output;
        let _ = self
            .event_tx
            .send(AiEvent::CostUpdate {
                session_usd: self.session_cost_usd,
                last_call_usd,
                tokens_in: self.session_tokens_in,
                tokens_out: self.session_tokens_out,
                cache_read_tokens: self.session_cache_read,
                cache_creation_tokens: self.session_cache_creation,
                context_window: self.context_window,
                context_used_tokens,
                turn_tokens_in: usage.prompt_tokens,
                turn_tokens_out: usage.completion_tokens,
                turn_cache_read: usage.cache_read_tokens,
                latency_ms: self.last_latency_ms,
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
}
