//! Provider-agnostic guardrail layer (ADR-045's four pillars) — wraps any
//! [`AgentProvider`] without touching the wrapped provider's own logic.
//! Lives in `mae-ai` (not `mae-agent-cli`) so both the `mae-agent` CLI
//! harness (ADR-046) *and* MAE's embedded `delegate()` sub-agent path can
//! apply the same hardening to non-`Verified`-tier models. Transparent
//! passthrough is just... not wrapping — there's no separate no-op variant
//! needed.
//!
//! Implements 3 of the 4 pillars directly; the 4th (context/budget-aware
//! compaction) is approximated here as message-count trimming sent to the
//! provider on each call — see `compact_messages`. A 5th, opt-in mechanism
//! (`StagePolicy` / `ToolStage`) adds workflow-stage tracking: a caller can
//! declare which tools are "Discovery", "Read", or "Write" for a given
//! workflow, and a premature Write (no prior Discovery/Read this session)
//! gets the same corrective-nudge-and-retry treatment as an empty response,
//! instead of being silently forwarded. Default (`stage_policy: None`) is a
//! complete no-op — zero behavior change for callers that don't opt in.

use std::sync::Mutex;

use crate::{
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

/// Coarse workflow stage a tool call belongs to, for [`StagePolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStage {
    /// Broad search / enumeration tools (e.g. `kb_search`).
    Discovery,
    /// Targeted single-item fetch tools (e.g. `kb_get`).
    Read,
    /// Mutating tools (e.g. `kb_add_link`).
    Write,
}

/// An optional, opt-in policy classifying tool names into [`ToolStage`]s so
/// `GuardrailProvider` can reject a premature Write (no prior Discovery/Read
/// this session) rather than silently forwarding it. A tool not covered by
/// the policy classifies as `None` and is exempt from stage checking.
#[derive(Clone)]
pub struct StagePolicy {
    pub classify: fn(&str) -> Option<ToolStage>,
}

pub struct GuardrailProvider {
    inner: Box<dyn AgentProvider>,
    max_messages_sent: usize,
    recent_calls: Mutex<Vec<(String, serde_json::Value)>>,
    stage_policy: Option<StagePolicy>,
}

impl GuardrailProvider {
    pub fn wrap(inner: Box<dyn AgentProvider>) -> Self {
        Self {
            inner,
            max_messages_sent: DEFAULT_MAX_MESSAGES_SENT,
            recent_calls: Mutex::new(Vec::new()),
            stage_policy: None,
        }
    }

    /// Attach a [`StagePolicy`] so premature Write calls (no prior
    /// Discovery/Read this session) get a corrective nudge instead of being
    /// forwarded as-is. Without this, `GuardrailProvider` never does stage
    /// checking (default `None` — zero behavior change).
    pub fn with_stage_policy(mut self, policy: StagePolicy) -> Self {
        self.stage_policy = Some(policy);
        self
    }
}

#[async_trait::async_trait]
impl AgentProvider for GuardrailProvider {
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
                eprintln!(
                    "guardrail: rescued a stray tool-call ({}) the provider emitted as prose instead of a structured call",
                    calls.first().map(|c| c.name.as_str()).unwrap_or("?")
                );
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
            eprintln!("guardrail: empty response -- sending one corrective retry nudge");
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

        // Pillar: stage/workflow enforcement (opt-in via `stage_policy`). A
        // Write-stage tool call with no prior Discovery/Read this session is
        // almost certainly premature (the model guessed instead of looking
        // first) — nudge it back rather than forwarding a blind write.
        if let Some(policy) = &self.stage_policy {
            if let Some(write_call) = response
                .tool_calls
                .iter()
                .find(|c| matches!((policy.classify)(&c.name), Some(ToolStage::Write)))
            {
                let has_prior_discovery_or_read = {
                    let recent = self.recent_calls.lock().unwrap_or_else(|e| e.into_inner());
                    recent.iter().any(|(name, _)| {
                        matches!(
                            (policy.classify)(name),
                            Some(ToolStage::Discovery) | Some(ToolStage::Read)
                        )
                    })
                };
                if !has_prior_discovery_or_read {
                    let name = write_call.name.clone();
                    eprintln!(
                        "guardrail: rejected premature '{name}' -- no prior Discovery/Read call this session, sending corrective nudge"
                    );
                    let mut nudge_messages = compacted.clone();
                    nudge_messages.push(Message {
                        role: Role::User,
                        content: MessageContent::Text(format!(
                            "You tried to call '{name}' but haven't searched or read anything yet \
                             this session. Call a search/read tool first, then retry."
                        )),
                    });
                    if let Ok(retry) = self.inner.send(&nudge_messages, tools, system_prompt).await
                    {
                        response = retry;
                    }
                }
            }
        }

        // Pillar: step/loop enforcement. A weak model can get stuck calling
        // the exact same tool with the exact same arguments repeatedly —
        // block it with an explicit, informative stop rather than burning
        // rounds (and, for a priced provider, budget) on a stuck loop.
        if let Some(call) = response.tool_calls.first() {
            let mut recent = self.recent_calls.lock().unwrap_or_else(|e| e.into_inner());
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
                eprintln!(
                    "guardrail: loop detected -- '{name}' called {LOOP_DETECTION_THRESHOLD} times in a row with identical arguments, stopping"
                );
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
            self.recent_calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
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
    use crate::Usage;

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

    // ---- StagePolicy / stage-tracking pillar ----

    /// A scripted fake provider: returns each queued response in order on
    /// successive `send()` calls, ignoring input. Lets tests assert on the
    /// *sequence* of responses (original call, then the nudged retry).
    struct ScriptedProvider {
        responses: Mutex<Vec<ProviderResponse>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for ScriptedProvider {
        async fn send(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system_prompt: &str,
        ) -> Result<ProviderResponse, ProviderError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Ok(ProviderResponse {
                    text: Some("done".to_string()),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                });
            }
            Ok(responses.remove(0))
        }

        fn name(&self) -> &str {
            "scripted"
        }
    }

    fn tool_call_response(name: &str) -> ProviderResponse {
        ProviderResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_0".into(),
                name: name.to_string(),
                arguments: serde_json::json!({}),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Some(Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
        }
    }

    fn kb_enrichment_policy() -> StagePolicy {
        StagePolicy {
            classify: |name| match name {
                "kb_search" | "kb_search_context" | "kb_agenda" => Some(ToolStage::Discovery),
                "kb_get" | "kb_links_from" => Some(ToolStage::Read),
                "kb_add_link" | "kb_set_role" => Some(ToolStage::Write),
                _ => None,
            },
        }
    }

    #[tokio::test]
    async fn stage_policy_rejects_premature_write_and_retries() {
        // First response is a premature Write (no prior Discovery/Read) --
        // the guardrail must not forward it, and instead should use the
        // *second* scripted response (the "retry").
        let provider = ScriptedProvider::new(vec![
            tool_call_response("kb_add_link"),
            tool_call_response("kb_search"),
        ]);
        let guardrail =
            GuardrailProvider::wrap(Box::new(provider)).with_stage_policy(kb_enrichment_policy());

        let messages = vec![text_msg(Role::User, "enrich the KB")];
        let response = guardrail
            .send(&messages, &[], "system prompt")
            .await
            .unwrap();

        // The premature write was rejected -- what came back is the retry's
        // response (kb_search), not the original kb_add_link call.
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "kb_search");
    }

    #[tokio::test]
    async fn stage_policy_allows_write_after_prior_discovery() {
        // First turn: a Discovery call goes through untouched. Second turn
        // (same guardrail instance, so recent_calls retains history): a
        // Write call is now allowed since Discovery already happened.
        let provider = ScriptedProvider::new(vec![
            tool_call_response("kb_search"),
            tool_call_response("kb_add_link"),
        ]);
        let guardrail =
            GuardrailProvider::wrap(Box::new(provider)).with_stage_policy(kb_enrichment_policy());

        let messages = vec![text_msg(Role::User, "enrich the KB")];
        let first = guardrail
            .send(&messages, &[], "system prompt")
            .await
            .unwrap();
        assert_eq!(first.tool_calls[0].name, "kb_search");

        let second = guardrail
            .send(&messages, &[], "system prompt")
            .await
            .unwrap();
        // Write call went through as-is -- prior Discovery satisfied the policy.
        assert_eq!(second.tool_calls.len(), 1);
        assert_eq!(second.tool_calls[0].name, "kb_add_link");
    }

    #[tokio::test]
    async fn no_stage_policy_is_a_complete_noop() {
        // Default (`stage_policy: None`): a Write-classified tool name with
        // zero prior calls must be forwarded exactly as the inner provider
        // returned it -- identical to `GuardrailProvider`'s behavior before
        // stage tracking existed.
        let provider = ScriptedProvider::new(vec![tool_call_response("kb_add_link")]);
        let guardrail = GuardrailProvider::wrap(Box::new(provider));

        let messages = vec![text_msg(Role::User, "enrich the KB")];
        let response = guardrail
            .send(&messages, &[], "system prompt")
            .await
            .unwrap();

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "kb_add_link");
    }
}
