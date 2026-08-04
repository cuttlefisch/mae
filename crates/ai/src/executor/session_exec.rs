//! Dispatch for the six session-scoped AI tools (decision #9, ADR-091).
//!
//! `ai_set_mode`, `ai_set_profile`, `ai_set_budget`, `log_activity`,
//! `read_transcript`, and `web_fetch` were advertised over MCP but handled
//! only inside `AgentSession`'s own loop, so an external `tools/call` for any
//! of them fell through `dispatch_tool` to `Unknown tool`. What blocked
//! wiring them was not the tools — it was that `dispatch_tool` had no session
//! handle at all. ADR-091 adds one (`Editor::agent_session_mut`, resolved
//! from the MCP session id the dispatch scope now records), and this module
//! is what consumes it.
//!
//! # What "the right session" means here
//!
//! With an MCP session in scope, every one of these acts on *that client's*
//! record: two connected agents have independent modes, profiles, budgets,
//! activity logs, and transcripts. With no session in scope — MAE's own
//! embedded agent, `--self-test` — the record is the process-wide one, and
//! `set_agent_mode`/`set_agent_profile` additionally move the editor's
//! `ai_mode`/`ai_profile` options, which is exactly the effect
//! `AiEvent::UpdateMode`/`UpdateProfile` already produce when the embedded
//! session runs the same tool. An MCP call is not a weaker version of the
//! same tool.
//!
//! # What this module deliberately does NOT do
//!
//! It does not reach into a *running* `AgentSession`'s private fields. That
//! task owns `self.budget`/`self.current_mode` by value on another thread and
//! there is no command channel for either; changing that is a separate piece
//! of work (see ADR-091's "Consequences"). The embedded session continues to
//! intercept all six in `handle_prompt.rs` before they ever reach here, so
//! nothing about its behaviour changes.

use mae_core::Editor;

use crate::types::ToolCall;

pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "ai_set_mode" => execute_ai_set_mode(editor, &call.arguments),
        "ai_set_profile" => execute_ai_set_profile(editor, &call.arguments),
        "ai_set_budget" => execute_ai_set_budget(editor, &call.arguments),
        "log_activity" => execute_log_activity(editor, &call.arguments),
        "read_transcript" => execute_read_transcript(editor),
        "web_fetch" => execute_web_fetch(&call.arguments),
        _ => return None,
    };
    Some(result)
}

pub fn execute_ai_set_mode(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'mode' argument")?;
    editor.set_agent_mode(mode)
}

pub fn execute_ai_set_profile(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let profile = args
        .get("profile")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'profile' argument")?;
    // Validate against the same list the embedded session's `ai_set_profile`
    // tool advertises, so an external client cannot install a profile name
    // that has no prompt behind it.
    if !crate::tools::AI_PROFILES.contains(&profile) {
        return Err(format!(
            "Unknown profile '{profile}' (expected one of: {})",
            crate::tools::AI_PROFILES.join(", ")
        ));
    }
    editor.set_agent_profile(profile)
}

pub fn execute_ai_set_budget(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let warn = args.get("warn").and_then(|v| v.as_f64());
    let cap = args.get("cap").and_then(|v| v.as_f64());
    if warn.is_none() && cap.is_none() {
        // Deliberately avoids this codebase's reserved "absent argument"
        // phrasing (the one every other impl above uses). That phrasing is
        // what `dispatch_contract_tests::schema_impl_params_agree` reads to
        // decide a parameter is *unconditionally* required, and neither of
        // these is — either one alone is a valid call. Using it here would
        // make the test demand both be declared `required`, which would be a
        // lie to the agent. (The comment avoids it too: the test scans source
        // text, not just string literals.)
        return Err("ai_set_budget needs at least one of 'warn' or 'cap' (USD, 0 to clear)".into());
    }
    // Matching `handle_prompt.rs`'s semantics exactly: a non-positive value
    // clears the threshold rather than setting an unreachable one.
    let session = editor.agent_session_mut();
    if let Some(w) = warn {
        session.budget_warn_usd = if w > 0.0 { Some(w) } else { None };
    }
    if let Some(c) = cap {
        session.budget_cap_usd = if c > 0.0 { Some(c) } else { None };
    }
    let (w, c) = (session.budget_warn_usd, session.budget_cap_usd);
    Ok(format!(
        "Budget updated: warn={}, cap={}",
        w.map(|v| format!("${v:.2}")).unwrap_or("none".into()),
        c.map(|v| format!("${v:.2}")).unwrap_or("none".into()),
    ))
}

pub fn execute_log_activity(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let activity = args
        .get("activity")
        .and_then(|v| v.as_str())
        .unwrap_or("Thinking...")
        .to_string();
    editor.agent_session_mut().push_activity(activity.clone());
    // Surface it the way the embedded session's `AiEvent::ToolCallFinished`
    // does: this tool exists so the human can see what the agent is doing,
    // and an external agent's narration is exactly as worth showing as the
    // embedded one's.
    editor.set_status(activity.clone());
    Ok(activity)
}

pub fn execute_read_transcript(editor: &mut Editor) -> Result<String, String> {
    match editor.agent_session().transcript_path.clone() {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read transcript file {}: {e}", path.display())),
        None => Err(
            "No transcript is being recorded for this session. MAE writes transcripts for \
             its own embedded agent sessions; an external MCP client's transcript lives on \
             the client side."
                .into(),
        ),
    }
}

/// Blocking counterpart of `AgentSession::execute_web_fetch`.
///
/// `dispatch_tool` runs synchronously on the main thread (it holds
/// `&mut Editor`, which is `!Send`), so the session's `async fn` cannot be
/// reused directly. The *policy* — scheme allow-list, HTML stripping, 32 KB
/// truncation — is shared through `crate::web::{validate_url, shape_body}`
/// rather than reimplemented, so the two transports cannot drift on what they
/// accept or how much they return (principle #8). `reqwest::blocking` in a
/// dispatch path is the existing precedent — `kb_enrich`'s embedding calls
/// (`tool_impls/kb.rs`) already work this way.
pub fn execute_web_fetch(args: &serde_json::Value) -> Result<String, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'url' argument")?;
    crate::web::validate_url(url)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(crate::web::TIMEOUT_SECS))
        .user_agent(crate::web::USER_AGENT)
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let response = client.get(url).send().map_err(|e| {
        if e.is_timeout() {
            format!(
                "Request timed out after {} seconds",
                crate::web::TIMEOUT_SECS
            )
        } else {
            format!("HTTP request failed: {e}")
        }
    })?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let body = response
        .text()
        .map_err(|e| format!("Failed to read response body: {e}"))?;
    Ok(crate::web::shape_body(status, &content_type, body))
}
