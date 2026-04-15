use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

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
            client: Client::new(),
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
                    let tool_calls: Vec<serde_json::Value> = calls
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
                        .collect();
                    result.push(json!({
                        "role": "assistant",
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
            })?;

        let message = choice.get("message").ok_or_else(|| ProviderError {
            message: "Missing 'message' in choice".into(),
            retryable: false,
        })?;

        let text = message
            .get("content")
            .and_then(|c| c.as_str())
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

        Ok(ProviderResponse {
            text,
            tool_calls,
            stop_reason,
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
                }
            })?;

        let status = response.status();
        debug!(status = %status, "OpenAI API response received");

        let resp_body: serde_json::Value = response.json().await.map_err(|e| {
            warn!(error = %e, "OpenAI response JSON parse error");
            ProviderError {
                message: format!("JSON parse error: {}", e),
                retryable: false,
            }
        })?;

        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
            warn!(status = %status, retryable, "OpenAI API error response");
            return Err(ProviderError {
                message: format!("API error {}: {}", status, resp_body),
                retryable,
            });
        }

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
