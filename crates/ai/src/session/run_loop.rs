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
                    tool_name: call.name.clone(),
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
    pub(super) async fn update_cost(&mut self, response: &ProviderResponse) {
        let Some(usage) = response.usage else { return };
        self.session_tokens_in += usage.prompt_tokens;
        self.session_tokens_out += usage.completion_tokens;
        let last_call_usd = match self.price {
            Some(price) => {
                let c = price.cost_usd(&usage);
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
}
