//! Shared AI event handling for terminal and GUI loops.
//!
//! Both event loops need identical logic for dispatching AI events
//! (tool calls, text responses, streaming, cost updates, budget warnings).
//! This module provides a single implementation to avoid the duplication
//! that historically plagues editor event loops (see: Emacs xdisp.c).

use mae_ai::{
    execute_tool_with_requester, AgentProvider, AgentSession, AiCommand, AiEvent, DeferredKind,
    ExecuteResult, PermissionPolicy, PermissionTier, ToolResult,
};
use mae_core::{Editor, InputLock};
use mae_lsp::LspCommand;
use tracing::{debug, error, info, warn};

use crate::bootstrap::{
    build_system_prompt, find_conversation_buffer_mut, load_ai_config, spawn_ai_session,
};

fn find_buffer_by_name_or_default_mut<'a>(
    editor: &'a mut Editor,
    name: Option<&str>,
) -> Option<&'a mut mae_core::conversation::Conversation> {
    if let Some(n) = name {
        if let Some(idx) = editor.find_buffer_by_name(n) {
            return editor.buffers[idx].conversation_mut();
        }
    }
    find_conversation_buffer_mut(editor)
}

/// Decide whether a sub-agent provider needs ADR-045's guardrail hardening
/// wrapped around it before it's handed an unsupervised tool-calling loop.
/// `Verified`-tier models are passed through untouched; anything else
/// (`Testing`/`Untested`) gets wrapped in [`mae_ai::GuardrailProvider`].
///
/// Extracted from the `AiEvent::Delegate` arm so this specific decision —
/// the one closing GitHub issue #310's "embedded delegate sub-agents get
/// zero guardrail protection" gap — is unit-testable without constructing
/// a full `&mut Editor` + `AiEventContext`.
fn guardrail_wrap_if_needed(
    verification: mae_ai::ModelVerification,
    provider: Box<dyn AgentProvider>,
) -> Box<dyn AgentProvider> {
    if matches!(verification, mae_ai::ModelVerification::Verified) {
        provider
    } else {
        Box::new(mae_ai::GuardrailProvider::wrap(provider))
    }
}

/// Type alias for the deferred AI reply state held across loop iterations.
pub type DeferredAiReply = Option<(
    DeferredKind,
    String, // tool_call_id
    tokio::sync::oneshot::Sender<ToolResult>,
    tokio::time::Instant, // created_at
)>;

/// DAP deferred resolution phase — tracks multi-stage async pipelines.
/// Unlike LSP (single event → resolve), DAP has a cascade:
/// Stopped → RefreshThreadsAndStack → StackTraceResult.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum DapDeferredPhase {
    /// Waiting for initial event (Stopped/Terminated/SessionStarted).
    WaitingForEvent,
    /// DapStart got SessionStarted with stop_on_entry — awaiting Stopped event.
    WaitingForStop,
    /// Got Stopped — now waiting for StackTraceResult (the refresh cascade).
    WaitingForStackTrace,
}

/// State for a deferred DAP tool call (the "promise").
pub struct DeferredDapState {
    pub kind: DeferredKind,
    pub phase: DapDeferredPhase,
    pub tool_call_id: String,
    pub reply: tokio::sync::oneshot::Sender<ToolResult>,
    pub created_at: tokio::time::Instant,
    /// Whether this DapStart was launched with stop_on_entry=true.
    pub stop_on_entry: bool,
}

/// Type alias for the deferred DAP reply state.
pub type DeferredDapReply = Option<DeferredDapState>;

/// Deferred MCP reply state — supports multiple concurrent deferred calls.
/// Each entry tracks its `DeferredKind`, reply channel, and creation time.
pub type DeferredMcpReply = Vec<(
    DeferredKind,
    tokio::sync::oneshot::Sender<mae_mcp::McpToolResult>,
    tokio::time::Instant, // created_at
)>;

/// A pending interactive AI request waiting for user input.
pub enum PendingInteractiveEvent {
    AskUser(tokio::sync::oneshot::Sender<String>),
    ProposeChanges(tokio::sync::oneshot::Sender<bool>),
    /// ADR-090 D3: the embedded editor's implementation of `Ask`. Resolved by
    /// `:ai-accept` / `:ai-reject`, the same two commands that already resolve
    /// `ProposeChanges` — a new permission prompt would have been a fourth
    /// vocabulary (principle #15), so this reuses the one that exists.
    ///
    /// @ai-caution: [security] Every path that *drops* this without answering
    /// must leave the sender un-signalled, not send `true`. The session treats
    /// a closed channel as a refusal (`decide_and_present`), so dropping is
    /// safe; sending `true` on a cancel would not be.
    ConfirmToolCall(tokio::sync::oneshot::Sender<bool>),
}

/// Shared reference to the MCP client manager for external tool dispatch.
pub type McpClientMgrRef =
    std::sync::Arc<tokio::sync::Mutex<mae_mcp::client_mgr::McpClientManager>>;

/// Context required for AI event dispatching.
pub struct AiEventContext<'a> {
    pub all_tools: &'a [mae_ai::ToolDefinition],
    pub permission_policy: &'a mae_ai::PermissionPolicy,
    pub deferred_ai_reply: &'a mut DeferredAiReply,
    pub deferred_dap_reply: &'a mut DeferredDapReply,
    pub pending_interactive_event: &'a mut Option<PendingInteractiveEvent>,
    pub lsp_command_tx: &'a tokio::sync::mpsc::Sender<LspCommand>,
    pub dap_command_tx: &'a tokio::sync::mpsc::Sender<mae_dap::DapCommand>,
    pub ai_event_tx: &'a tokio::sync::mpsc::Sender<AiEvent>,
    pub scheme: &'a mut mae_scheme::SchemeRuntime,
    pub mcp_client_mgr: &'a McpClientMgrRef,
}

/// Handle a single AI event. Shared between terminal and GUI loops.
pub fn handle_ai_event(editor: &mut Editor, ai_event: AiEvent, ctx: AiEventContext) {
    match ai_event {
        AiEvent::ToolCallRequest {
            call,
            reply,
            approved_tier,
        } => {
            editor.ai.streaming = true;
            info!(tool = %call.name, call_id = %call.id, "executing AI tool call");
            // Update the existing Pending entry (created by ToolCallStarted) to Running,
            // rather than creating a duplicate entry.
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.update_or_push_tool_call(
                    &call.name,
                    mae_core::conversation::ToolCallState::Running,
                );
            }
            // Intercept mcp_* external tool calls — dispatch async via client manager
            if let Some(rest) = call.name.strip_prefix("mcp_") {
                if let Some((server, tool)) = rest.split_once('_') {
                    let mgr = ctx.mcp_client_mgr.clone();
                    let server_name = server.to_string();
                    let tool_name = tool.to_string();
                    let call_id = call.id.clone();
                    let call_name = call.name.clone();
                    let arguments = call.arguments.clone();
                    tokio::spawn(async move {
                        let result = {
                            let mgr = mgr.lock().await;
                            mgr.call_tool(&server_name, &tool_name, arguments).await
                        };
                        let tool_result = match result {
                            Ok(output) => mae_ai::ToolResult {
                                tool_call_id: call_id,
                                tool_name: call_name,
                                success: true,
                                output,
                            },
                            Err(e) => mae_ai::ToolResult {
                                tool_call_id: call_id,
                                tool_name: call_name,
                                success: false,
                                output: e,
                            },
                        };
                        let _ = reply.send(tool_result);
                    });
                    return;
                }
            }

            let tool_start = std::time::Instant::now();
            // ADR-048: the embedded/delegate path's provider is authoritative —
            // MAE constructed it itself — so the check keys on it directly.
            let provider = editor.ai.provider.clone();
            let exec_result = match crate::ai_residency::check_kb_residency(
                editor,
                &call.name,
                &call.arguments,
                Some(provider.as_str()),
            ) {
                crate::ai_residency::ResidencyDecision::Deny(reason) => {
                    ExecuteResult::Immediate(ToolResult {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        success: false,
                        output: reason,
                    })
                }
                crate::ai_residency::ResidencyDecision::Allow => {
                    // ADR-090: `approved_tier` is set only when the session
                    // already showed this exact call to a human at this exact
                    // tier and got a yes. `with_one_time_approval` raises the
                    // auto-approval ceiling and NOTHING else, so the hard
                    // ceiling and the category allowlist re-decide here
                    // regardless -- an approval cannot promote a `Deny`.
                    let policy = match approved_tier {
                        Some(tier) => ctx.permission_policy.with_one_time_approval(tier),
                        None => ctx.permission_policy.clone(),
                    };
                    execute_tool_with_requester(
                        editor,
                        &call,
                        ctx.all_tools,
                        &policy,
                        Some(provider.as_str()),
                        // No MCP session -- this is the embedded human/delegate
                        // AI path (ADR-051 scopes per-session dispatch to real
                        // external MCP clients only).
                        None,
                    )
                }
            };
            // Drain any pending Scheme evals queued by the tool (e.g. eval_scheme).
            let scheme_output = drain_pending_scheme_evals(
                editor,
                ctx.scheme,
                ctx.permission_policy.ambient_scheme_tier(),
            );
            match exec_result {
                ExecuteResult::Immediate(mut result) => {
                    // If the tool queued a Scheme eval, replace the output with the
                    // result. ADR-086/#590.2: a queued eval that errored must not
                    // clobber a prior refusal/failure with success:true.
                    if let Some((output, all_ok)) = scheme_output {
                        result.output = output;
                        result.success = all_ok;
                    }
                    let elapsed = tool_start.elapsed().as_millis() as u64;
                    info!(
                        tool = %call.name,
                        duration_ms = elapsed,
                        success = result.success,
                        "AI tool completed"
                    );
                    if let Some(conv) = find_conversation_buffer_mut(editor) {
                        conv.complete_last_tool_call(result.success, &result.output, Some(elapsed));
                    }
                    if reply.send(result).is_err() {
                        warn!("AI tool result channel closed before reply");
                    }
                    // Drain any DAP intents queued by immediate tools (e.g. dap_set_breakpoint)
                    // so they take effect immediately rather than batching with the next deferred.
                    if editor.has_pending_dap_intents() {
                        crate::dap_bridge::drain_dap_intents(editor, ctx.dap_command_tx);
                    }
                }
                // ADR-090: the session pre-asks (see `decide_and_present`),
                // so reaching here means the session's own gate said `Allow`
                // while dispatch said `Ask` -- the two disagree only if a
                // policy changed mid-turn, or if a caller bypassed the
                // session. Either way nothing ran; deny explicitly rather
                // than re-entering an async prompt from the main thread.
                ExecuteResult::NeedsApproval(req) => {
                    let result = req.into_denied(EMBEDDED_RACE_SURFACE);
                    warn!(tool = %result.tool_name, "embedded AI tool call needed approval at dispatch time");
                    if let Some(conv) = find_conversation_buffer_mut(editor) {
                        conv.complete_last_tool_call(false, &result.output, None);
                    }
                    if reply.send(result).is_err() {
                        warn!("AI tool result channel closed before reply");
                    }
                }
                ExecuteResult::Deferred { kind, .. } => {
                    if kind.is_dap() {
                        info!(?kind, "deferred AI tool — awaiting DAP response");
                        crate::dap_bridge::drain_dap_intents(editor, ctx.dap_command_tx);
                        let stop_on_entry = kind == DeferredKind::DapStart
                            && call
                                .arguments
                                .get("stop_on_entry")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                        *ctx.deferred_dap_reply = Some(DeferredDapState {
                            kind,
                            phase: DapDeferredPhase::WaitingForEvent,
                            tool_call_id: call.id.clone(),
                            reply,
                            created_at: tokio::time::Instant::now(),
                            stop_on_entry,
                        });
                    } else {
                        info!(?kind, "deferred AI tool — awaiting LSP response");
                        crate::scheme_lsp_bridge::drain_scheme_lsp_intents(editor, ctx.scheme);
                        crate::lsp_bridge::drain_lsp_intents(editor, ctx.lsp_command_tx);
                        *ctx.deferred_ai_reply =
                            Some((kind, call.id.clone(), reply, tokio::time::Instant::now()));
                    }
                }
            }
        }
        AiEvent::TextResponse {
            text,
            target_buffer,
        } => {
            editor.ai.streaming = true;
            if let Some(conv_buf) =
                find_buffer_by_name_or_default_mut(editor, target_buffer.as_deref())
            {
                conv_buf.push_assistant(&text);
            } else {
                // ADR-087 / audit #594: AI model output is arbitrary UTF-8; a
                // fixed byte cut can land mid-character and panic.
                let display = if text.len() > 120 {
                    let cut = mae_core::grapheme::floor_char_boundary(&text, 117);
                    format!("[AI] {}...", &text[..cut])
                } else {
                    format!("[AI] {}", text)
                };
                editor.set_status(display);
            }
            editor.sync_conversation_buffer_rope();
            crate::key_handling::conversation::scroll_output_to_bottom(editor);
        }
        AiEvent::ToolCallStarted { name } => {
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_tool_call_with_state(
                    &name,
                    mae_core::conversation::ToolCallState::Pending,
                );
            }
            editor.sync_conversation_buffer_rope();
            crate::key_handling::conversation::scroll_output_to_bottom(editor);
        }
        AiEvent::ToolCallFinished { success, output } => {
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                // Auto-expand plans and large writes for better parity with Claude Code/Cursor
                let expanded = if let Some(last) = conv.entries.last() {
                    match &last.role {
                        mae_core::conversation::ConversationRole::ToolCall { name, .. } => {
                            matches!(
                                name.as_str(),
                                "create_plan" | "update_plan" | "write_file" | "replace"
                            )
                        }
                        _ => false,
                    }
                } else {
                    false
                };
                conv.complete_last_tool_call(success, &output, None);
                if expanded {
                    if let Some(last) = conv.entries.last_mut() {
                        last.collapsed = false;
                    }
                }
            }
            editor.sync_conversation_buffer_rope();
            crate::key_handling::conversation::scroll_output_to_bottom(editor);
        }
        AiEvent::StreamChunk {
            text,
            target_buffer,
        } => {
            editor.ai.streaming = true;
            if let Some(conv_buf) =
                find_buffer_by_name_or_default_mut(editor, target_buffer.as_deref())
            {
                conv_buf.append_streaming_chunk(&text);
            }
            // Sync rope + scroll, but throttle to avoid per-chunk overhead.
            editor.sync_conversation_buffer_rope();
            let should_scroll = editor
                .ai
                .last_output_scroll
                .map(|t| t.elapsed() >= std::time::Duration::from_millis(50))
                .unwrap_or(true);
            if should_scroll {
                crate::key_handling::conversation::scroll_output_to_bottom(editor);
                editor.ai.last_output_scroll = Some(std::time::Instant::now());
            }
        }
        AiEvent::SessionComplete {
            text: _text,
            target_buffer,
            transcript_path,
        } => {
            info!("AI session complete");
            if let Some(conv_buf) =
                find_buffer_by_name_or_default_mut(editor, target_buffer.as_deref())
            {
                conv_buf.end_streaming();
                if let Some(ref path) = transcript_path {
                    conv_buf.push_system(format!("Transcript saved to: {}", path));
                }
            }
            editor.sync_conversation_buffer_rope();
            // Explicit scroll-to-bottom on session complete — the common epilogue
            // also scrolls, but this ensures it happens before state restore.
            crate::key_handling::conversation::scroll_output_to_bottom(editor);
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            editor.ai.work_window.set(None);
            editor.ai.last_output_scroll = None;

            // Auto-restore editor state and clean up sandbox after self-test session.
            if editor.cleanup_self_test() {
                editor.set_status("[AI] Done — state restored");
            } else {
                editor.set_status("[AI] Done");
            }
        }
        AiEvent::CostUpdate {
            session_usd,
            tokens_in,
            tokens_out,
            cache_read_tokens,
            cache_creation_tokens,
            context_window,
            context_used_tokens,
            turn_tokens_in,
            turn_tokens_out,
            turn_cache_read,
            latency_ms,
            ..
        } => {
            editor.ai.session_cost_usd = session_usd;
            editor.ai.session_tokens_in = tokens_in;
            editor.ai.session_tokens_out = tokens_out;
            editor.ai.cache_read_tokens = cache_read_tokens;
            editor.ai.cache_creation_tokens = cache_creation_tokens;
            editor.ai.context_window = context_window;
            editor.ai.context_used_tokens = context_used_tokens;
            // Network diagnostics
            editor.ai.last_api_success = Some(std::time::Instant::now());
            editor.ai.last_api_latency_ms = Some(latency_ms);
            editor.ai.api_call_count += 1;
            // Attach per-turn usage to the last assistant entry.
            if turn_tokens_in > 0 || turn_tokens_out > 0 {
                if let Some(conv) = find_conversation_buffer_mut(editor) {
                    // Walk backwards to find the last assistant entry.
                    for entry in conv.entries.iter_mut().rev() {
                        if matches!(
                            entry.role,
                            mae_core::conversation::ConversationRole::Assistant
                        ) {
                            entry.token_usage = Some(mae_core::conversation::TokenUsage {
                                input: turn_tokens_in as u32,
                                output: turn_tokens_out as u32,
                                cache_read: turn_cache_read as u32,
                            });
                            break;
                        }
                    }
                    conv.rebuild_render_cache();
                }
            }
        }
        AiEvent::BudgetWarning {
            session_usd,
            threshold_usd,
        } => {
            let msg = format!(
                "AI budget warning: session spend ${:.4} crossed ${:.2} threshold",
                session_usd, threshold_usd
            );
            warn!(session_usd, threshold_usd, "AI budget threshold crossed");
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.push_system(msg.clone());
            }
            editor.set_status(msg);
        }
        AiEvent::BudgetExceeded {
            session_usd,
            cap_usd,
        } => {
            let msg = format!(
                "AI budget exceeded: session spend ${:.4} reached cap ${:.2}. \
                 Raise `ai.budget.session_hard_cap_usd` in config.toml or restart \
                 the editor to reset.",
                session_usd, cap_usd
            );
            error!(session_usd, cap_usd, "AI session hard cap reached");
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.push_system(msg.clone());
                conv_buf.end_streaming();
            }
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            editor.set_status(msg);
        }
        AiEvent::AskUser { question, reply } => {
            info!(%question, "AI asking user");
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_system(format!("AI Question: {}", question));
                conv.end_streaming();
            }
            editor.set_status(format!("AI: {}", question));
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            *ctx.pending_interactive_event = Some(PendingInteractiveEvent::AskUser(reply));
        }
        AiEvent::ConfirmToolCall {
            tool_name,
            arguments,
            tier,
            auto_approve_up_to,
            reply,
        } => {
            let prompt = mae_ai::ask_message(&tool_name, tier, auto_approve_up_to);
            info!(tool = %tool_name, ?tier, "AI tool call awaiting approval");
            // `auto-accept` mode is the human having pre-answered every
            // prompt for this session -- the same opt-in `ProposeChanges`
            // already honours. It is NOT a policy override: a `Deny` never
            // reaches here (the session refuses it outright), so auto-accept
            // can only ever auto-answer an `Ask`.
            if editor.ai.mode == "auto-accept" {
                if let Some(conv) = find_conversation_buffer_mut(editor) {
                    conv.push_system(format!("{prompt} Auto-accepted (ai-mode=auto-accept)."));
                }
                let _ = reply.send(true);
                return;
            }
            let args_preview = serde_json::to_string(&arguments).unwrap_or_default();
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_system(format!(
                    "{prompt}\n  args: {args_preview}\n  Approve with :ai-accept, refuse with :ai-reject."
                ));
                conv.end_streaming();
            }
            editor.set_status(format!("{prompt} :ai-accept / :ai-reject"));
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            *ctx.pending_interactive_event = Some(PendingInteractiveEvent::ConfirmToolCall(reply));
        }
        AiEvent::ProposeChanges { changes, reply } => {
            let count = if let Some(arr) = changes.as_array() {
                arr.len()
            } else {
                1
            };
            info!(count, "AI proposing changes");

            // Auto-accept mode: skip manual approval
            if editor.ai.mode == "auto-accept" {
                info!("Auto-accepting AI changes");
                if let Some(conv) = find_conversation_buffer_mut(editor) {
                    conv.push_system(format!("Auto-accepted changes to {} file(s)", count));
                }
                let _ = reply.send(true);
                return;
            }

            // 1. Generate diff text
            let diff_text = render_changes_to_diff(&changes);

            // 2. Create/Update *AI-Diff* buffer
            let diff_buf_name = "*AI-Diff*";
            let buf_idx = match editor.find_buffer_by_name(diff_buf_name) {
                Some(idx) => idx,
                None => {
                    let mut b = mae_core::Buffer::new();
                    b.name = diff_buf_name.to_string();
                    b.kind = mae_core::BufferKind::Diff;
                    editor.buffers.push(b);
                    editor.buffers.len() - 1
                }
            };
            editor.buffers[buf_idx].replace_contents(&diff_text);
            editor.display_buffer_and_focus(buf_idx);

            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_system(format!(
                    "AI proposed changes to {} file(s). Review the *AI-Diff* buffer, then use :ai-accept or :ai-reject.",
                    count
                ));
                conv.end_streaming();
            }
            editor.set_status(format!("AI: Proposing changes to {} file(s)", count));
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            *ctx.pending_interactive_event = Some(PendingInteractiveEvent::ProposeChanges(reply));
        }
        AiEvent::NetworkDiagnostic(result) => {
            let status = if result.reachable {
                format!(
                    "[AI] Network OK \u{2014} {}ms to {}",
                    result.latency_ms, result.endpoint
                )
            } else {
                format!(
                    "[AI] Network FAIL \u{2014} {}",
                    result.error.as_deref().unwrap_or("unknown")
                )
            };
            editor.set_status(&status);
            editor.ai.last_network_check = Some(mae_core::editor::AiNetworkCheck {
                endpoint: result.endpoint,
                reachable: result.reachable,
                http_status: result.http_status,
                latency_ms: result.latency_ms,
                error: result.error,
            });
        }
        AiEvent::Delegate {
            profile,
            objective,
            reply,
        } => {
            let session_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let target_buf_name = format!("*AI-{}-{}*", profile, session_id);
            info!(%profile, %target_buf_name, "AI delegating to sub-agent");

            // Create a dedicated conversation buffer for the sub-agent.
            // Users can switch to this buffer to monitor progress in real-time.
            let sub_buf = mae_core::Buffer::new_conversation(&target_buf_name);
            editor.buffers.push(sub_buf);
            if let Some(conv) = find_buffer_by_name_or_default_mut(editor, Some(&target_buf_name)) {
                conv.push_system(format!("Objective: {}", objective));
            }

            // Initialize the sub-agent session using the parent's configuration.
            let config = match load_ai_config(editor) {
                Some(c) => c,
                None => {
                    let _ = reply.send(ToolResult {
                        tool_call_id: "delegate".into(),
                        tool_name: "delegate".into(),
                        success: false,
                        output: "AI not configured".into(),
                    });
                    return;
                }
            };

            let (sub_cmd_tx, sub_cmd_rx) = tokio::sync::mpsc::channel::<AiCommand>(8);
            let (proxy_tx, mut proxy_rx) = tokio::sync::mpsc::channel::<AiEvent>(32);
            let main_event_tx = ctx.ai_event_tx.clone();

            let provider =
                crate::bootstrap::construct_provider(&config.provider_type, config.clone());

            // Reliability hardening for non-Verified-tier models (ADR-045's
            // guardrail pillars): rescue-parsing malformed tool-call JSON, a
            // one-time retry nudge on an empty response, and loop detection.
            // The embedded primary session (`setup_ai()`) intentionally does
            // NOT get this treatment in this pass -- a deliberate, documented
            // scope boundary, not an oversight. This sub-agent delegate path
            // is in scope because it's the one place besides the `mae-agent`
            // CLI harness that can hand a weak/local model an unsupervised
            // tool-calling loop.
            let verification = mae_ai::context_limits::lookup(&config.model).verification;
            let provider = guardrail_wrap_if_needed(verification, provider);

            // Scope verifier tools: read-only + shell, no write/create/modify.
            let all_tools = {
                let mut t = mae_ai::tools_from_registry(&editor.commands);
                t.extend(mae_ai::ai_specific_tools(&editor.option_registry));
                t
            };
            let tools = if profile == "verifier" {
                all_tools
                    .into_iter()
                    .filter(|t| {
                        matches!(
                            t.name.as_str(),
                            "buffer_read"
                                | "project_search"
                                | "project_files"
                                | "project_info"
                                | "run_test"
                                | "run_build"
                                | "shell_exec"
                                | "cursor_info"
                                | "editor_state"
                                | "list_buffers"
                                | "kb_search"
                                | "kb_get"
                                | "introspect"
                                | "lsp_diagnostics"
                                | "open_file"
                                | "file_read"
                                | "read_messages"
                                | "model_exam"
                                | "self_test_suite"
                        ) || t.name.starts_with("command_")
                    })
                    .collect()
            } else {
                all_tools
            };

            let effective_tier = {
                let (file_cfg, _) = crate::config::load_config();
                file_cfg
                    .ai
                    .prompt_tier
                    .as_deref()
                    .map(mae_ai::context_limits::ModelTier::parse_tier)
                    .unwrap_or_else(|| mae_ai::context_limits::tier(&config.model))
            };

            let mut sub_prompt = build_system_prompt(&profile, effective_tier);
            let provider_hint = mae_ai::context_limits::ProviderHint::from_model(&config.model);
            if let Some(hints) = provider_hint.prompt_hints() {
                sub_prompt.push_str(hints);
            }

            // ADR-084 D2: a `delegate()` sub-agent inherits the parent's
            // policy verbatim. It is not a fresh principal -- it exists only
            // because the parent asked for it, so it must not be able to
            // reach an effect the parent could not.
            let sub_session = AgentSession::new(provider, tools, sub_prompt, proxy_tx, sub_cmd_rx)
                .with_budget(config.model, config.budget)
                .with_permission_policy(ctx.permission_policy.clone())
                .with_target_buffer(target_buf_name.clone());

            // Spawn the sub-agent session.
            spawn_ai_session(sub_session);

            // Proxy task: monitor the sub-agent and relay events back to the main thread.
            // Captures the final SessionComplete or Error to resolve the `delegate` tool call.
            tokio::spawn(async move {
                let _ = sub_cmd_tx.send(AiCommand::Prompt(objective)).await;

                while let Some(evt) = proxy_rx.recv().await {
                    match &evt {
                        AiEvent::SessionComplete { text, .. } => {
                            let _ = reply.send(ToolResult {
                                tool_call_id: "delegate".into(),
                                tool_name: "delegate".into(),
                                success: true,
                                output: text.clone(),
                            });
                            let _ = main_event_tx.send(evt).await;
                            break;
                        }
                        AiEvent::Error(msg, _) => {
                            let _ = reply.send(ToolResult {
                                tool_call_id: "delegate".into(),
                                tool_name: "delegate".into(),
                                success: false,
                                output: format!("Sub-agent error: {}", msg),
                            });
                            let _ = main_event_tx.send(evt).await;
                            break;
                        }
                        _ => {
                            // Relay streaming chunks and tool calls to the main event loop
                            if main_event_tx.send(evt).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
        AiEvent::UpdateMode(mode) => {
            info!(%mode, "AI requested mode update");
            let _ = editor.set_option("ai-mode", &mode);
            crate::config::persist_editor_preference("ai.mode", &mode);
        }
        AiEvent::UpdateProfile(profile) => {
            info!(%profile, "AI requested profile update");
            let _ = editor.set_option("ai-profile", &profile);
            crate::config::persist_editor_preference("ai.profile", &profile);
            // Profile changes require session rebuild to reload prompt.
            // This is handled by the main thread noticing the change.
        }
        AiEvent::RoundUpdate {
            round,
            transaction_start_idx,
        } => {
            editor.ai.current_round = round;
            editor.ai.transaction_start_idx = transaction_start_idx;
        }
        AiEvent::EventMeta {
            session_id,
            agent_name,
        } => {
            debug!(%session_id, %agent_name, "AI event metadata received");
        }
        AiEvent::Error(msg, transcript_path) => {
            error!(error = %msg, "AI error event");
            editor.ai.last_api_error = Some(msg.clone());
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.push_system(format!("Error: {}", msg));
                if let Some(ref path) = transcript_path {
                    conv_buf.push_system(format!("Transcript saved to: {}", path));
                }
                conv_buf.end_streaming();
            }
            editor.ai.streaming = false;
            editor.ai.input_lock = InputLock::None;
            editor.set_status(format!("AI Error: {}", msg));
        }
    }

    // After every AI event that may have mutated conversation state,
    // sync the output rope and auto-scroll the output window to bottom
    // — but only if the user hasn't scroll-locked during streaming.
    editor.sync_conversation_buffer_rope();
    let is_scroll_locked = editor
        .ai
        .conversation_pair
        .as_ref()
        .and_then(|p| editor.buffers.get(p.output_buffer_idx))
        .and_then(|b| b.conversation())
        .map(|conv| conv.scroll_locked)
        .unwrap_or(false);
    if !is_scroll_locked {
        crate::key_handling::conversation::scroll_output_to_bottom(editor);
    }
}

fn render_changes_to_diff(changes: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(arr) = changes.as_array() {
        for change in arr {
            let path = change
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let new_content = change
                .get("new_content")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Read old file content for diff comparison.
            let old_content = if std::path::Path::new(path).exists() {
                std::fs::read_to_string(path).unwrap_or_default()
            } else {
                String::new()
            };

            out.push_str(&mae_core::diff::unified_diff_string(
                &old_content,
                new_content,
                path,
                path,
                3,
            ));
        }
    }
    out
}

/// Check if a deferred LSP tool call has timed out (15s) and send an error
/// result back to the AI session if so.
pub fn timeout_deferred_reply(editor: &mut Editor, deferred_ai_reply: &mut DeferredAiReply) {
    if let Some((kind, ref tool_call_id, _, created_at)) = *deferred_ai_reply {
        if created_at.elapsed() > std::time::Duration::from_secs(15) {
            let tid = tool_call_id.clone();
            warn!(?kind, tool_call_id = %tid, "deferred LSP tool call timed out after 15s");
            let result = ToolResult {
                tool_call_id: tid,
                tool_name: match kind {
                    DeferredKind::LspDefinition => "lsp_definition",
                    DeferredKind::LspReferences => "lsp_references",
                    DeferredKind::LspHover => "lsp_hover",
                    DeferredKind::LspWorkspaceSymbol => "lsp_workspace_symbol",
                    DeferredKind::LspDocumentSymbols => "lsp_document_symbols",
                    DeferredKind::DapStart => "dap_start",
                    DeferredKind::DapContinue => "dap_continue",
                    DeferredKind::DapStep => "dap_step",
                }
                .into(),
                success: false,
                output: format!(
                    "LSP request timed out after 15 seconds ({:?}) — server may not be running",
                    kind
                ),
            };
            let (_, _, reply, _) = deferred_ai_reply.take().unwrap();
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_tool_result(result.success, &result.output, None);
            }
            if reply.send(result).is_err() {
                warn!("deferred tool result channel closed after timeout");
            }
        }
    }
}

/// Parse a wire-format permission-tier string (`ToolInfo::permission`'s own
/// `format!("{p:?}")` convention: `"ReadOnly"`/`"Write"`/`"Shell"`/
/// `"Privileged"`) into a `PermissionTier`. `None` for anything else --
/// callers must treat that as "no override," never as an implicit tier.
/// Mirrors `crates/agent-cli/src/main.rs`'s identically-shaped helper (that
/// crate can't depend on this one, so it's kept small and duplicated rather
/// than factored into a shared crate for one four-arm match).
fn parse_permission_tier(s: &str) -> Option<PermissionTier> {
    match s {
        "ReadOnly" => Some(PermissionTier::ReadOnly),
        "Write" => Some(PermissionTier::Write),
        "Shell" => Some(PermissionTier::Shell),
        "Privileged" => Some(PermissionTier::Privileged),
        _ => None,
    }
}

/// How the external-MCP dispatch path identifies itself when it maps
/// ADR-090's `Ask` to a denial.
///
/// @ai-caution: [security] This path is **non-interactive by construction**
/// (ADR-090 D3). An MCP tool request arrives on a reply channel the client is
/// blocking on, from a client MAE cannot prompt — MAE implements no MCP
/// elicitation, and the local human is not the principal making the request.
/// So `Ask` becomes a denial that names the ceiling, never an allow. Wiring a
/// real prompt here means parking the MCP reply (the `deferred_mcp_reply`
/// machinery already supports parking) and resolving it from the event loop;
/// until that exists, do not "temporarily" widen the policy here instead.
const MCP_SURFACE: &str = "external MCP dispatch";

/// The embedded path's `Ask` is implemented in `AgentSession::decide_and_present`
/// (an `AiEvent::ConfirmToolCall` the human answers with `:ai-accept` /
/// `:ai-reject`), *before* dispatch. Dispatch answering `Ask` therefore means
/// the two saw different policies — a race, not a normal outcome — and the
/// main thread cannot block on a prompt, so it denies.
const EMBEDDED_RACE_SURFACE: &str =
    "the embedded dispatch path (the policy changed after the call passed its prompt)";

/// The ambient Scheme tier for evaluation driven by a *human* keypress.
///
/// @ai-caution: [permission] The human is not the principal ADR-084's tiers
/// bound — they already have a shell, and `:` + `(shell-command …)` was never
/// gated. `Privileged` here is not a bypass; it is the statement that a
/// keystroke-driven eval carries the user's own authority. Only the AI/MCP
/// drains lower it, and they lower it from their own resolved policy.
pub const HUMAN_AMBIENT_TIER: PermissionTier = PermissionTier::Privileged;

/// Compute a session's effective permission policy (ADR-051 tier +
/// ADR-056 category): the minimum tier and the intersected category set of
/// the server's global policy and the session's own self-declared values
/// (`initialize`'s `permissionCeiling`/`toolCategoryAllowlist` params,
/// threaded via `RequesterContext`), if any. A self-declared value can only
/// ever TIGHTEN the effective policy on either axis -- an unrecognized tier
/// value, or a category list intersecting to the empty set only because the
/// session declared something the global policy doesn't already allow, is
/// never treated as an escalation request. `allowed_categories` composition:
/// `None ∩ X = X`, `Some(a) ∩ Some(b) = Some(a ∩ b)` -- a session can only
/// narrow an already-unrestricted global policy, or further narrow an
/// already-restricted one, never widen it.
fn effective_permission_policy(
    global: &PermissionPolicy,
    declared_ceiling: Option<&str>,
    declared_categories: Option<&str>,
) -> PermissionPolicy {
    // @ai-caution: [security] Distinguish "declared nothing" from "declared
    // something unparseable" (ADR-084 D4). Both used to fall through to the
    // global policy, so a session that *meant* to restrict itself but sent a
    // value this build doesn't recognise got no tightening at all — the typo
    // silently removed the restriction. An unparseable declaration now resolves
    // to the most restrictive value on its axis. It still cannot escalate: the
    // tier axis is a `min` against the global, and the category axis only ever
    // narrows.
    //
    // ADR-090 D2: a session-declared ceiling is a HARD ceiling, not an
    // auto-approval ceiling. Exceeding the global auto-approval ceiling is
    // askable; exceeding what the session asked to be limited to is not —
    // prompting a human to undo the session's own declaration would make the
    // declaration meaningless, and the same goes for a declaration that failed
    // to parse.
    let hard = match declared_ceiling {
        None => None,
        Some(raw) => match parse_permission_tier(raw) {
            Some(declared) => Some(mae_ai::HardCeiling {
                tier: declared,
                source: mae_ai::HardCeilingSource::SessionDeclared,
            }),
            None => {
                warn!(
                    declared = %raw,
                    "unparseable session permission ceiling — falling back to the most \
                     restrictive tier, not to the global policy"
                );
                Some(mae_ai::HardCeiling {
                    tier: PermissionTier::ReadOnly,
                    source: mae_ai::HardCeilingSource::UnparseableDeclaration,
                })
            }
        },
    };
    let allowed_categories = match declared_categories {
        None => global.allowed_categories.clone(),
        Some(raw) => {
            let declared: std::collections::HashSet<_> =
                mae_ai::parse_categories(raw).into_iter().collect();
            if declared.is_empty() {
                warn!(
                    declared = %raw,
                    "session tool-category allowlist parsed to nothing — denying all \
                     categories rather than falling back to unrestricted"
                );
            }
            match &global.allowed_categories {
                Some(global_set) => Some(global_set.intersection(&declared).copied().collect()),
                None => Some(declared),
            }
        }
    };
    let base = PermissionPolicy {
        auto_approve_up_to: global.auto_approve_up_to,
        hard_ceiling: global.hard_ceiling,
        allowed_categories,
    };
    match hard {
        // `with_hard_ceiling` only ever lowers, on both axes, so a session
        // declaring a ceiling ABOVE the global one changes nothing — the
        // never-escalate property this function has always had.
        Some(hc) => base.with_hard_ceiling(hc),
        None => base,
    }
}

/// Handle an MCP tool request from an external agent.
///
/// Immediate tools resolve and reply synchronously. Deferred tools (LSP-dependent)
/// store the reply channel in `deferred_mcp_reply` and drain the queued LSP intent
/// so the language server receives it immediately. The result is sent later when
/// `try_resolve_deferred_mcp` matches the incoming LSP event.
/// Returns `true` if the tool resolved immediately (no deferred LSP wait).
/// The caller should clear the MCP input lock when this returns `true` and
/// `deferred_mcp_reply` is empty.
pub fn handle_mcp_request(
    editor: &mut Editor,
    mcp_req: mae_mcp::McpToolRequest,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    deferred_mcp_reply: &mut DeferredMcpReply,
    scheme: &mut mae_scheme::SchemeRuntime,
) -> bool {
    debug!(tool = %mcp_req.tool_name, "MCP tool call");
    let session_id = mcp_req.requester.session_id;
    // ADR-051: this session's own effective policy -- never looser than
    // `permission_policy` (the server's global default), possibly tighter
    // if the session declared its own ceiling at `initialize`.
    let effective_policy = effective_permission_policy(
        permission_policy,
        mcp_req.requester.declared_permission_ceiling.as_deref(),
        mcp_req.requester.declared_tool_categories.as_deref(),
    );
    let fake_call = mae_ai::ToolCall {
        id: "mcp".to_string(),
        name: mcp_req.tool_name.clone(),
        arguments: mcp_req.arguments,
    };
    // ADR-048: an external MCP client's declared provider is only ever present
    // on `requester` when the session actually completed the PSK handshake
    // (`shared/mcp`'s invariant) — this is a second, defense-in-depth check of
    // that same invariant, not the only enforcement of it.
    let requester_provider = if mcp_req.requester.psk_authenticated {
        mcp_req.requester.declared_provider.as_deref()
    } else {
        None
    };
    let exec_result = match crate::ai_residency::check_kb_residency(
        editor,
        &fake_call.name,
        &fake_call.arguments,
        requester_provider,
    ) {
        crate::ai_residency::ResidencyDecision::Deny(reason) => {
            ExecuteResult::Immediate(ToolResult {
                tool_call_id: fake_call.id.clone(),
                tool_name: fake_call.name.clone(),
                success: false,
                output: reason,
            })
        }
        crate::ai_residency::ResidencyDecision::Allow => {
            // #363: `execute_command` normally routes into `crates/ai`'s
            // `execute_tool_with_requester` (builtins-only — `crates/ai` has
            // no `SchemeRuntime` in scope, so it structurally can never
            // dispatch a Scheme-defined command). This handler already has
            // both `editor` and `scheme`, same as the `eval_scheme` drain
            // just below — bridge the gap here for Scheme-sourced commands,
            // mirroring `dispatch_command_by_name`'s (`state_sync_apply.rs`)
            // fix for the `(run-command ...)` path, and fall through to the
            // existing builtins-only dispatch unchanged for everything else.
            let scheme_command = (fake_call.name == "execute_command")
                .then(|| fake_call.arguments.get("command").and_then(|v| v.as_str()))
                .flatten()
                .filter(|cmd| {
                    matches!(
                        editor.commands.get(cmd).map(|c| &c.source),
                        Some(mae_core::CommandSource::Scheme(_))
                    )
                })
                .map(str::to_string);
            match scheme_command {
                Some(cmd) => {
                    // SECURITY (found via adversarial review): this bridge
                    // used to dispatch straight through with NO permission
                    // check at all -- it never reaches
                    // `execute_tool_dispatch_body`'s `policy.is_allowed(...)`
                    // gate (line ~98), which is the ONLY enforcement point in
                    // the builtins-only path below. A session that declared
                    // a ReadOnly ceiling at `initialize` (ADR-051's own
                    // headline feature) could call `execute_command` naming
                    // any Scheme-sourced command -- a large fraction of
                    // feature-module commands (git, kb-sharing, collab,
                    // babel, etc. per `crates/core/src/commands.rs`) are
                    // Scheme-sourced -- and it would execute with full
                    // effect regardless of the declared/global policy.
                    // Enforce the SAME blanket bar the generic
                    // `execute_command` tool itself carries for the Rust-
                    // builtin path (`execute_command_dispatch`,
                    // `crates/ai/src/executor/core_exec.rs`, dispatches via
                    // `editor.dispatch_builtin` with no further per-command
                    // check beyond that tool's own registered `Write` tier)
                    // -- this bridge must never be a strictly weaker path
                    // than the one it's standing in for.
                    //
                    // ADR-056 correction found while writing this bridge's
                    // own adversarial test: `execute_tool_dispatch_body`'s
                    // new category check (step 2b) does NOT cover this
                    // branch either -- `scheme_command` is matched BEFORE
                    // ever falling through to `execute_tool_with_requester`
                    // below, so this is a second, independent chokepoint,
                    // exactly like the tier check already is. Check it here
                    // too, same fail-closed semantics (`execute_command` is
                    // itself uncategorized).
                    //
                    // ADR-090: this bridge asks the same PDP, and gets the
                    // same three answers. It is a non-interactive surface
                    // (see the `Ask` arm below), so it maps `Ask` to a denial
                    // explicitly rather than treating it as an allow.
                    let bridge_decision =
                        effective_policy.decide(&fake_call.name, PermissionTier::Write);
                    if let mae_ai::Decision::Deny(reason) = bridge_decision {
                        ExecuteResult::Immediate(ToolResult {
                            tool_call_id: fake_call.id.clone(),
                            tool_name: fake_call.name.clone(),
                            success: false,
                            output: mae_ai::deny_message(
                                &fake_call.name,
                                PermissionTier::Write,
                                reason,
                            ),
                        })
                    } else if bridge_decision.is_ask() {
                        ExecuteResult::Immediate(ToolResult {
                            tool_call_id: fake_call.id.clone(),
                            tool_name: fake_call.name.clone(),
                            success: false,
                            output: mae_ai::ask_denied_message(
                                &fake_call.name,
                                PermissionTier::Write,
                                effective_policy.auto_approve_up_to,
                                MCP_SURFACE,
                            ),
                        })
                    } else {
                        // Issue #372: this is the one MCP-originated mutation
                        // path that bypasses `execute_tool_with_requester`
                        // (and thus its `with_ai_dispatch_scope` wrap)
                        // entirely, since `crates/ai` has no `SchemeRuntime`
                        // in scope and can't dispatch a Scheme-defined
                        // command itself. Wrap this call site the same way
                        // so it gets the same companion-window guarantee —
                        // do NOT wrap `dispatch_command_by_name` itself,
                        // since it's also called from
                        // `state_sync_apply.rs`'s `(run-command ...)` drain
                        // loop for ordinary human-triggered Scheme
                        // automation, where running in the human's own
                        // focused window is correct. ADR-051: session-scoped
                        // the same way execute_tool_with_requester below is,
                        // so this bridge doesn't reintroduce the
                        // cross-session window-sharing gap for exactly the
                        // one dispatch path that doesn't go through that
                        // function.
                        editor.with_ai_dispatch_scope_for_session(Some(session_id), |editor| {
                            scheme.dispatch_command_by_name(editor, &cmd)
                        });
                        // Matches execute_command_dispatch's existing
                        // response shape exactly
                        // (`crates/ai/src/executor/core_exec.rs`) — this
                        // bridge is a dispatch-mechanism swap, not a
                        // response-contract change.
                        ExecuteResult::Immediate(ToolResult {
                            tool_call_id: fake_call.id.clone(),
                            tool_name: fake_call.name.clone(),
                            success: true,
                            output: format!("Executed: {}", cmd),
                        })
                    }
                }
                None => execute_tool_with_requester(
                    editor,
                    &fake_call,
                    all_tools,
                    &effective_policy,
                    requester_provider,
                    Some(session_id),
                ),
            }
        }
    };
    // Drain any pending Scheme evals queued by the tool (e.g. eval_scheme).
    let scheme_output =
        drain_pending_scheme_evals(editor, scheme, effective_policy.ambient_scheme_tier());
    match exec_result {
        ExecuteResult::Immediate(mut result) => {
            // If the tool queued a Scheme eval, replace the output with the
            // result. ADR-086/#590.2: an errored eval must report failure,
            // not clobber it with success:true.
            if let Some((output, all_ok)) = scheme_output {
                result.output = output;
                result.success = all_ok;
            }
            let _ = mcp_req.reply.send(mae_mcp::McpToolResult {
                success: result.success,
                output: result.output,
            });
            true
        }
        // ADR-090 D3: the explicit non-interactive mapping for this surface.
        ExecuteResult::NeedsApproval(req) => {
            let result = req.into_denied(MCP_SURFACE);
            warn!(tool = %result.tool_name, "MCP tool call denied — above the auto-approval ceiling and this surface cannot ask");
            let _ = mcp_req.reply.send(mae_mcp::McpToolResult {
                success: false,
                output: result.output,
            });
            true
        }
        ExecuteResult::Deferred { kind, .. } => {
            info!(
                ?kind,
                pending = deferred_mcp_reply.len(),
                "deferred MCP tool — awaiting LSP response"
            );
            crate::lsp_bridge::drain_lsp_intents(editor, lsp_command_tx);
            deferred_mcp_reply.push((kind, mcp_req.reply, tokio::time::Instant::now()));
            false
        }
    }
}

/// Check if any deferred MCP tool calls have timed out (15s) and send error
/// results back to the MCP client.
pub fn timeout_deferred_mcp_reply(editor: &mut Editor, deferred_mcp_reply: &mut DeferredMcpReply) {
    let timeout = std::time::Duration::from_secs(15);
    let mut i = 0;
    while i < deferred_mcp_reply.len() {
        if deferred_mcp_reply[i].2.elapsed() > timeout {
            let (kind, reply, _) = deferred_mcp_reply.swap_remove(i);
            warn!(?kind, "deferred MCP tool call timed out after 15s");
            editor.set_status("MCP tool timed out (15s)");
            let _ = reply.send(mae_mcp::McpToolResult {
                success: false,
                output: format!(
                    "LSP request timed out after 15 seconds ({:?}) — server may not be running",
                    kind
                ),
            });
            // Don't increment i — swap_remove moved the last element here.
        } else {
            i += 1;
        }
    }
}

/// Check if an incoming LSP event completes a deferred AI tool call, and send
/// the result back if so. Returns true if a deferred call was completed.
pub fn try_resolve_deferred(
    editor: &mut Editor,
    lsp_event: &mae_lsp::LspTaskEvent,
    deferred_ai_reply: &mut DeferredAiReply,
) -> bool {
    if let Some((kind, ref tool_call_id, _, _)) = *deferred_ai_reply {
        if let Some(result) =
            crate::lsp_bridge::try_complete_deferred(lsp_event, kind, tool_call_id)
        {
            let (_, _, reply, _) = deferred_ai_reply.take().unwrap();
            debug!(tool_call_id = %result.tool_call_id, "deferred tool call completed");
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_tool_result(result.success, &result.output, None);
            }
            if reply.send(result).is_err() {
                warn!("deferred tool result channel closed");
            }
            return true;
        }
    }
    false
}

/// Check if an incoming LSP event completes any deferred MCP tool call, and send
/// the result back to the MCP client if so. Returns true if any was resolved.
pub fn try_resolve_deferred_mcp(
    lsp_event: &mae_lsp::LspTaskEvent,
    deferred_mcp_reply: &mut DeferredMcpReply,
) -> bool {
    let mut resolved = false;
    let mut i = 0;
    while i < deferred_mcp_reply.len() {
        let kind = deferred_mcp_reply[i].0;
        if let Some(result) = crate::lsp_bridge::try_complete_deferred(lsp_event, kind, "mcp") {
            let (_, reply, _) = deferred_mcp_reply.swap_remove(i);
            debug!(?kind, "deferred MCP tool call completed");
            let _ = reply.send(mae_mcp::McpToolResult {
                success: result.success,
                output: result.output,
            });
            resolved = true;
            // Don't increment — swap_remove moved last element here.
            // Also break: one LSP event typically resolves one deferred call.
            break;
        } else {
            i += 1;
        }
    }
    resolved
}

/// Result of trying to resolve a deferred DAP call.
#[derive(Debug, PartialEq, Eq)]
pub enum DapResolveAction {
    /// No deferred call pending or event didn't match.
    None,
    /// Transitioned from WaitingForEvent → WaitingForStackTrace.
    /// Caller should drain DAP intents so RefreshThreadsAndStack is sent.
    TransitionedToStackTrace,
    /// Fully resolved — result sent back to AI session.
    Resolved,
}

/// Check if an incoming DAP event advances or completes a deferred DAP tool call.
///
/// DAP has a multi-stage event cascade:
/// - `dap_start` (stop_on_entry=false): WaitingForEvent → SessionStarted → resolve
/// - `dap_start` (stop_on_entry=true): WaitingForEvent → SessionStarted → WaitingForStop → Stopped → WaitingForStackTrace → StackTraceResult → resolve
/// - `dap_continue/step`: WaitingForEvent → Stopped → WaitingForStackTrace → StackTraceResult → resolve
/// - Any: WaitingForEvent → Terminated → resolve
///
/// Call this BEFORE `handle_dap_event` so the phase transition happens before
/// the event loop processes the event (which queues RefreshThreadsAndStack).
pub fn try_resolve_deferred_dap(
    editor: &mut Editor,
    dap_event: &mae_dap::DapTaskEvent,
    deferred_dap_reply: &mut DeferredDapReply,
) -> DapResolveAction {
    let state = match deferred_dap_reply.as_ref() {
        Some(s) => s,
        None => return DapResolveAction::None,
    };

    debug!(
        kind = ?state.kind,
        phase = ?state.phase,
        event = ?dap_event_name(dap_event),
        "try_resolve_deferred_dap: checking event against deferred"
    );

    match (state.kind, state.phase, dap_event) {
        // === DapStart (stop_on_entry=true): Phase 1 — SessionStarted → WaitingForStop ===
        (
            DeferredKind::DapStart,
            DapDeferredPhase::WaitingForEvent,
            mae_dap::DapTaskEvent::SessionStarted { .. },
        ) if state.stop_on_entry => {
            if let Some(s) = deferred_dap_reply.as_mut() {
                s.phase = DapDeferredPhase::WaitingForStop;
            }
            DapResolveAction::None
        }

        // === DapStart (stop_on_entry=true): Phase 2 — Stopped → WaitingForStackTrace ===
        (
            DeferredKind::DapStart,
            DapDeferredPhase::WaitingForStop,
            mae_dap::DapTaskEvent::Stopped { .. },
        ) => {
            if let Some(s) = deferred_dap_reply.as_mut() {
                s.phase = DapDeferredPhase::WaitingForStackTrace;
            }
            DapResolveAction::TransitionedToStackTrace
        }

        // === DapStart (stop_on_entry=true): Phase 3 — StackTraceResult → Resolved ===
        (
            DeferredKind::DapStart,
            DapDeferredPhase::WaitingForStackTrace,
            mae_dap::DapTaskEvent::StackTraceResult { .. },
        ) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = build_dap_stopped_response(editor, dap_event);
            resolve_dap_deferred(editor, deferred_dap_reply, true, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        // === DapStart (stop_on_entry=false): SessionStarted → Resolved immediately ===
        (
            DeferredKind::DapStart,
            DapDeferredPhase::WaitingForEvent,
            mae_dap::DapTaskEvent::SessionStarted { adapter_id, .. },
        ) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = serde_json::json!({
                "status": "session_started",
                "adapter": adapter_id,
            })
            .to_string();
            resolve_dap_deferred(editor, deferred_dap_reply, true, &output, &tool_call_id);
            DapResolveAction::Resolved
        }
        (DeferredKind::DapStart, _, mae_dap::DapTaskEvent::SessionStartFailed { error }) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = format!("Debug session failed to start: {}", error);
            resolve_dap_deferred(editor, deferred_dap_reply, false, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        // === DapContinue / DapStep: Phase 1 — Stopped event ===
        (
            DeferredKind::DapContinue | DeferredKind::DapStep,
            DapDeferredPhase::WaitingForEvent,
            mae_dap::DapTaskEvent::Stopped { .. },
        ) => {
            // Transition to phase 2: wait for StackTraceResult after the refresh cascade
            if let Some(s) = deferred_dap_reply.as_mut() {
                s.phase = DapDeferredPhase::WaitingForStackTrace;
            }
            DapResolveAction::TransitionedToStackTrace
        }

        // === DapContinue / DapStep: Phase 2 — StackTraceResult ===
        (
            DeferredKind::DapContinue | DeferredKind::DapStep,
            DapDeferredPhase::WaitingForStackTrace,
            mae_dap::DapTaskEvent::StackTraceResult { .. },
        ) => {
            let tool_call_id = state.tool_call_id.clone();
            // Build rich response from editor.dap.state (already updated by handle_dap_event
            // for the Stopped event; StackTraceResult will be applied after this returns)
            let output = build_dap_stopped_response(editor, dap_event);
            resolve_dap_deferred(editor, deferred_dap_reply, true, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        // === Terminated — resolves any pending DAP deferred ===
        (_, _, mae_dap::DapTaskEvent::Terminated) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = serde_json::json!({"status": "terminated"}).to_string();
            resolve_dap_deferred(editor, deferred_dap_reply, true, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        // === Error — resolves any pending DAP deferred ===
        (_, _, mae_dap::DapTaskEvent::Error { message }) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = format!("DAP error: {}", message);
            resolve_dap_deferred(editor, deferred_dap_reply, false, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        // === AdapterExited — resolves any pending DAP deferred ===
        (_, _, mae_dap::DapTaskEvent::AdapterExited) => {
            let tool_call_id = state.tool_call_id.clone();
            let output = "Debug adapter process exited".to_string();
            resolve_dap_deferred(editor, deferred_dap_reply, false, &output, &tool_call_id);
            DapResolveAction::Resolved
        }

        _ => DapResolveAction::None,
    }
}

/// Send the deferred DAP result back to the AI session and update conversation.
fn resolve_dap_deferred(
    editor: &mut Editor,
    deferred_dap_reply: &mut DeferredDapReply,
    success: bool,
    output: &str,
    tool_call_id: &str,
) {
    let state = deferred_dap_reply.take().unwrap();
    let result = ToolResult {
        tool_call_id: tool_call_id.to_string(),
        tool_name: state.kind.tool_name().into(),
        success,
        output: output.to_string(),
    };
    debug!(tool_call_id, success, "deferred DAP tool call completed");
    if let Some(conv) = find_conversation_buffer_mut(editor) {
        conv.complete_last_tool_call(result.success, &result.output, None);
    }
    if state.reply.send(result).is_err() {
        warn!("deferred DAP tool result channel closed");
    }
}

/// Build a rich JSON response from the current debug state after a Stopped + StackTraceResult.
fn build_dap_stopped_response(editor: &Editor, dap_event: &mae_dap::DapTaskEvent) -> String {
    // Extract thread_id and frames from the StackTraceResult event
    let (thread_id, frames) = match dap_event {
        mae_dap::DapTaskEvent::StackTraceResult { thread_id, frames } => (*thread_id, frames),
        _ => return serde_json::json!({"status": "stopped"}).to_string(),
    };

    // Get stop reason from debug_state (already updated by apply_dap_stopped)
    let reason = editor
        .dap
        .state
        .as_ref()
        .and_then(|ds| ds.last_stop_reason.as_deref())
        .unwrap_or("unknown");

    // Top frame from the event data
    let top_frame = frames.first().map(|f| {
        let src = f
            .source
            .as_ref()
            .and_then(|s| s.path.as_deref().or(s.name.as_deref()));
        serde_json::json!({
            "id": f.id,
            "name": &f.name,
            "source": src,
            "line": f.line,
            "column": f.column,
        })
    });

    // Breakpoint count
    let bp_count = editor
        .dap
        .state
        .as_ref()
        .map(|ds| ds.breakpoints.values().map(|v| v.len()).sum::<usize>())
        .unwrap_or(0);

    serde_json::json!({
        "status": "stopped",
        "reason": reason,
        "thread_id": thread_id,
        "frame": top_frame,
        "total_frames": frames.len(),
        "breakpoints_set": bp_count,
    })
    .to_string()
}

/// Check if a deferred DAP tool call has timed out (15s).
/// Short name for a DAP event — used only for tracing.
fn dap_event_name(event: &mae_dap::DapTaskEvent) -> &'static str {
    match event {
        mae_dap::DapTaskEvent::SessionStarted { .. } => "SessionStarted",
        mae_dap::DapTaskEvent::SessionStartFailed { .. } => "SessionStartFailed",
        mae_dap::DapTaskEvent::Stopped { .. } => "Stopped",
        mae_dap::DapTaskEvent::Continued { .. } => "Continued",
        mae_dap::DapTaskEvent::ThreadEvent { .. } => "ThreadEvent",
        mae_dap::DapTaskEvent::Output { .. } => "Output",
        mae_dap::DapTaskEvent::Terminated => "Terminated",
        mae_dap::DapTaskEvent::AdapterExited => "AdapterExited",
        mae_dap::DapTaskEvent::Error { .. } => "Error",
        mae_dap::DapTaskEvent::ThreadsResult { .. } => "ThreadsResult",
        mae_dap::DapTaskEvent::StackTraceResult { .. } => "StackTraceResult",
        mae_dap::DapTaskEvent::ScopesResult { .. } => "ScopesResult",
        mae_dap::DapTaskEvent::VariablesResult { .. } => "VariablesResult",
        mae_dap::DapTaskEvent::BreakpointsSet { .. } => "BreakpointsSet",
        mae_dap::DapTaskEvent::EvaluateResult { .. } => "EvaluateResult",
    }
}

pub fn timeout_deferred_dap_reply(editor: &mut Editor, deferred_dap_reply: &mut DeferredDapReply) {
    if let Some(ref state) = *deferred_dap_reply {
        if state.created_at.elapsed() > std::time::Duration::from_secs(15) {
            let tool_call_id = state.tool_call_id.clone();
            let kind = state.kind;
            let phase = state.phase;
            warn!(?kind, ?phase, %tool_call_id, "deferred DAP tool call timed out after 15s");

            // Build diagnostic info from current debug state.
            let diag = if let Some(ds) = editor.dap.state.as_ref() {
                let thread_info = if ds.threads.is_empty() {
                    "no threads known".to_string()
                } else {
                    ds.threads
                        .iter()
                        .map(|t| {
                            format!(
                                "{}({})",
                                t.name,
                                if t.stopped { "stopped" } else { "running" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let bp_info = ds
                    .breakpoints
                    .iter()
                    .map(|(src, bps)| {
                        let lines: Vec<_> = bps
                            .iter()
                            .map(|b| {
                                format!(
                                    "{}:{}{}",
                                    src,
                                    b.line,
                                    if b.verified { "" } else { " (unverified)" }
                                )
                            })
                            .collect();
                        lines.join(", ")
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(
                    "Threads: [{}]. Breakpoints: [{}]. Active thread: {}",
                    thread_info,
                    if bp_info.is_empty() { "none" } else { &bp_info },
                    ds.active_thread_id,
                )
            } else {
                "No debug state (session may have ended)".to_string()
            };

            // Pull recent warn/error messages so the agent sees root cause inline.
            let recent_warnings: Vec<String> = editor
                .message_log
                .entries_filtered(mae_core::MessageLevel::Warn)
                .iter()
                .rev()
                .take(10)
                .map(|e| format!("[{}] {}: {}", e.level, e.target, e.message))
                .collect();
            let recent_section = if recent_warnings.is_empty() {
                String::new()
            } else {
                format!(" Recent warnings/errors: {}", recent_warnings.join(" | "))
            };

            let output = format!(
                "DAP operation timed out after 15s ({:?}, phase: {:?}). \
                 Diagnostic: {}.{} \
                 Check MAE logs (MAE_LOG=mae_dap=debug) for adapter events.",
                kind, phase, diag, recent_section
            );
            resolve_dap_deferred(editor, deferred_dap_reply, false, &output, &tool_call_id);
        }
    }
}

/// Drain any pending Scheme evaluations queued by AI tools (e.g. `eval_scheme`).
/// Returns `Some(output)` if any expressions were evaluated, `None` otherwise.
///
/// Uses `eval_yielding` to handle yield primitives inline:
/// - `yield-tick`: drains hooks and side effects, then resumes
/// - `await-hook`: drains hooks each tick until the target fires or timeout
/// - `flush!`: same as tick (apply + inject)
///
/// Returns `(joined transcript, whether every eval succeeded)`. The bool is
/// the ADR-086 signal: callers must only force `success = true` on the tool
/// result when it is `true`. It comes from `eval_with_yield_handling`'s own
/// `Result`, never from sniffing the formatted string for an "error" prefix
/// (that prose is for the human-facing REPL transcript, not control flow).
///
/// `ambient_tier` is ADR-084 D2/D7's missing wire: it is passed straight to
/// [`mae_scheme::SchemeRuntime::with_ambient_tier`], which is what makes
/// D3's per-primitive tier declarations do anything at all. Without a caller
/// lowering it, the VM's ambient tier stays at its `Privileged` default and
/// every classified primitive passes its check trivially.
///
/// @ai-caution: [permission] `ambient_tier` MUST come from the caller's
/// resolved `PermissionPolicy` (`PermissionPolicy::ambient_scheme_tier`), or
/// from [`HUMAN_AMBIENT_TIER`] on a keypress-driven path — never from
/// anything the evaluated program or the tool arguments supplied. A
/// caller-derived tier turns the check into the confused deputy it exists to
/// prevent.
pub fn drain_pending_scheme_evals(
    editor: &mut Editor,
    scheme: &mut mae_scheme::SchemeRuntime,
    ambient_tier: PermissionTier,
) -> Option<(String, bool)> {
    if editor.pending_scheme_eval.is_empty() {
        return None;
    }
    let exprs: Vec<String> = std::mem::take(&mut editor.pending_scheme_eval);
    let mut results = Vec::new();
    let mut all_ok = true;
    for code in &exprs {
        scheme.inject_editor_state(editor);
        let (ok, output) = scheme.with_ambient_tier(ambient_tier, |scheme| {
            eval_with_yield_handling(editor, scheme, code)
        });
        all_ok = all_ok && ok;
        let formatted = format!("> {}\n{}\n", code.trim(), output);
        editor.append_to_scheme_repl(&formatted);
        results.push(formatted);
    }
    Some((results.join("\n"), all_ok))
}

/// Evaluate scheme code with inline yield handling for synchronous contexts
/// (MCP, AI tools). Handles yield-tick and await-hook by draining hooks
/// and side effects without returning to the event loop.
///
/// Returns `(succeeded, formatted transcript line)`. `succeeded` is `false`
/// for every path that previously formatted an `"; error: ..."` string —
/// per ADR-086/audit #590.2, a tool-result caller must use this bool to
/// decide `success`, not re-parse the formatted text for the word "error".
fn eval_with_yield_handling(
    editor: &mut Editor,
    scheme: &mut mae_scheme::SchemeRuntime,
    code: &str,
) -> (bool, String) {
    use mae_scheme::vm::YieldRequest;
    use mae_scheme::SchemeEvalResult;

    let mut eval_result = match scheme.eval_yielding(code) {
        Ok(r) => r,
        Err(e) => return (false, format!("; error: {}", e.message)),
    };

    loop {
        match eval_result {
            SchemeEvalResult::Done(s) => {
                scheme.apply_to_editor(editor);
                return (
                    true,
                    if s.is_empty() {
                        "; => (void)".to_string()
                    } else {
                        format!("; => {}", s)
                    },
                );
            }
            SchemeEvalResult::Yield(ref req) => {
                match req {
                    YieldRequest::Tick | YieldRequest::Flush => {
                        scheme.apply_to_editor(editor);
                        crate::key_handling::drain_hook_evals(editor, scheme);
                        scheme.inject_editor_state(editor);
                    }
                    YieldRequest::AwaitHook(hook_name, _timeout) => {
                        // In synchronous context (MCP), check if the hook
                        // is already pending. If not, it won't fire without
                        // external events, so resume immediately with #f.
                        let hook_name = hook_name.clone();
                        scheme.apply_to_editor(editor);
                        let fired = editor
                            .pending_hook_evals
                            .iter()
                            .any(|(h, _)| h == &hook_name);
                        crate::key_handling::drain_hook_evals(editor, scheme);
                        scheme.inject_editor_state(editor);
                        eval_result =
                            match scheme.resume_yield(mae_scheme::value::Value::Bool(fired)) {
                                Ok(r) => r,
                                Err(e) => return (false, format!("; error: {}", e.message)),
                            };
                        continue;
                    }
                    YieldRequest::Sleep(d) => {
                        std::thread::sleep(*d);
                    }
                    YieldRequest::WaitForFile(path, timeout) => {
                        let deadline = std::time::Instant::now() + *timeout;
                        while !path.exists() && std::time::Instant::now() < deadline {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                    YieldRequest::Breakpoint(_) => {
                        // Can't pause in MCP context — skip.
                    }
                }
                eval_result = match scheme.resume_yield(mae_scheme::value::Value::Bool(true)) {
                    Ok(r) => r,
                    Err(e) => return (false, format!("; error: {}", e.message)),
                };
            }
        }
    }
}

#[cfg(test)]
#[path = "ai_event_handler_tests.rs"]
mod ai_event_handler_tests;
