use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::provider::ErrorKind;
use crate::provider::*;
use crate::types::*;

/// Claude Messages API provider.
///
/// Serializes tools and messages into Claude's format and parses responses.
/// Supports both anthropic.com and custom base URLs (for proxies/testing).
pub struct ClaudeProvider {
    client: Client,
    config: ProviderConfig,
}

impl ClaudeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        ClaudeProvider { client, config }
    }

    /// Convert canonical ToolDefinition to Claude's tool format.
    /// The last tool gets a `cache_control` breakpoint for prompt caching.
    pub fn serialize_tools(tools: &[ToolDefinition]) -> serde_json::Value {
        let len = tools.len();
        tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut tool = json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": {
                        "type": t.parameters.schema_type,
                        "properties": t.parameters.properties.iter().map(|(k, v)| {
                            (k.clone(), json!({
                                "type": v.prop_type,
                                "description": v.description,
                            }))
                        }).collect::<serde_json::Map<String, serde_json::Value>>(),
                        "required": t.parameters.required,
                    }
                });
                // Cache breakpoint on the last tool definition
                if i == len - 1 {
                    tool["cache_control"] = json!({"type": "ephemeral"});
                }
                tool
            })
            .collect()
    }

    /// Format system prompt as a cacheable content block array.
    /// Claude's prompt caching requires the system field to be an array
    /// of content blocks with `cache_control` on the last block.
    fn serialize_system_prompt(system_prompt: &str) -> serde_json::Value {
        json!([{
            "type": "text",
            "text": system_prompt,
            "cache_control": {"type": "ephemeral"}
        }])
    }

    /// Convert canonical Messages to Claude's message format.
    fn serialize_tool_use_blocks(calls: &[ToolCall]) -> Vec<serde_json::Value> {
        calls
            .iter()
            .map(|call| {
                json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                })
            })
            .collect()
    }

    pub fn serialize_messages(messages: &[Message]) -> serde_json::Value {
        let mut result = Vec::new();

        for msg in messages {
            match (&msg.role, &msg.content) {
                (Role::User, MessageContent::Text(text)) => {
                    result.push(json!({
                        "role": "user",
                        "content": text,
                    }));
                }
                (Role::Assistant, MessageContent::Text(text)) => {
                    result.push(json!({
                        "role": "assistant",
                        "content": text,
                    }));
                }
                (Role::Assistant, MessageContent::ToolCalls(calls)) => {
                    let content = Self::serialize_tool_use_blocks(calls);
                    result.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
                (
                    Role::Assistant,
                    MessageContent::TextWithToolCalls {
                        text,
                        tool_calls: calls,
                    },
                ) => {
                    let mut content = vec![json!({ "type": "text", "text": text })];
                    content.extend(Self::serialize_tool_use_blocks(calls));
                    result.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
                (Role::Tool, MessageContent::ToolResult(tr)) => {
                    result.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tr.tool_call_id,
                            "content": tr.output,
                        }],
                    }));
                }
                _ => {} // Skip unsupported combinations
            }
        }

        serde_json::Value::Array(result)
    }

    /// Parse Claude's response into canonical ProviderResponse.
    pub fn parse_response(body: &serde_json::Value) -> Result<ProviderResponse, ProviderError> {
        let content = body
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ProviderError {
                message: "Missing 'content' array in response".into(),
                retryable: false,
                kind: ErrorKind::Unknown,
            })?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = match block.get("input").cloned() {
                        Some(v) => v,
                        None => {
                            warn!(tool = %name, "Claude tool_use block missing 'input' field, defaulting to {{}}");
                            json!({})
                        }
                    };
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }

        let stop_reason = match body.get("stop_reason").and_then(|s| s.as_str()) {
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };

        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join("\n"))
        };

        // Anthropic returns usage at the top level of the response body.
        let usage = body.get("usage").map(|u| {
            let cache_read = u
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_creation = u
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let prompt_tokens = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                + cache_read
                + cache_creation;

            Usage {
                prompt_tokens,
                completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_creation,
            }
        });

        Ok(ProviderResponse {
            text,
            tool_calls,
            stop_reason,
            usage,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for ClaudeProvider {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let url = self
            .config
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1/messages");

        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": Self::serialize_system_prompt(system_prompt),
            "messages": Self::serialize_messages(messages),
        });

        // Only include tools if non-empty
        if !tools.is_empty() {
            body["tools"] = Self::serialize_tools(tools);
        }

        if let Some(temp) = self.config.temperature {
            body["temperature"] = json!(temp);
        }

        debug!(model = %self.config.model, url, message_count = messages.len(), tool_count = tools.len(), "sending Claude API request");

        let response = self
            .client
            .post(url)
            .header("x-api-key", self.config.api_key.as_deref().unwrap_or(""))
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, is_timeout = e.is_timeout(), "Claude HTTP error");
                ProviderError {
                    message: format!("HTTP error: {}", e),
                    retryable: e.is_timeout(),
                    kind: ErrorKind::Transport,
                }
            })?;

        let status = response.status();
        debug!(status = %status, "Claude API response received");

        let resp_body: serde_json::Value = response.json().await.map_err(|e| {
            warn!(error = %e, "Claude response JSON parse error");
            ProviderError {
                message: format!("JSON parse error: {}", e),
                retryable: false,
                kind: ErrorKind::Transport,
            }
        })?;

        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
            let body_lower = resp_body.to_string().to_ascii_lowercase();
            let kind = if body_lower.contains("context_length")
                || body_lower.contains("too many tokens")
            {
                ErrorKind::ContextOverflow
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                ErrorKind::Auth
            } else if status.as_u16() == 429 {
                ErrorKind::RateLimit
            } else {
                ErrorKind::Unknown
            };
            warn!(status = %status, retryable, ?kind, "Claude API error response");
            return Err(ProviderError {
                message: format!("API error {}: {}", status, resp_body),
                retryable,
                kind,
            });
        }

        Self::parse_response(&resp_body)
    }

    fn name(&self) -> &str {
        "claude"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_tools() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "buffer_read".into(),
            description: "Read buffer".into(),
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
        }]
    }

    #[test]
    fn serialize_tools_shape() {
        let tools = sample_tools();
        let json = ClaudeProvider::serialize_tools(&tools);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let tool = &arr[0];
        assert_eq!(tool["name"], "buffer_read");
        assert_eq!(tool["description"], "Read buffer");
        assert_eq!(tool["input_schema"]["type"], "object");
        assert!(tool["input_schema"]["properties"]["start_line"].is_object());
        assert_eq!(tool["input_schema"]["required"][0], "start_line");
        // Last tool gets cache_control for prompt caching
        assert_eq!(tool["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn serialize_tools_cache_control_on_last_only() {
        let tools = vec![
            sample_tools().remove(0),
            ToolDefinition {
                name: "cursor_info".into(),
                description: "Get cursor".into(),
                parameters: ToolParameters {
                    schema_type: "object".into(),
                    properties: HashMap::new(),
                    required: vec![],
                },
                permission: Some(PermissionTier::ReadOnly),
            },
        ];
        let json = ClaudeProvider::serialize_tools(&tools);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // First tool: no cache_control
        assert!(arr[0].get("cache_control").is_none());
        // Last tool: has cache_control
        assert_eq!(arr[1]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn serialize_system_prompt_cacheable() {
        let sys = ClaudeProvider::serialize_system_prompt("You are an editor.");
        let arr = sys.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "You are an editor.");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn serialize_messages_text() {
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hello".into()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("hi".into()),
            },
        ];
        let json = ClaudeProvider::serialize_messages(&messages);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[0]["content"], "hello");
        assert_eq!(arr[1]["role"], "assistant");
        assert_eq!(arr[1]["content"], "hi");
    }

    #[test]
    fn serialize_messages_tool_result() {
        let messages = vec![Message {
            role: Role::Tool,
            content: MessageContent::ToolResult(ToolResult {
                tool_call_id: "call_123".into(),
                tool_name: "test_tool".into(),
                success: true,
                output: "result text".into(),
            }),
        }];
        let json = ClaudeProvider::serialize_messages(&messages);
        let arr = json.as_array().unwrap();
        assert_eq!(arr[0]["role"], "user");
        let content = arr[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "call_123");
        assert_eq!(content[0]["content"], "result text");
    }

    #[test]
    fn parse_response_text_only() {
        let body = json!({
            "content": [{"type": "text", "text": "Hello world"}],
            "stop_reason": "end_turn",
        });
        let resp = ClaudeProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Hello world"));
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_response_tool_use() {
        let body = json!({
            "content": [{
                "type": "tool_use",
                "id": "call_abc",
                "name": "buffer_read",
                "input": {"start_line": 1},
            }],
            "stop_reason": "tool_use",
        });
        let resp = ClaudeProvider::parse_response(&body).unwrap();
        assert!(resp.text.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_abc");
        assert_eq!(resp.tool_calls[0].name, "buffer_read");
        assert_eq!(resp.tool_calls[0].arguments["start_line"], 1);
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_mixed() {
        let body = json!({
            "content": [
                {"type": "text", "text": "Let me check"},
                {"type": "tool_use", "id": "call_1", "name": "cursor_info", "input": {}},
            ],
            "stop_reason": "tool_use",
        });
        let resp = ClaudeProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Let me check"));
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_max_tokens() {
        let body = json!({
            "content": [{"type": "text", "text": "truncated..."}],
            "stop_reason": "max_tokens",
        });
        let resp = ClaudeProvider::parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn parse_response_with_cache_tokens() {
        let body = json!({
            "content": [{"type": "text", "text": "cached response"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 8000,
                "cache_creation_input_tokens": 0,
            },
        });
        let resp = ClaudeProvider::parse_response(&body).unwrap();
        let usage = resp.usage.unwrap();
        // prompt_tokens = input_tokens + cache_read + cache_creation = 100 + 8000 + 0
        assert_eq!(usage.prompt_tokens, 8100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 8000);
        assert_eq!(usage.cache_creation_tokens, 0);
    }
}
