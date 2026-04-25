use tracing::{debug, warn};

use crate::token_estimate;
use crate::types::*;

use super::AgentSession;

impl AgentSession {
    /// Available token budget for message history.
    pub(super) fn available_message_budget(&self) -> u64 {
        self.context_window
            .saturating_sub(self.system_prompt_tokens)
            .saturating_sub(self.tools_tokens)
            .saturating_sub(self.reserved_output)
    }

    /// Trim message history to stay within token budget AND hard message cap.
    /// Keeps the first message (initial user context) and the most recent messages.
    pub(super) fn trim_messages(&mut self) {
        let original_len = self.messages.len();
        let mut safe_boundary = self.transaction_start_idx.unwrap_or(self.messages.len());

        // 1. Hard cap on message count (secondary safeguard)
        while self.messages.len() > self.max_messages && self.messages.len() > 1 {
            if safe_boundary <= 1 {
                break; // Protect current transaction
            }
            self.messages.remove(1);
            safe_boundary -= 1;
        }

        // 2. Token-aware pruning: drop oldest non-first messages until within budget
        let budget = self.available_message_budget();
        while self.messages.len() > 1 {
            let total = token_estimate::estimate_messages_tokens(&self.messages);
            if total <= budget {
                break;
            }
            if safe_boundary <= 1 {
                break; // Protect current transaction
            }
            // Remove the oldest message after the first
            self.messages.remove(1);
            safe_boundary -= 1;
        }

        // 3. Enforce API schema: No orphaned Tool messages at the prune boundary.
        // Only needed after actual pruning — if steps 1-2 didn't remove anything,
        // there are no orphans and running this would destroy valid tool history.
        if self.messages.len() < original_len {
            while self.messages.len() > 1 {
                let msg = &self.messages[1];
                if msg.role == Role::Tool {
                    self.messages.remove(1);
                    safe_boundary = safe_boundary.saturating_sub(1).max(1);
                } else if let MessageContent::TextWithToolCalls { .. }
                | MessageContent::ToolCalls(_) = msg.content
                {
                    // Drop any Assistant message with tool calls at the boundary,
                    // as its matching ToolResults might have been pruned or partially pruned.
                    self.messages.remove(1);
                    safe_boundary = safe_boundary.saturating_sub(1).max(1);
                } else {
                    break;
                }
            }
        }

        self.transaction_start_idx = self.transaction_start_idx.map(|_| safe_boundary);

        if self.messages.len() < original_len {
            let removed = original_len - self.messages.len();
            let total = token_estimate::estimate_messages_tokens(&self.messages);
            warn!(
                removed,
                remaining = self.messages.len(),
                estimated_tokens = total,
                budget,
                "trim_messages removed messages"
            );
            self.log_transcript_event(
                "trim_messages",
                &format!(
                    "removed={}, remaining={}, tokens={}, budget={}",
                    removed,
                    self.messages.len(),
                    total,
                    budget
                ),
            );
        }
    }

    /// Prune oldest messages for context overflow recovery.
    /// Drops ~10% of non-first messages to preserve as much context as possible.
    pub(super) fn aggressive_prune(&mut self) {
        if self.messages.len() <= 2 {
            return;
        }
        let to_remove = (self.messages.len() - 1) / 10; // 10% of non-first messages
        let to_remove = to_remove.max(2);
        self.messages.drain(1..1 + to_remove);

        // Enforce OpenAI tool call schema:
        // A Tool message MUST be preceded by an Assistant message with tool_calls.
        // If our arbitrary prune cut off the Assistant message, or left an Assistant message
        // with tool_calls but dropped its corresponding Tool messages, we must drop them too.
        while self.messages.len() > 1 {
            let msg = &self.messages[1];
            if msg.role == Role::Tool {
                self.messages.remove(1);
            } else if let MessageContent::TextWithToolCalls { .. } | MessageContent::ToolCalls(_) =
                msg.content
            {
                // Assistant message with tool calls at the boundary is unsafe to keep,
                // as its matching ToolResults might have been pruned.
                self.messages.remove(1);
            } else {
                break;
            }
        }

        // Adjust transaction_start_idx if it was affected
        if let Some(idx) = self.transaction_start_idx {
            self.transaction_start_idx = Some(idx.saturating_sub(to_remove).max(1));
        }

        warn!(
            removed = to_remove,
            remaining = self.messages.len(),
            "aggressively pruned messages for context overflow recovery"
        );
    }

    /// Compact conversation history by truncating old tool results.
    ///
    /// Strategy: keep all messages (preserving conversation structure and progress
    /// context), but replace large tool result outputs with a short summary.
    /// This is idempotent — already-compacted results stay compacted.
    ///
    /// Protected zones (untouched):
    /// - `messages[0]` (initial context)
    /// - Current transaction (`transaction_start_idx..`)
    /// - Last 4 messages (recent context)
    pub(super) fn compact_history(&mut self) {
        let tx_start = self.transaction_start_idx.unwrap_or(self.messages.len());
        let protect_tail = 4;
        let compact_end = self
            .messages
            .len()
            .saturating_sub(protect_tail)
            .min(tx_start);
        let mut compacted = 0;

        if compact_end <= 1 {
            return;
        }
        for msg in &mut self.messages[1..compact_end] {
            if let MessageContent::ToolResult(ref mut r) = msg.content {
                let token_est = token_estimate::estimate_tokens(&r.output);
                if token_est > 100 {
                    let first_lines: String =
                        r.output.lines().take(2).collect::<Vec<_>>().join("\n");
                    let status = if r.success { "✓" } else { "✗" };
                    r.output = format!(
                        "[{} {} — {} tokens compacted]\n{}",
                        status, r.tool_name, token_est, first_lines
                    );
                    compacted += 1;
                }
            }
        }
        if compacted > 0 {
            debug!(compacted, "compact_history: truncated old tool results");
        }
    }

    /// Truncate a tool result if it exceeds 25% of available message budget.
    pub(super) fn truncate_tool_result(&self, result: &mut ToolResult) {
        let budget = self.available_message_budget();
        let max_result_tokens = budget / 4;
        let result_tokens = token_estimate::estimate_tokens(&result.output);
        if result_tokens > max_result_tokens && max_result_tokens > 100 {
            // Truncate to roughly max_result_tokens * 4 bytes
            let max_bytes = (max_result_tokens * 4) as usize;
            let truncated = if result.output.len() > max_bytes {
                let safe_end = result.output.floor_char_boundary(max_bytes);
                &result.output[..safe_end]
            } else {
                &result.output
            };
            let original_tokens = result_tokens;
            result.output = format!(
                "{}\n...\n[TRUNCATED — {} tokens, showing first {}]. Result exceeded 25% of context budget. To get the data you need:\n- For file reads: use start_line/end_line to request a specific range\n- For searches: use a more specific query or glob pattern\n- For shell output: pipe through head/tail/grep to filter\n- Do NOT retry the same call — narrow your request instead.",
                truncated, original_tokens, max_result_tokens
            );
            debug!(
                original_tokens,
                truncated_to = max_result_tokens,
                "truncated oversized tool result"
            );
        }
    }

    /// Check context pressure and degrade capabilities if needed.
    /// Returns true if degradation level changed.
    /// One-way: Normal → ToolsShed → Minimal (never recovers within a session).
    pub(super) fn check_and_degrade(&mut self) -> bool {
        use super::DegradationLevel;

        let total_used = self.system_prompt_tokens
            + self.tools_tokens
            + token_estimate::estimate_messages_tokens(&self.messages)
            + self.reserved_output;
        let usage_pct = total_used as f64 / self.context_window as f64;

        match self.degradation_level {
            DegradationLevel::Normal if usage_pct > 0.85 => {
                self.shed_extended_tools();
                self.degradation_level = DegradationLevel::ToolsShed;
                true
            }
            DegradationLevel::ToolsShed if usage_pct > 0.92 => {
                self.simplify_system_prompt();
                self.degradation_level = DegradationLevel::Minimal;
                true
            }
            _ => false,
        }
    }

    /// Remove all extended tools, keeping only core tools.
    fn shed_extended_tools(&mut self) {
        self.tools
            .retain(|t| crate::tools::classify_tool_tier(&t.name) == crate::tools::ToolTier::Core);
        // Re-add request_tools so the model knows it lost tools
        if !self.tools.iter().any(|t| t.name == "request_tools") {
            self.tools.push(crate::tools::request_tools_definition());
        }
        self.tools_tokens = token_estimate::estimate_tools_tokens(&self.tools);
        self.enabled_categories.clear();
        warn!(
            tool_count = self.tools.len(),
            tools_tokens = self.tools_tokens,
            "shed extended tools due to context pressure"
        );
    }

    /// Truncate the system prompt to reduce token usage.
    fn simplify_system_prompt(&mut self) {
        const MAX_CHARS: usize = 2000;
        if self.system_prompt.len() > MAX_CHARS {
            let truncated =
                &self.system_prompt[..self.system_prompt.floor_char_boundary(MAX_CHARS)];
            self.system_prompt = format!(
                "{}\n\n[System prompt truncated due to context pressure. \
                 Focus on completing the current task with available tools.]",
                truncated
            );
            self.system_prompt_tokens = token_estimate::estimate_tokens(&self.system_prompt);
            warn!(
                system_prompt_tokens = self.system_prompt_tokens,
                "simplified system prompt due to context pressure"
            );
        }
    }

    /// Collapse and clear the current transaction, if any.
    pub(super) fn finalize_transaction(&mut self) {
        if let Some(start_idx) = self.transaction_start_idx {
            self.collapse_transaction(start_idx);
        }
        self.transaction_start_idx = None;
    }
}
