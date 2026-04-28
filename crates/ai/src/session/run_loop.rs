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

    /// Execute web_fetch tool asynchronously on the AI task thread.
    pub(super) async fn execute_web_fetch(call: &ToolCall) -> ToolResult {
        let url = call
            .arguments
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if url.is_empty() {
            return ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: "Missing 'url' argument".into(),
            };
        }

        // Validate URL scheme
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: format!(
                    "Invalid URL scheme: only http:// and https:// are supported, got: {}",
                    url
                ),
            };
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("MAE/0.5.0")
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
                let is_html = content_type.contains("html");

                match response.text().await {
                    Ok(body) => {
                        let text = if is_html {
                            Self::strip_html(&body)
                        } else {
                            body
                        };
                        // Truncate to 32KB
                        let text = if text.len() > 32_768 {
                            let boundary = text.floor_char_boundary(32_768);
                            format!("{}...\n[truncated at 32KB]", &text[..boundary])
                        } else {
                            text
                        };
                        ToolResult {
                            tool_call_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            success: true,
                            output: format!("HTTP {} ({})\n\n{}", status, content_type, text),
                        }
                    }
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
    pub(super) fn strip_html(html: &str) -> String {
        let mut result = String::with_capacity(html.len() / 2);
        let mut in_tag = false;
        let mut in_script = false;
        let mut in_style = false;
        let mut chars = html.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '<' {
                // Check for script/style open/close tags
                let rest: String = chars.clone().take(20).collect();
                let rest_lower = rest.to_ascii_lowercase();
                if rest_lower.starts_with("script") {
                    in_script = true;
                } else if rest_lower.starts_with("/script") {
                    in_script = false;
                } else if rest_lower.starts_with("style") {
                    in_style = true;
                } else if rest_lower.starts_with("/style") {
                    in_style = false;
                }
                in_tag = true;
                continue;
            }
            if ch == '>' {
                in_tag = false;
                continue;
            }
            if in_tag || in_script || in_style {
                continue;
            }
            // Decode HTML entities
            if ch == '&' {
                let entity: String = chars
                    .clone()
                    .take_while(|c| *c != ';' && *c != ' ' && *c != '<')
                    .collect();
                if entity.len() < 10 {
                    let decoded = match entity.as_str() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "nbsp" => Some(' '),
                        "#39" | "apos" => Some('\''),
                        _ => None,
                    };
                    if let Some(decoded_char) = decoded {
                        result.push(decoded_char);
                        // Advance past entity + semicolon
                        for _ in 0..entity.len() {
                            chars.next();
                        }
                        if chars.peek() == Some(&';') {
                            chars.next();
                        }
                        continue;
                    }
                }
                result.push('&');
                continue;
            }
            result.push(ch);
        }

        // Collapse excessive whitespace
        let mut collapsed = String::with_capacity(result.len());
        let mut blank_lines = 0;
        for line in result.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                blank_lines += 1;
                if blank_lines <= 1 {
                    collapsed.push('\n');
                }
            } else {
                blank_lines = 0;
                collapsed.push_str(trimmed);
                collapsed.push('\n');
            }
        }

        collapsed.trim().to_string()
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
