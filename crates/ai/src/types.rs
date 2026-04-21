use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission tier for AI operations.
///
/// Container-first: standard ops are pre-allowed within the container.
/// Only "escape hatch" operations (host filesystem, external network)
/// require explicit user approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PermissionTier {
    /// Read buffer contents, cursor state, file metadata.
    ReadOnly,
    /// Modify buffers, move cursors, standard editing.
    Write,
    /// Execute shell commands within the container.
    Shell,
    /// Host filesystem, external network, editor config changes.
    Privileged,
}

/// A tool definition sent to the LLM provider.
/// Format is provider-agnostic — serialized into Claude/OpenAI format by each provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: ToolParameters,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameters {
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(default)]
    pub properties: HashMap<String, ToolProperty>,
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProperty {
    #[serde(rename = "type")]
    pub prop_type: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

/// A tool call requested by the AI model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result of executing a tool call, sent back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub success: bool,
    pub output: String,
}

/// A message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// Content of a message — text, tool calls, or a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    ToolCalls(Vec<ToolCall>),
    /// Assistant returned both reasoning text and tool calls in the same turn.
    /// Reasoning models (DeepSeek, etc.) commonly do this — the text must be
    /// preserved in message history so the model can track its own progress.
    TextWithToolCalls {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult(ToolResult),
}

/// Events sent from the AI task to the main thread via mpsc channel.
#[derive(Debug)]
pub enum AiEvent {
    /// AI wants to execute a tool — main thread runs it and replies via oneshot.
    ToolCallRequest {
        call: ToolCall,
        reply: tokio::sync::oneshot::Sender<ToolResult>,
    },
    /// AI produced a text response.
    TextResponse(String),
    /// Partial streaming token for real-time display in conversation buffer.
    StreamChunk(String),
    /// AI session completed (final response).
    SessionComplete(String),
    /// AI wants to ask the user a clarifying question.
    AskUser {
        question: String,
        reply: tokio::sync::oneshot::Sender<String>,
    },
    /// AI proposed file changes for approval.
    ProposeChanges {
        /// Array of {file_path, new_content}
        changes: serde_json::Value,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    /// AI wants to spawn a sub-agent.
    Delegate {
        profile: String,
        objective: String,
        reply: tokio::sync::oneshot::Sender<ToolResult>,
    },
    /// An error occurred in the AI transport.
    Error(String),
    /// Incremental cost tally after a successful provider round. The
    /// main thread uses this to keep the status-line spend counter
    /// fresh without having to inspect the provider response itself.
    /// A `None` price means the model isn't in the pricing table
    /// (e.g. local Ollama) — callers should still surface token counts
    /// but treat the session cost as effectively zero.
    CostUpdate {
        session_usd: f64,
        last_call_usd: f64,
        tokens_in: u64,
        tokens_out: u64,
    },
    /// Fired once the first time cumulative session cost crosses the
    /// configured warning threshold. The editor posts this to
    /// *Messages* so users see a heads-up while they still have budget
    /// to act on.
    BudgetWarning {
        session_usd: f64,
        threshold_usd: f64,
    },
    /// Fired when the session would exceed its configured hard cap.
    /// The session aborts the in-flight prompt after emitting this.
    /// Separate from `Error` so consumers can treat cost-refusal
    /// distinctly from transport/API errors (e.g. for telemetry or
    /// retry policies).
    BudgetExceeded { session_usd: f64, cap_usd: f64 },
    /// Update on the current round and transaction status.
    RoundUpdate {
        round: usize,
        transaction_start_idx: Option<usize>,
    },
    /// AI session wants to change the editor's AI operating mode.
    UpdateMode(String),
    /// AI session wants to change the active prompt profile.
    UpdateProfile(String),
}

/// Commands sent from the main thread to the AI task.
#[derive(Debug)]
pub enum AiCommand {
    /// Start a new conversation turn.
    Prompt(String),
    /// Cancel the current operation.
    Cancel,
    /// Shut down the AI task.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_tier_ordering() {
        assert!(PermissionTier::ReadOnly < PermissionTier::Write);
        assert!(PermissionTier::Write < PermissionTier::Shell);
        assert!(PermissionTier::Shell < PermissionTier::Privileged);
    }

    #[test]
    fn tool_definition_serde_round_trip() {
        let tool = ToolDefinition {
            name: "buffer_read".into(),
            description: "Read buffer contents".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "start_line".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "First line (1-indexed)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["start_line".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "buffer_read");
        assert_eq!(parsed.parameters.required, vec!["start_line"]);
    }

    #[test]
    fn tool_call_serde_round_trip() {
        let call = ToolCall {
            id: "call_123".into(),
            name: "buffer_read".into(),
            arguments: serde_json::json!({"start_line": 1}),
        };
        let json = serde_json::to_string(&call).unwrap();
        let parsed: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "call_123");
        assert_eq!(parsed.arguments["start_line"], 1);
    }

    #[test]
    fn tool_result_serde_round_trip() {
        let result = ToolResult {
            tool_call_id: "call_123".into(),
            success: true,
            output: "Hello world".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let parsed: ToolResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_call_id, "call_123");
        assert!(parsed.success);
    }

    #[test]
    fn message_text_serde_round_trip() {
        let msg = Message {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, Role::User);
        match parsed.content {
            MessageContent::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn message_tool_calls_serde_round_trip() {
        let msg = Message {
            role: Role::Assistant,
            content: MessageContent::ToolCalls(vec![ToolCall {
                id: "call_1".into(),
                name: "buffer_read".into(),
                arguments: serde_json::json!({}),
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, Role::Assistant);
        match parsed.content {
            MessageContent::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "buffer_read");
            }
            _ => panic!("expected ToolCalls"),
        }
    }
}
