use std::time::Duration;

use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::openai::OpenAiProvider;
use crate::provider::ErrorKind;
use crate::provider::*;
use crate::types::*;

/// Ollama native API provider (`/api/chat`).
///
/// Ollama also exposes an OpenAI-compatible `/v1/chat/completions` endpoint
/// (which `OpenAiProvider` can talk to via `base_url`), but that shim does
/// not forward Ollama-specific fields — notably `think`, the toggle for
/// hybrid-reasoning models (Qwen3 and later, DeepSeek-R1, etc.) to skip their
/// reasoning trace. Only the native endpoint honors it. This provider exists
/// so `ai_thinking` actually works for Ollama; everything else about it
/// (tool-calling, message roles) is otherwise equivalent to the OpenAI shape,
/// so tool-schema serialization is reused from [`OpenAiProvider`].
pub struct OllamaProvider {
    client: Client,
    config: ProviderConfig,
}

impl OllamaProvider {
    pub fn new(config: ProviderConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        OllamaProvider { client, config }
    }

    /// Convert canonical ToolCalls to Ollama's native format. Unlike OpenAI,
    /// arguments are a native JSON object, not a JSON-encoded string, and an
    /// `id` is not part of the wire format — MAE's `ToolCall.id` is a local
    /// correlation handle only, not sent to Ollama.
    fn serialize_tool_calls(calls: &[ToolCall]) -> Vec<serde_json::Value> {
        calls
            .iter()
            .map(|call| {
                json!({
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments,
                    },
                })
            })
            .collect()
    }

    /// Convert canonical Messages to Ollama's native message format. Same
    /// role/content shape as OpenAI's, except tool-call arguments stay a
    /// native object (see `serialize_tool_calls`).
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
                        "content": tr.output,
                    }));
                }
                _ => {}
            }
        }

        serde_json::Value::Array(result)
    }

    /// Parse Ollama's native `/api/chat` response into canonical ProviderResponse.
    pub fn parse_response(body: &serde_json::Value) -> Result<ProviderResponse, ProviderError> {
        let message = body.get("message").ok_or_else(|| ProviderError {
            message: "Missing 'message' in response".into(),
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
                    .enumerate()
                    .filter_map(|(idx, call)| {
                        let func = call.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        // Ollama's native format has no call id and gives
                        // arguments as an object, not a JSON-encoded string.
                        let id = call
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("ollama_call_{idx}"));
                        let arguments = func.get("arguments").cloned().unwrap_or(json!({}));
                        Some(ToolCall {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Ollama doesn't have a distinct "tool_calls" done_reason — it
        // reports "stop" regardless, so infer ToolUse from presence of
        // tool_calls, same fallback OpenAiProvider uses for lenient providers.
        let done_reason = body
            .get("done_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("stop");

        let stop_reason = match done_reason {
            "length" => StopReason::MaxTokens,
            _ => {
                if tool_calls.is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                }
            }
        };

        // Native API reports counts at the top level, not nested under "usage".
        let usage = if body.get("prompt_eval_count").is_some() || body.get("eval_count").is_some() {
            Some(Usage {
                prompt_tokens: body
                    .get("prompt_eval_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                completion_tokens: body.get("eval_count").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            })
        } else {
            None
        };

        Ok(ProviderResponse {
            text,
            tool_calls,
            stop_reason,
            usage,
        })
    }
}

#[async_trait::async_trait]
impl AgentProvider for OllamaProvider {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let url = format!(
            "{}/api/chat",
            self.config
                .base_url
                .as_deref()
                .unwrap_or("http://localhost:11434")
        );

        let mut body = json!({
            "model": self.config.model,
            "stream": false,
            "messages": Self::serialize_messages(messages, system_prompt),
        });

        if !tools.is_empty() {
            // Ollama documents its tool schema as OpenAI-compatible.
            body["tools"] = OpenAiProvider::serialize_tools(tools);
        }

        // Generation params live under "options" in the native API, not at
        // the top level (mirrors Modelfile PARAMETER semantics).
        let mut options = serde_json::Map::new();
        options.insert("num_predict".into(), json!(self.config.max_tokens));
        if let Some(temp) = self.config.temperature {
            options.insert("temperature".into(), json!(temp));
        }
        body["options"] = serde_json::Value::Object(options);

        if let Some(thinking) = &self.config.thinking {
            body["think"] = match thinking.as_str() {
                "true" => json!(true),
                "false" => json!(false),
                other => json!(other), // "high" | "medium" | "low"
            };
        }

        debug!(model = %self.config.model, url = %url, message_count = messages.len(), tool_count = tools.len(), "sending Ollama API request");

        let mut request = self
            .client
            .post(&url)
            .header("content-type", "application/json");
        if let Some(key) = self.config.api_key.as_deref().filter(|k| !k.is_empty()) {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        let response = request.json(&body).send().await.map_err(|e| {
            warn!(error = %e, is_timeout = e.is_timeout(), "Ollama HTTP error");
            ProviderError {
                message: format!("HTTP error: {}", e),
                retryable: e.is_timeout(),
                kind: ErrorKind::Transport,
            }
        })?;

        let status = response.status();
        debug!(status = %status, "Ollama API response received");

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
            let full_body_lower = body_preview.to_lowercase();
            let kind = if full_body_lower.contains("context") && full_body_lower.contains("length")
            {
                ErrorKind::ContextOverflow
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                ErrorKind::Auth
            } else if status.as_u16() == 429 {
                ErrorKind::RateLimit
            } else {
                ErrorKind::Unknown
            };
            warn!(status = %status, retryable, "API error response");
            return Err(ProviderError {
                message: format!("API error {}: {}", status, body_preview),
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
                kind: ErrorKind::Transport,
            }
        })?;

        Self::parse_response(&resp_body)
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let json = OllamaProvider::serialize_messages(&messages, "You are helpful");
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "You are helpful");
        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[2]["role"], "assistant");
    }

    #[test]
    fn serialize_tool_calls_keeps_arguments_as_object() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "buffer_read".into(),
            arguments: json!({"start_line": 1}),
        }];
        let serialized = OllamaProvider::serialize_tool_calls(&calls);
        assert_eq!(serialized[0]["function"]["name"], "buffer_read");
        // Must be a native object, not a JSON-encoded string (unlike OpenAI).
        assert!(serialized[0]["function"]["arguments"].is_object());
        assert_eq!(serialized[0]["function"]["arguments"]["start_line"], 1);
        // Ollama's wire format has no call id field.
        assert!(serialized[0].get("id").is_none());
    }

    #[test]
    fn parse_response_text_only() {
        let body = json!({
            "message": {"role": "assistant", "content": "Hello world"},
            "done": true,
            "done_reason": "stop",
        });
        let resp = OllamaProvider::parse_response(&body).unwrap();
        assert_eq!(resp.text.as_deref(), Some("Hello world"));
        assert!(resp.tool_calls.is_empty());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_response_tool_calls_without_id_or_stringified_arguments() {
        let body = json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": "buffer_read",
                        "arguments": {"start_line": 1},
                    },
                }],
            },
            "done": true,
            "done_reason": "stop",
        });
        let resp = OllamaProvider::parse_response(&body).unwrap();
        assert!(resp.text.is_none());
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "buffer_read");
        assert_eq!(resp.tool_calls[0].arguments["start_line"], 1);
        // Synthesized since Ollama doesn't provide one.
        assert_eq!(resp.tool_calls[0].id, "ollama_call_0");
        // Inferred from tool_calls presence, since done_reason stays "stop".
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn parse_response_max_tokens() {
        let body = json!({
            "message": {"role": "assistant", "content": "truncated..."},
            "done": true,
            "done_reason": "length",
        });
        let resp = OllamaProvider::parse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn parse_response_usage_from_top_level_counts() {
        let body = json!({
            "message": {"role": "assistant", "content": "OK"},
            "done": true,
            "done_reason": "stop",
            "prompt_eval_count": 15,
            "eval_count": 3,
        });
        let resp = OllamaProvider::parse_response(&body).unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 15);
        assert_eq!(usage.completion_tokens, 3);
    }

    #[test]
    fn parse_response_missing_message_errors() {
        let body = json!({"done": true});
        assert!(OllamaProvider::parse_response(&body).is_err());
    }
}
