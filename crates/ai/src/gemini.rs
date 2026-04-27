use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::provider::ErrorKind;
use crate::provider::*;
use crate::types::*;

/// Gemini (Google Generative AI) API provider.
pub struct GeminiProvider {
    client: Client,
    config: ProviderConfig,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        GeminiProvider { client, config }
    }

    /// Convert canonical ToolDefinition to Gemini's tool format.
    pub fn serialize_tools(tools: &[ToolDefinition]) -> serde_json::Value {
        let function_declarations: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": {
                        "type": t.parameters.schema_type,
                        "properties": t.parameters.properties.iter().map(|(k, v)| {
                            let mut prop = json!({
                                "type": v.prop_type,
                                "description": v.description,
                            });
                            if let Some(ref enums) = v.enum_values {
                                prop["enum"] = json!(enums);
                            }
                            (k.clone(), prop)
                        }).collect::<serde_json::Map<String, serde_json::Value>>(),
                        "required": t.parameters.required,
                    }
                })
            })
            .collect();

        json!([{ "function_declarations": function_declarations }])
    }

    fn serialize_function_call_parts(calls: &[ToolCall]) -> Vec<serde_json::Value> {
        calls
            .iter()
            .map(|call| {
                json!({
                    "function_call": {
                        "name": call.name,
                        "args": call.arguments,
                    }
                })
            })
            .collect()
    }

    /// Convert canonical Messages to Gemini's content format.
    pub fn serialize_messages(messages: &[Message]) -> serde_json::Value {
        let mut result = Vec::new();

        for msg in messages {
            match (&msg.role, &msg.content) {
                (Role::User, MessageContent::Text(text)) => {
                    result.push(json!({
                        "role": "user",
                        "parts": [{ "text": text }],
                    }));
                }
                (Role::Assistant, MessageContent::Text(text)) => {
                    result.push(json!({
                        "role": "model",
                        "parts": [{ "text": text }],
                    }));
                }
                (Role::Assistant, MessageContent::ToolCalls(calls)) => {
                    let parts = Self::serialize_function_call_parts(calls);
                    result.push(json!({
                        "role": "model",
                        "parts": parts,
                    }));
                }
                (
                    Role::Assistant,
                    MessageContent::TextWithToolCalls {
                        text,
                        tool_calls: calls,
                    },
                ) => {
                    let mut parts = vec![json!({ "text": text })];
                    parts.extend(Self::serialize_function_call_parts(calls));
                    result.push(json!({
                        "role": "model",
                        "parts": parts,
                    }));
                }
                (Role::Tool, MessageContent::ToolResult(tr)) => {
                    // Gemini expects tool results in a "function" role or similar,
                    // but in the generateContent API, it's often user role or
                    // a dedicated function role depending on the specific API version.
                    // For v1beta, it's a part with function_response.
                    result.push(json!({
                        "role": "function", // Gemini uses 'function' role for tool results
                        "parts": [{
                            "function_response": {
                                "name": tr.tool_name,
                                "response": {
                                    "result": tr.output,
                                }
                            }
                        }],
                    }));
                }
                _ => {} // Skip unsupported combinations
            }
        }

        serde_json::Value::Array(result)
    }

    /// Parse Gemini's response into canonical ProviderResponse.
    pub fn parse_response(body: &serde_json::Value) -> Result<ProviderResponse, ProviderError> {
        let candidates = body
            .get("candidates")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ProviderError {
                message: format!("Missing 'candidates' in Gemini response: {}", body),
                retryable: false,
                kind: ErrorKind::Unknown,
            })?;

        if candidates.is_empty() {
            return Err(ProviderError {
                message: "Empty candidates in Gemini response".into(),
                retryable: false,
                kind: ErrorKind::Unknown,
            });
        }

        let first_candidate = &candidates[0];
        let content = first_candidate
            .get("content")
            .ok_or_else(|| ProviderError {
                message: "Missing 'content' in Gemini candidate".into(),
                retryable: false,
                kind: ErrorKind::Unknown,
            })?;

        let parts = content
            .get("parts")
            .and_then(|p| p.as_array())
            .ok_or_else(|| ProviderError {
                message: "Missing 'parts' in Gemini content".into(),
                retryable: false,
                kind: ErrorKind::Unknown,
            })?;

        let mut text_parts = Vec::new();
        let mut tool_calls = Vec::new();

        for (i, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                text_parts.push(text.to_string());
            } else if let Some(fc) = part.get("function_call") {
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = match fc.get("args").cloned() {
                    Some(v) => v,
                    None => {
                        warn!(tool = %name, "Gemini function_call missing 'args' field, defaulting to {{}}");
                        json!({})
                    }
                };
                // Gemini doesn't provide unique IDs for tool calls in the same way Claude does.
                // We'll generate one or use the index.
                let id = format!("call_{}_{}", name, i);
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }

        let finish_reason = first_candidate.get("finishReason").and_then(|s| s.as_str());
        let stop_reason = match finish_reason {
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => {
                if !tool_calls.is_empty() {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                }
            }
        };

        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };

        let usage = body.get("usageMetadata").map(|u| Usage {
            prompt_tokens: u
                .get("promptTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            completion_tokens: u
                .get("candidatesTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
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
impl AgentProvider for GeminiProvider {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let api_key = self.config.api_key.as_deref().unwrap_or("");
        let url = if let Some(ref base) = self.config.base_url {
            format!(
                "{}/models/{}:generateContent?key={}",
                base, self.config.model, api_key
            )
        } else {
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
                self.config.model, api_key
            )
        };

        let mut body = json!({
            "contents": Self::serialize_messages(messages),
            "generationConfig": {
                "maxOutputTokens": self.config.max_tokens,
                "temperature": self.config.temperature.unwrap_or(0.7),
            }
        });

        if !system_prompt.is_empty() {
            body["system_instruction"] = json!({
                "parts": [{ "text": system_prompt }]
            });
        }

        if !tools.is_empty() {
            body["tools"] = Self::serialize_tools(tools);
            body["tool_config"] = json!({
                "function_calling_config": {
                    "mode": "AUTO"
                }
            });
        }

        debug!(model = %self.config.model, message_count = messages.len(), tool_count = tools.len(), "sending Gemini API request");

        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, is_timeout = e.is_timeout(), "Gemini HTTP error");
                ProviderError {
                    message: format!("HTTP error: {}", e),
                    retryable: e.is_timeout(),
                    kind: ErrorKind::Transport,
                }
            })?;

        let status = response.status();
        debug!(status = %status, "Gemini API response received");

        let resp_body: serde_json::Value = response.json().await.map_err(|e| {
            warn!(error = %e, "Gemini response JSON parse error");
            ProviderError {
                message: format!("JSON parse error: {}", e),
                retryable: false,
                kind: ErrorKind::Transport,
            }
        })?;

        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
            warn!(status = %status, retryable, "Gemini API error response: {}", resp_body);
            let body_lower = resp_body.to_string().to_ascii_lowercase();
            let kind = if body_lower.contains("context_length")
                || body_lower.contains("token limit")
                || body_lower.contains("too long")
            {
                ErrorKind::ContextOverflow
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                ErrorKind::Auth
            } else if status.as_u16() == 429 {
                ErrorKind::RateLimit
            } else {
                ErrorKind::Unknown
            };
            return Err(ProviderError {
                message: format!("API error {}: {}", status, resp_body),
                retryable,
                kind,
            });
        }

        Self::parse_response(&resp_body)
    }

    fn name(&self) -> &str {
        "gemini"
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
        let json = GeminiProvider::serialize_tools(&tools);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let fd = &arr[0]["function_declarations"][0];
        assert_eq!(fd["name"], "buffer_read");
        assert_eq!(fd["description"], "Read buffer");
        assert_eq!(fd["parameters"]["type"], "object");
        assert!(fd["parameters"]["properties"]["start_line"].is_object());
        assert_eq!(fd["parameters"]["required"][0], "start_line");
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
        let json = GeminiProvider::serialize_messages(&messages);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["role"], "user");
        assert_eq!(arr[0]["parts"][0]["text"], "hello");
        assert_eq!(arr[1]["role"], "model");
        assert_eq!(arr[1]["parts"][0]["text"], "hi");
    }

    #[test]
    fn parse_response_text_only() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello world"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5
            }
        });
        let resp = GeminiProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Hello world"));
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
    }

    #[test]
    fn parse_response_tool_use() {
        let body = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "function_call": {
                            "name": "buffer_read",
                            "args": {"start_line": 1}
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        });
        let resp = GeminiProvider::parse_response(&body).unwrap();
        assert!(resp.text.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "buffer_read");
        assert_eq!(resp.tool_calls[0].arguments["start_line"], 1);
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }
}
