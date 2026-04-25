//! Shared AI event handling for terminal and GUI loops.
//!
//! Both event loops need identical logic for dispatching AI events
//! (tool calls, text responses, streaming, cost updates, budget warnings).
//! This module provides a single implementation to avoid the duplication
//! that historically plagues editor event loops (see: Emacs xdisp.c).

use mae_ai::{
    execute_tool, AgentSession, AiCommand, AiEvent, DeferredKind, ExecuteResult, ToolResult,
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
            return editor.buffers[idx].conversation.as_mut();
        }
    }
    find_conversation_buffer_mut(editor)
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
}

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
    #[allow(dead_code)]
    pub ai_command_tx: &'a Option<tokio::sync::mpsc::Sender<AiCommand>>,
}

/// Handle a single AI event. Shared between terminal and GUI loops.
pub fn handle_ai_event(editor: &mut Editor, ai_event: AiEvent, ctx: AiEventContext) {
    match ai_event {
        AiEvent::ToolCallRequest { call, reply } => {
            editor.ai_streaming = true;
            info!(tool = %call.name, call_id = %call.id, "executing AI tool call");
            // Update the existing Pending entry (created by ToolCallStarted) to Running,
            // rather than creating a duplicate entry.
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.update_or_push_tool_call(
                    &call.name,
                    mae_core::conversation::ToolCallState::Running,
                );
            }
            let tool_start = std::time::Instant::now();
            let exec_result = execute_tool(editor, &call, ctx.all_tools, ctx.permission_policy);
            match exec_result {
                ExecuteResult::Immediate(result) => {
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
            editor.ai_streaming = true;
            if let Some(conv_buf) =
                find_buffer_by_name_or_default_mut(editor, target_buffer.as_deref())
            {
                conv_buf.push_assistant(&text);
            } else {
                let display = if text.len() > 120 {
                    format!("[AI] {}...", &text[..117])
                } else {
                    format!("[AI] {}", text)
                };
                editor.set_status(display);
            }
        }
        AiEvent::ToolCallStarted { name } => {
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_tool_call_with_state(
                    &name,
                    mae_core::conversation::ToolCallState::Pending,
                );
            }
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
        }
        AiEvent::StreamChunk {
            text,
            target_buffer,
        } => {
            editor.ai_streaming = true;
            if let Some(conv_buf) =
                find_buffer_by_name_or_default_mut(editor, target_buffer.as_deref())
            {
                conv_buf.append_streaming_chunk(&text);
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
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            editor.set_status("[AI] Done");
        }
        AiEvent::CostUpdate {
            session_usd,
            tokens_in,
            tokens_out,
            cache_read_tokens,
            cache_creation_tokens,
            context_window,
            context_used_tokens,
            ..
        } => {
            editor.ai_session_cost_usd = session_usd;
            editor.ai_session_tokens_in = tokens_in;
            editor.ai_session_tokens_out = tokens_out;
            editor.ai_cache_read_tokens = cache_read_tokens;
            editor.ai_cache_creation_tokens = cache_creation_tokens;
            editor.ai_context_window = context_window;
            editor.ai_context_used_tokens = context_used_tokens;
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
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            editor.set_status(msg);
        }
        AiEvent::AskUser { question, reply } => {
            info!(%question, "AI asking user");
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_system(format!("AI Question: {}", question));
                conv.end_streaming();
            }
            editor.set_status(format!("AI: {}", question));
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            *ctx.pending_interactive_event = Some(PendingInteractiveEvent::AskUser(reply));
        }
        AiEvent::ProposeChanges { changes, reply } => {
            let count = if let Some(arr) = changes.as_array() {
                arr.len()
            } else {
                1
            };
            info!(count, "AI proposing changes");

            // Auto-accept mode: skip manual approval
            if editor.ai_mode == "auto-accept" {
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
                    editor.buffers.push(b);
                    editor.buffers.len() - 1
                }
            };
            editor.buffers[buf_idx].replace_contents(&diff_text);
            editor.switch_to_buffer(buf_idx);

            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_system(format!(
                    "AI proposed changes to {} file(s). Review the *AI-Diff* buffer, then use :ai-accept or :ai-reject.",
                    count
                ));
                conv.end_streaming();
            }
            editor.set_status(format!("AI: Proposing changes to {} file(s)", count));
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            *ctx.pending_interactive_event = Some(PendingInteractiveEvent::ProposeChanges(reply));
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
            let mut sub_buf = mae_core::Buffer::new();
            sub_buf.name = target_buf_name.clone();
            sub_buf.conversation = Some(mae_core::conversation::Conversation::new());
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

            let provider: Box<dyn mae_ai::AgentProvider> = match config.provider_type.as_str() {
                "openai" => Box::new(mae_ai::OpenAiProvider::new(config.clone())),
                "gemini" => Box::new(mae_ai::GeminiProvider::new(config.clone())),
                _ => Box::new(mae_ai::ClaudeProvider::new(config.clone())),
            };

            let tools = {
                let mut t = mae_ai::tools_from_registry(&editor.commands);
                t.extend(mae_ai::ai_specific_tools(&editor.option_registry));
                t
            };

            let sub_session = AgentSession::new(
                provider,
                tools,
                build_system_prompt(&profile),
                proxy_tx,
                sub_cmd_rx,
            )
            .with_budget(config.model, config.budget)
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
            editor.ai_current_round = round;
            editor.ai_transaction_start_idx = transaction_start_idx;
        }
        AiEvent::EventMeta {
            session_id,
            agent_name,
        } => {
            debug!(%session_id, %agent_name, "AI event metadata received");
        }
        AiEvent::Error(msg, transcript_path) => {
            error!(error = %msg, "AI error event");
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.push_system(format!("Error: {}", msg));
                if let Some(ref path) = transcript_path {
                    conv_buf.push_system(format!("Transcript saved to: {}", path));
                }
                conv_buf.end_streaming();
            }
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            editor.set_status(format!("AI Error: {}", msg));
        }
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
            let content = change
                .get("new_content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            out.push_str(&format!("--- a/{}\n", path));
            out.push_str(&format!("+++ b/{}\n", path));
            out.push_str("@@ -1,1 +1,1 @@\n");
            // Simplified: just show the new content
            out.push_str(content);
            if !content.ends_with('\n') {
                out.push('\n');
            }
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
) -> bool {
    debug!(tool = %mcp_req.tool_name, "MCP tool call");
    let fake_call = mae_ai::ToolCall {
        id: "mcp".to_string(),
        name: mcp_req.tool_name.clone(),
        arguments: mcp_req.arguments,
    };
    let exec_result = execute_tool(editor, &fake_call, all_tools, permission_policy);
    match exec_result {
        ExecuteResult::Immediate(result) => {
            let _ = mcp_req.reply.send(mae_mcp::McpToolResult {
                success: result.success,
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
            // Build rich response from editor.debug_state (already updated by handle_dap_event
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
        .debug_state
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
        .debug_state
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
            let diag = if let Some(ds) = editor.debug_state.as_ref() {
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
                 Check MAE logs (RUST_LOG=mae_dap=debug) for adapter events.",
                kind, phase, diag, recent_section
            );
            resolve_dap_deferred(editor, deferred_dap_reply, false, &output, &tool_call_id);
        }
    }
}
