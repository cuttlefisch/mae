//! ADR-061 Phase C/E: an Ollama `/api/embed` call, split into a pure,
//! dependency-light half (request-body construction and response parsing —
//! always available, no `reqwest` dependency) and an optional blocking-HTTP
//! half (gated behind the `remote-hub` feature, which unlocks the same
//! optional `reqwest` dependency ADR-062's `RemoteHubQueryLayer` already
//! uses).
//!
//! The pure half is shared between `crates/ai::OllamaProvider::embed` (the
//! editor's chat-adjacent ASYNC caller, ADR-061 Phase A) and the daemon's own
//! ASYNC enrichment sweep (Phase C) — the daemon workspace is explicitly
//! excluded from the editor workspace precisely to avoid pulling `mae-core`
//! (ADR-014), so it cannot reuse `OllamaProvider` directly and instead builds
//! its own async HTTP call around this shared request/response shaping.
//!
//! [`ollama_embed_blocking`] is the third caller: the editor's `daemon_mode=
//! off` enrich-now path (Phase E), which ADR-061's own Decision text
//! specifies as running "synchronously with respect to the invoking
//! command" — a genuinely BLOCKING call, not a bridge into async from sync
//! code (which risks a "cannot start a runtime from within a runtime" panic
//! depending on the caller's own execution context). `crates/ai` enables
//! `remote-hub` unconditionally (not behind its own opt-in feature) so this
//! path is always compiled into the shipped `mae` binary — Phase E's
//! "genuinely usable floor" requirement (CLAUDE.md principle #12) means it
//! cannot be gated behind an extra opt-in build flag most users would never
//! pass.

use serde::{Deserialize, Serialize};

/// A minimal, transport-agnostic embedding error — deliberately not
/// `crates/ai::ProviderError` (which lives in a crate the daemon cannot
/// depend on). Callers with their own richer error type (e.g. `ProviderError`)
/// convert from this at their own boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedError {
    pub message: String,
    /// Whether retrying the same request might succeed (e.g. a transient
    /// transport error or 5xx/429), mirroring `crates/ai::ProviderError`'s
    /// own `retryable` field so callers can map this 1:1.
    pub retryable: bool,
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for EmbedError {}

/// Build the outbound `/api/embed` request body — Ollama's native batch
/// embedding endpoint, which accepts an `input` array directly and returns
/// `embeddings` (plural) in the same order (verified against Ollama's own API
/// docs during Phase A, not assumed).
pub fn build_ollama_embed_request(model: &str, inputs: &[String]) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": inputs,
    })
}

/// Parse Ollama's native `/api/embed` response body into a list of embedding
/// vectors, one per input, in the same order as the request's `input` array.
pub fn parse_ollama_embed_response(body: &serde_json::Value) -> Result<Vec<Vec<f32>>, EmbedError> {
    let embeddings = body
        .get("embeddings")
        .and_then(|v| v.as_array())
        .ok_or_else(|| EmbedError {
            message: format!(
                "Ollama embed response missing 'embeddings' array (body: {})",
                body
            ),
            retryable: false,
        })?;

    embeddings
        .iter()
        .map(|vec_val| {
            vec_val
                .as_array()
                .ok_or_else(|| EmbedError {
                    message: "Ollama embed response entry was not an array".to_string(),
                    retryable: false,
                })?
                .iter()
                .map(|n| {
                    n.as_f64().map(|f| f as f32).ok_or_else(|| EmbedError {
                        message: "Ollama embed response contained a non-numeric component"
                            .to_string(),
                        retryable: false,
                    })
                })
                .collect::<Result<Vec<f32>, EmbedError>>()
        })
        .collect()
}

/// ADR-061 Phase E: a genuinely blocking Ollama `/api/embed` call, for the
/// editor's `daemon_mode=off` enrich-now command — a low-priority,
/// user-invoked action the ADR's own Decision text specifies as synchronous.
/// Uses `reqwest::blocking::Client` (never bridges to the async
/// `OllamaProvider`), so it is safe to call from a plain synchronous tool
/// executor regardless of whether an async runtime happens to be running on
/// the calling thread.
#[cfg(feature = "remote-hub")]
pub fn ollama_embed_blocking(
    client: &reqwest::blocking::Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let url = format!("{base_url}/api/embed");
    let body = build_ollama_embed_request(model, inputs);

    let mut request = client.post(&url).header("content-type", "application/json");
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let response = request.json(&body).send().map_err(|e| EmbedError {
        message: format!("HTTP error: {e}"),
        retryable: e.is_timeout(),
    })?;

    let status = response.status();
    let resp_body: serde_json::Value = response.json().map_err(|e| EmbedError {
        message: format!("failed to read/parse response body: {e}"),
        retryable: false,
    })?;

    if !status.is_success() {
        let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
        return Err(EmbedError {
            message: format!("API error {status}: {resp_body}"),
            retryable,
        });
    }

    parse_ollama_embed_response(&resp_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_carries_model_and_inputs_verbatim() {
        let req = build_ollama_embed_request("nomic-embed-text", &["hello".to_string()]);
        assert_eq!(req["model"], "nomic-embed-text");
        assert_eq!(req["input"], serde_json::json!(["hello"]));
    }

    #[test]
    fn parse_embed_response_single_input() {
        let body = serde_json::json!({"embeddings": [[0.1, 0.2, 0.3]]});
        let result = parse_ollama_embed_response(&body).unwrap();
        assert_eq!(result, vec![vec![0.1f32, 0.2, 0.3]]);
    }

    #[test]
    fn parse_embed_response_batch_preserves_order() {
        let body = serde_json::json!({"embeddings": [[1.0, 0.0], [0.0, 1.0], [0.5, 0.5]]});
        let result = parse_ollama_embed_response(&body).unwrap();
        assert_eq!(result, vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]]);
    }

    #[test]
    fn parse_embed_response_missing_embeddings_field_errors() {
        let body = serde_json::json!({"not_embeddings": []});
        let err = parse_ollama_embed_response(&body).unwrap_err();
        assert!(err.message.contains("missing 'embeddings'"));
        assert!(!err.retryable);
    }

    #[test]
    fn parse_embed_response_non_numeric_component_errors() {
        let body = serde_json::json!({"embeddings": [["not", "a", "number"]]});
        let err = parse_ollama_embed_response(&body).unwrap_err();
        assert!(err.message.contains("non-numeric"));
    }

    #[test]
    fn parse_embed_response_empty_batch_is_empty_not_an_error() {
        let body = serde_json::json!({"embeddings": []});
        let result = parse_ollama_embed_response(&body).unwrap();
        assert!(result.is_empty());
    }
}
