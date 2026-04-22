//! Lightweight token estimation using chars/4 heuristic.
//!
//! Good enough for budget-aware pruning — tiktoken would add a 10MB dep
//! for marginal accuracy improvement. The chars/4 ratio is widely used
//! (OpenAI docs suggest it, Claude tokenizer averages ~3.5-4.5 chars/token).
//!
//! All estimates are conservative (overestimate) so pruning triggers early
//! rather than late — the safe direction for context management.

use crate::types::*;

/// Estimate token count for a text string.
/// Uses chars/4 heuristic, rounded up. Minimum 1 for non-empty strings.
pub fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let chars = text.len() as u64; // byte len is close enough for ASCII-heavy content
    chars.div_ceil(4)
}

/// Overhead tokens per message (role marker, formatting).
const MESSAGE_OVERHEAD: u64 = 4;

/// Estimate tokens for a single message including role overhead.
pub fn estimate_message_tokens(msg: &Message) -> u64 {
    let content_tokens = match &msg.content {
        MessageContent::Text(text) => estimate_tokens(text),
        MessageContent::ToolCalls(calls) => {
            let mut total = 0u64;
            for call in calls {
                total += estimate_tokens(&call.name);
                total += estimate_tokens(&call.arguments.to_string());
                total += 10; // id + structural overhead
            }
            total
        }
        MessageContent::TextWithToolCalls { text, tool_calls } => {
            let mut total = estimate_tokens(text);
            for call in tool_calls {
                total += estimate_tokens(&call.name);
                total += estimate_tokens(&call.arguments.to_string());
                total += 10;
            }
            total
        }
        MessageContent::ToolResult(result) => {
            estimate_tokens(&result.output) + estimate_tokens(&result.tool_call_id) + 5
        }
    };
    content_tokens + MESSAGE_OVERHEAD
}

/// Estimate total tokens for a slice of messages.
pub fn estimate_messages_tokens(msgs: &[Message]) -> u64 {
    msgs.iter().map(estimate_message_tokens).sum()
}

/// Estimate tokens for tool definitions (serialized to JSON).
pub fn estimate_tools_tokens(tools: &[ToolDefinition]) -> u64 {
    if tools.is_empty() {
        return 0;
    }
    // Serialize to JSON string and estimate that.
    // This is done once at session construction, so the allocation is fine.
    match serde_json::to_string(tools) {
        Ok(json) => estimate_tokens(&json),
        Err(_) => {
            // Fallback: rough estimate per tool
            tools.len() as u64 * 150
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn empty_string_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn short_text() {
        // "hello" = 5 bytes -> ceil(5/4) = 2
        assert_eq!(estimate_tokens("hello"), 2);
    }

    #[test]
    fn longer_text() {
        // 100 chars -> 25 tokens
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens(&text), 25);
    }

    #[test]
    fn one_char_is_one_token() {
        assert_eq!(estimate_tokens("x"), 1);
    }

    #[test]
    fn message_includes_overhead() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hi".into()),
        };
        // "hi" = 2 bytes -> ceil(2/4) = 1 + 4 overhead = 5
        assert_eq!(estimate_message_tokens(&msg), 5);
    }

    #[test]
    fn tool_calls_estimated() {
        let msg = Message {
            role: Role::Assistant,
            content: MessageContent::ToolCalls(vec![ToolCall {
                id: "call_1".into(),
                name: "buffer_read".into(),
                arguments: serde_json::json!({"start_line": 1}),
            }]),
        };
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 10, "tool call should have significant tokens");
    }

    #[test]
    fn multiple_messages_summed() {
        let msgs = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("world".into()),
            },
        ];
        let total = estimate_messages_tokens(&msgs);
        let individual: u64 = msgs.iter().map(estimate_message_tokens).sum();
        assert_eq!(total, individual);
    }

    #[test]
    fn tools_tokens_nonzero() {
        let tools = vec![ToolDefinition {
            name: "buffer_read".into(),
            description: "Read buffer contents".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "start_line".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "First line".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["start_line".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        }];
        let tokens = estimate_tools_tokens(&tools);
        assert!(tokens > 0);
    }

    #[test]
    fn empty_tools_is_zero() {
        assert_eq!(estimate_tools_tokens(&[]), 0);
    }
}
