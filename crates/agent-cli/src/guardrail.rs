//! Provider-agnostic guardrail layer (ADR-045's four pillars), harness-side per
//! ADR-046 — wraps any [`AgentProvider`] without touching MAE core. Applied by
//! the harness to any non-`Verified`-tier model (transparent passthrough is
//! just... not wrapping — there's no separate no-op variant needed).
//!
//! **Documented trade-off** (ADR-048 §8): this only covers the harness's own
//! sessions. Embedded `delegate()` sub-agents on non-Verified models remain
//! unguarded after the `construct_provider()` dispatch-bug fix — accepted
//! because the embedded surface is frozen per ADR-046. `GuardrailProvider`
//! wraps the same trait every provider already implements, so extending it to
//! the embedded path later is additive, not a redesign.
//!
//! Implements 3 of the 4 pillars directly; the 4th (context/budget-aware
//! compaction) is approximated here as message-count trimming sent to the
//! provider on each call — see `compact_messages`.

use std::sync::Mutex;

use mae_ai::{
    AgentProvider, Message, MessageContent, ProviderError, ProviderResponse, Role, StopReason,
    ToolCall, ToolDefinition,
};

/// How many trailing messages to actually send the provider on each turn
/// (oldest ones are dropped from the *outgoing request*, not from the
/// caller's canonical history — `AgentProvider::send` takes `&[Message]`, so
/// this can only shape what's sent, not truncate the caller's own Vec).
const DEFAULT_MAX_MESSAGES_SENT: usize = 60;

/// Consecutive identical (name, arguments) tool calls before the loop-
/// detection guardrail intervenes.
const LOOP_DETECTION_THRESHOLD: usize = 3;

pub struct GuardrailProvider<P: AgentProvider> {
    inner: P,
    max_messages_sent: usize,
    recent_calls: Mutex<Vec<(String, serde_json::Value)>>,
}

impl<P: AgentProvider> GuardrailProvider<P> {
    pub fn wrap(inner: P) -> Self {
        Self {
            inner,
            max_messages_sent: DEFAULT_MAX_MESSAGES_SENT,
            recent_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl<P: AgentProvider> AgentProvider for GuardrailProvider<P> {
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        let compacted = compact_messages(messages, self.max_messages_sent);
        let mut response = self.inner.send(&compacted, tools, system_prompt).await?;

        // Pillar: rescue-parsing. A weak model sometimes dumps a tool call as
        // prose JSON instead of using structured tool-calling — recover it
        // rather than treating an empty `tool_calls` as "the model is done."
        if response.tool_calls.is_empty() {
            if let Some((remaining_text, calls)) =
                rescue_parse_stray_tool_call(response.text.as_deref().unwrap_or(""))
            {
                response.text = (!remaining_text.is_empty()).then_some(remaining_text);
                response.tool_calls = calls;
                response.stop_reason = StopReason::ToolUse;
            }
        }

        // Pillar: targeted retry nudge. The model produced nothing usable at
        // all (no text, no tool call, not even a rescuable one) — one
        // specific, corrective re-prompt, not a blind resubmission.
        if response.tool_calls.is_empty()
            && response.text.as_deref().unwrap_or("").trim().is_empty()
        {
            let mut nudge_messages = compacted.clone();
            nudge_messages.push(Message {
                role: Role::User,
                content: MessageContent::Text(
                    "Your last response was empty. Call one of the available tools to make \
                     progress, or reply with a short summary of what you've done so far."
                        .to_string(),
                ),
            });
            if let Ok(retry) = self.inner.send(&nudge_messages, tools, system_prompt).await {
                response = retry;
            }
        }

        // Pillar: step/loop enforcement. A weak model can get stuck calling
        // the exact same tool with the exact same arguments repeatedly —
        // block it with an explicit, informative stop rather than burning
        // rounds (and, for a priced provider, budget) on a stuck loop.
        if let Some(call) = response.tool_calls.first() {
            let mut recent = self.recent_calls.lock().unwrap();
            recent.push((call.name.clone(), call.arguments.clone()));
            if recent.len() > LOOP_DETECTION_THRESHOLD {
                recent.remove(0);
            }
            if recent.len() == LOOP_DETECTION_THRESHOLD
                && recent
                    .iter()
                    .all(|(n, a)| *n == call.name && *a == call.arguments)
            {
                let name = call.name.clone();
                drop(recent);
                return Ok(ProviderResponse {
                    text: Some(format!(
                        "Guardrail: '{name}' was called {LOOP_DETECTION_THRESHOLD} times in a \
                         row with identical arguments — stopping to avoid an infinite loop. Try \
                         a different approach or ask the user for guidance."
                    )),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    usage: response.usage,
                });
            }
        } else {
            self.recent_calls.lock().unwrap().clear();
        }

        Ok(response)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// Keep only the trailing `max` messages, always preserving the very first
/// message (typically the initial user turn / objective) so context about
/// *what* the session is for isn't lost purely to keep it short.
fn compact_messages(messages: &[Message], max: usize) -> Vec<Message> {
    if messages.len() <= max {
        return messages.to_vec();
    }
    let mut out = Vec::with_capacity(max);
    out.push(messages[0].clone());
    let tail_start = messages.len() - (max - 1);
    out.extend_from_slice(&messages[tail_start.max(1)..]);
    out
}

/// A tool call a model emitted as prose JSON instead of structured
/// tool-calling. Recognizes `{"name": "...", "arguments": {...}}`, optionally
/// wrapped in a ```` ```json ... ``` ```` fence, appearing anywhere in `text`.
/// Returns the text with that JSON removed plus the recovered call(s), or
/// `None` if nothing recognizable is found.
fn rescue_parse_stray_tool_call(text: &str) -> Option<(String, Vec<ToolCall>)> {
    #[derive(serde::Deserialize)]
    struct StrayCall {
        name: String,
        #[serde(default)]
        arguments: serde_json::Value,
    }

    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let candidate = &text[start..=end];
    let parsed: StrayCall = serde_json::from_str(candidate).ok()?;

    let remaining = format!("{}{}", &text[..start], &text[end + 1..]);
    let remaining = remaining.trim().to_string();
    Some((
        remaining,
        vec![ToolCall {
            id: "rescued_0".to_string(),
            name: parsed.name,
            arguments: parsed.arguments,
        }],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_ai::Role;

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn compact_messages_noop_under_limit() {
        let messages = vec![text_msg(Role::User, "a"), text_msg(Role::Assistant, "b")];
        let out = compact_messages(&messages, 60);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn compact_messages_keeps_first_and_trims_middle() {
        let messages: Vec<Message> = (0..100)
            .map(|i| text_msg(Role::User, &format!("msg-{i}")))
            .collect();
        let out = compact_messages(&messages, 10);
        assert_eq!(out.len(), 10);
        // First message preserved (the original objective).
        assert!(matches!(&out[0].content, MessageContent::Text(t) if t == "msg-0"));
        // Last message is still the true most-recent one.
        assert!(matches!(&out[9].content, MessageContent::Text(t) if t == "msg-99"));
    }

    #[test]
    fn rescue_parses_stray_tool_call_json() {
        let text =
            r#"Let me search for that. {"name": "kb_search", "arguments": {"query": "buffer"}}"#;
        let (remaining, calls) = rescue_parse_stray_tool_call(text).unwrap();
        assert_eq!(remaining, "Let me search for that.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "kb_search");
        assert_eq!(calls[0].arguments["query"], "buffer");
    }

    #[test]
    fn rescue_parse_returns_none_for_plain_text() {
        assert!(rescue_parse_stray_tool_call("just a normal reply, no JSON here").is_none());
    }

    #[test]
    fn rescue_parse_returns_none_for_unrelated_json() {
        // Valid JSON, but not our {name, arguments} shape — must not misfire.
        assert!(rescue_parse_stray_tool_call(r#"here's some data: {"foo": "bar"}"#).is_none());
    }
}
