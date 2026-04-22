use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::provider::ErrorKind;
use crate::provider::*;
use crate::types::*;

/// OpenAI Chat Completions API provider.
///
/// Also works with OpenAI-compatible endpoints (Ollama, vLLM, etc.)
/// via the `base_url` config.
pub struct OpenAiProvider {
    client: Client,
    config: ProviderConfig,
}

impl OpenAiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        OpenAiProvider {
            client: Client::builder()
                .timeout(Duration::from_secs(config.timeout_secs))
                .build()
                .expect("failed to build HTTP client"),
            config,
        }
    }

    /// Convert canonical ToolDefinition to OpenAI's function-calling format.
    pub fn serialize_tools(tools: &[ToolDefinition]) -> serde_json::Value {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": {
                            "type": t.parameters.schema_type,
                            "properties": t.parameters.properties.iter().map(|(k, v)| {
                                (k.clone(), json!({
                                    "type": v.prop_type,
                                    "description": v.description,
                                }))
                            }).collect::<serde_json::Map<String, serde_json::Value>>(),
                            "required": t.parameters.required,
                        },
                    }
                })
            })
            .collect()
    }

    fn serialize_tool_calls(calls: &[ToolCall]) -> Vec<serde_json::Value> {
        calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    },
                })
            })
            .collect()
    }

    /// Convert canonical Messages to OpenAI's message format.
    pub fn serialize_messages(messages: &[Message], system_prompt: &str) -> serde_json::Value {
        let mut result = vec![json!({
            "role": "system",
            "content": system_prompt,
        })];

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
                    let tool_calls = Self::serialize_tool_calls(calls);
                    result.push(json!({
                        "role": "assistant",
                        "content": serde_json::Value::Null,
                        "tool_calls": tool_calls,
                    }));
                }
                (
                    Role::Assistant,
                    MessageContent::TextWithToolCalls {
                        text,
                        tool_calls: calls,
                    },
                ) => {
                    let tool_calls = Self::serialize_tool_calls(calls);
                    result.push(json!({
                        "role": "assistant",
                        "content": text,
                        "tool_calls": tool_calls,
                    }));
                }
                (Role::Tool, MessageContent::ToolResult(tr)) => {
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_call_id,
                        "content": tr.output,
                    }));
                }
                _ => {}
            }
        }

        serde_json::Value::Array(result)
    }

    /// Parse OpenAI's response into canonical ProviderResponse.
    pub fn parse_response(body: &serde_json::Value) -> Result<ProviderResponse, ProviderError> {
        let choice = body
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .ok_or_else(|| ProviderError {
                message: "Missing 'choices' array in response".into(),
                retryable: false,
                kind: ErrorKind::Unknown,
            })?;

        let message = choice.get("message").ok_or_else(|| ProviderError {
            message: "Missing 'message' in choice".into(),
            retryable: false,
            kind: ErrorKind::Unknown,
        })?;

        let text = message
            .get("content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let tool_calls: Vec<ToolCall> = message
            .get("tool_calls")
            .and_then(|tc| tc.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|call| {
                        let id = call.get("id")?.as_str()?.to_string();
                        let func = call.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let args_str = func.get("arguments")?.as_str()?;
                        let arguments = serde_json::from_str(args_str).unwrap_or(json!({}));
                        Some(ToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("stop");

        let stop_reason = match finish_reason {
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            _ => {
                if tool_calls.is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                }
            }
        };

        // OpenAI-compatible endpoints put token counts at the top level
        // under "usage". DeepSeek (and potentially OpenAI) include cache details.
        let usage = body.get("usage").map(|u| {
            let prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let completion_tokens = u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // DeepSeek specific: usage.prompt_tokens_details.cached_tokens
            let cache_read = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            Usage {
                prompt_tokens,
                completion_tokens,
                cache_read_tokens: cache_read,
                cache_creation_tokens: 0, // DeepSeek doesn't report creation separately yet
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
impl AgentProvider for OpenAiProvider {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let url = format!(
            "{}/chat/completions",
            self.config
                .base_url
                .as_deref()
                .unwrap_or("https://api.openai.com/v1")
        );

        let mut body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "messages": Self::serialize_messages(messages, system_prompt),
        });

        if !tools.is_empty() {
            body["tools"] = Self::serialize_tools(tools);
        }

        if let Some(temp) = self.config.temperature {
            body["temperature"] = json!(temp);
        }

        debug!(model = %self.config.model, url = %url, message_count = messages.len(), tool_count = tools.len(), "sending OpenAI API request");

        let response = self
            .client
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.api_key.as_deref().unwrap_or("")),
            )
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, is_timeout = e.is_timeout(), "OpenAI HTTP error");
                ProviderError {
                    message: format!("HTTP error: {}", e),
                    retryable: e.is_timeout(),
                    kind: ErrorKind::Transport,
                }
            })?;

        let status = response.status();
        debug!(status = %status, "OpenAI API response received");

        // Read raw body first so we can give useful error messages for
        // non-JSON responses (e.g. HTML error pages from auth failures).
        let raw_body = response.bytes().await.map_err(|e| {
            warn!(error = %e, "failed to read response body");
            ProviderError {
                message: format!("Failed to read response body: {}", e),
                retryable: false,
                kind: ErrorKind::Transport,
            }
        })?;

        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
            let body_preview = String::from_utf8_lossy(&raw_body);
            let body_preview = if body_preview.len() > 500 {
                format!("{}...", &body_preview[..500])
            } else {
                body_preview.to_string()
            };
            let hint = if status.as_u16() == 401 {
                " (check API key / api_key_command in config)"
            } else {
                ""
            };
            let full_body_lower = String::from_utf8_lossy(&raw_body).to_lowercase();
            let kind = if full_body_lower.contains("context_length_exceeded")
                || full_body_lower.contains("maximum context length")
            {
                ErrorKind::ContextOverflow
            } else if status.as_u16() == 401 {
                ErrorKind::Auth
            } else if status.as_u16() == 429 {
                ErrorKind::RateLimit
            } else {
                ErrorKind::Unknown
            };
            warn!(status = %status, retryable, "API error response");
            return Err(ProviderError {
                message: format!("API error {}{}: {}", status, hint, body_preview),
                retryable,
                kind,
            });
        }

        let resp_body: serde_json::Value = serde_json::from_slice(&raw_body).map_err(|e| {
            let preview = String::from_utf8_lossy(&raw_body[..raw_body.len().min(200)]);
            warn!(error = %e, body_preview = %preview, "response JSON parse error");
            ProviderError {
                message: format!("JSON parse error: {} (body starts with: {})", e, preview),
                retryable: false,
                kind: ErrorKind::Unknown,
            }
        })?;

        Self::parse_response(&resp_body)
    }

    fn name(&self) -> &str {
        "openai"
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
        let json = OpenAiProvider::serialize_tools(&tools);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let tool = &arr[0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "buffer_read");
        assert_eq!(tool["function"]["description"], "Read buffer");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
        assert!(tool["function"]["parameters"]["properties"]["start_line"].is_object());
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
        let json = OpenAiProvider::serialize_messages(&messages, "You are helpful");
        let arr = json.as_array().unwrap();
        // System message + 2 user/assistant messages
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "You are helpful");
        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[2]["role"], "assistant");
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
        let json = OpenAiProvider::serialize_messages(&messages, "sys");
        let arr = json.as_array().unwrap();
        // System + tool result
        assert_eq!(arr[1]["role"], "tool");
        assert_eq!(arr[1]["tool_call_id"], "call_123");
        assert_eq!(arr[1]["content"], "result text");
    }

    #[test]
    fn parse_response_text_only() {
        let body = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "Hello world"},
                "finish_reason": "stop",
            }],
        });
        let resp = OpenAiProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Hello world"));
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_response_tool_calls() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "buffer_read",
                            "arguments": "{\"start_line\":1}",
                        },
                    }],
                },
                "finish_reason": "tool_calls",
            }],
        });
        let resp = OpenAiProvider::parse_response(&body).unwrap();
        assert!(resp.text.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_abc");
        assert_eq!(resp.tool_calls[0].name, "buffer_read");
        assert_eq!(resp.tool_calls[0].arguments["start_line"], 1);
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_stop_with_tool_calls_infers_tool_use() {
        // Some providers return "stop" instead of "tool_calls" but still have tool_calls
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Let me check",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "cursor_info",
                            "arguments": "{}",
                        },
                    }],
                },
                "finish_reason": "stop",
            }],
        });
        let resp = OpenAiProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Let me check"));
        assert_eq!(resp.tool_calls.len(), 1);
        // Even though finish_reason is "stop", we infer ToolUse from presence of tool_calls
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_max_tokens() {
        let body = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "truncated..."},
                "finish_reason": "length",
            }],
        });
        let resp = OpenAiProvider::parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }
}
