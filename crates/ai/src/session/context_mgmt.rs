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
            let total = token_estimate::estimate_messages_tokens(&self.messages);
            debug!(
                messages = self.messages.len(),
                estimated_tokens = total,
                budget,
                "post-trim message state"
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

    /// Summarize older conversation turns into compact recaps before resorting
    /// to `trim_messages()` which drops messages entirely.
    ///
    /// Strategy:
    /// 1. Protect: `messages[0]` (initial context), current transaction, last 4 messages
    /// 2. Group compactable messages into chunks of ~6
    /// 3. Replace each chunk with one summary message
    /// 4. Adjust `transaction_start_idx` after compaction
    /// 5. Enforce orphan-free boundary (no dangling Tool/ToolCalls messages)
    pub(super) fn compact_history(&mut self) {
        let total = self.messages.len();
        if total < 8 {
            return; // Not enough messages to compact
        }

        // Determine protected ranges
        let tx_start = self.transaction_start_idx.unwrap_or(total);
        let protect_tail = 4;
        let compactable_end = total.saturating_sub(protect_tail).min(tx_start);

        // Must have at least 6 compactable messages (indices 1..compactable_end)
        if compactable_end <= 7 {
            return;
        }

        let chunk_size = 6;
        let mut new_messages: Vec<Message> = Vec::new();
        // Keep first message (initial context)
        new_messages.push(self.messages[0].clone());

        let mut i = 1;
        while i < compactable_end {
            let end = (i + chunk_size).min(compactable_end);
            if end - i < 3 {
                // Too small to summarize, keep as-is
                for msg in &self.messages[i..end] {
                    new_messages.push(msg.clone());
                }
                i = end;
                continue;
            }
            let chunk = &self.messages[i..end];
            let summary = Self::summarize_chunk(chunk);
            new_messages.push(Message {
                role: Role::User,
                content: MessageContent::Text(summary),
            });
            i = end;
        }

        // Ensure no orphaned Tool messages at the boundary
        // (The protected tail starts at compactable_end)
        let mut tail_start = compactable_end;
        while tail_start < total {
            let msg = &self.messages[tail_start];
            if msg.role == Role::Tool {
                tail_start += 1; // Skip orphaned Tool message
            } else if matches!(
                msg.content,
                MessageContent::ToolCalls(_) | MessageContent::TextWithToolCalls { .. }
            ) {
                tail_start += 1; // Skip orphaned ToolCalls
            } else {
                break;
            }
        }

        // Append protected tail
        for msg in &self.messages[tail_start..] {
            new_messages.push(msg.clone());
        }

        let removed = total - new_messages.len();
        if removed == 0 {
            return;
        }

        // Adjust transaction_start_idx
        if let Some(old_idx) = self.transaction_start_idx {
            let new_idx = new_messages
                .len()
                .saturating_sub(total.saturating_sub(old_idx));
            self.transaction_start_idx = Some(new_idx);
        }

        debug!(
            original = total,
            compacted = new_messages.len(),
            removed,
            "compact_history summarized old turns"
        );
        self.messages = new_messages;
    }

    fn summarize_chunk(messages: &[Message]) -> String {
        let mut tool_names: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut key_points: Vec<String> = Vec::new();

        for msg in messages {
            match &msg.content {
                MessageContent::Text(t) => {
                    if msg.role == Role::Assistant && key_points.len() < 3 {
                        // Take first sentence
                        let first = t.split(['.', '\n']).next().unwrap_or(t);
                        if !first.trim().is_empty() && first.len() < 200 {
                            key_points.push(first.trim().to_string());
                        }
                    }
                }
                MessageContent::ToolCalls(calls) => {
                    for c in calls {
                        *tool_names.entry(c.name.clone()).or_insert(0) += 1;
                    }
                }
                MessageContent::TextWithToolCalls { text, tool_calls } => {
                    if msg.role == Role::Assistant && key_points.len() < 3 {
                        let first = text.split(['.', '\n']).next().unwrap_or(text);
                        if !first.trim().is_empty() && first.len() < 200 {
                            key_points.push(first.trim().to_string());
                        }
                    }
                    for c in tool_calls {
                        *tool_names.entry(c.name.clone()).or_insert(0) += 1;
                    }
                }
                MessageContent::ToolResult(_) => {}
            }
        }

        let mut summary = format!("[Context summary — {} turns compacted]\n", messages.len());

        if !tool_names.is_empty() {
            let mut tools: Vec<String> = tool_names
                .iter()
                .map(|(name, count)| {
                    if *count > 1 {
                        format!("{}×{}", name, count)
                    } else {
                        name.clone()
                    }
                })
                .collect();
            tools.sort();
            summary.push_str(&format!("Tools: {}\n", tools.join(", ")));
        }

        if !key_points.is_empty() {
            summary.push_str("Key points:\n");
            for point in &key_points {
                summary.push_str(&format!("- {}\n", point));
            }
        }

        summary
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
