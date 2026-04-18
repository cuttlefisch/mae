//! Shared AI event handling for terminal and GUI loops.
//!
//! Both event loops need identical logic for dispatching AI events
//! (tool calls, text responses, streaming, cost updates, budget warnings).
//! This module provides a single implementation to avoid the duplication
//! that historically plagues editor event loops (see: Emacs xdisp.c).

use mae_ai::{execute_tool, AiEvent, DeferredKind, ExecuteResult, ToolResult};
use mae_core::{Editor, InputLock};
use mae_lsp::LspCommand;
use tracing::{debug, error, info, warn};

use crate::bootstrap::find_conversation_buffer_mut;

/// Type alias for the deferred AI reply state held across loop iterations.
pub type DeferredAiReply = Option<(
    DeferredKind,
    String, // tool_call_id
    tokio::sync::oneshot::Sender<ToolResult>,
    tokio::time::Instant, // created_at
)>;

/// Deferred MCP reply state — supports multiple concurrent deferred calls.
/// Each entry tracks its `DeferredKind`, reply channel, and creation time.
pub type DeferredMcpReply = Vec<(
    DeferredKind,
    tokio::sync::oneshot::Sender<mae_mcp::McpToolResult>,
    tokio::time::Instant, // created_at
)>;

/// Handle a single AI event. Shared between terminal and GUI loops.
pub fn handle_ai_event(
    editor: &mut Editor,
    ai_event: AiEvent,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    deferred_ai_reply: &mut DeferredAiReply,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
) {
    match ai_event {
        AiEvent::ToolCallRequest { call, reply } => {
            editor.ai_streaming = true;
            info!(tool = %call.name, call_id = %call.id, "executing AI tool call");
            if let Some(conv) = find_conversation_buffer_mut(editor) {
                conv.push_tool_call(&call.name);
            }
            let tool_start = std::time::Instant::now();
            let exec_result = execute_tool(editor, &call, all_tools, permission_policy);
            match exec_result {
                ExecuteResult::Immediate(result) => {
                    let elapsed_ms = tool_start.elapsed().as_millis() as u64;
                    info!(tool = %call.name, success = result.success, elapsed_ms, "AI tool call complete");
                    if let Some(conv) = find_conversation_buffer_mut(editor) {
                        conv.push_tool_result(result.success, &result.output, Some(elapsed_ms));
                    }
                    if reply.send(result).is_err() {
                        warn!(tool = %call.name, "tool result channel closed — AI session may have been cancelled");
                    }
                }
                ExecuteResult::Deferred { tool_call_id, kind } => {
                    info!(tool = %call.name, ?kind, "deferred LSP tool call — waiting for server response");
                    // Drain the LSP intent we just queued so it's sent immediately.
                    crate::drain_lsp_intents(editor, lsp_command_tx);
                    *deferred_ai_reply =
                        Some((kind, tool_call_id, reply, tokio::time::Instant::now()));
                }
            }
        }
        AiEvent::TextResponse(text) => {
            editor.ai_streaming = true;
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
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
        AiEvent::StreamChunk(text) => {
            editor.ai_streaming = true;
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.append_streaming_chunk(&text);
            }
        }
        AiEvent::SessionComplete(_text) => {
            info!("AI session complete");
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.end_streaming();
            }
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            editor.set_status("[AI] Done");
        }
        AiEvent::Error(msg) => {
            error!(error = %msg, "AI error");
            if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                conv_buf.push_system(format!("Error: {}", msg));
                conv_buf.end_streaming();
            }
            editor.ai_streaming = false;
            editor.input_lock = InputLock::None;
            editor.set_status(format!("[AI error] {}", msg));
        }
        AiEvent::CostUpdate {
            session_usd,
            last_call_usd,
            tokens_in,
            tokens_out,
        } => {
            editor.ai_session_cost_usd = session_usd;
            editor.ai_session_tokens_in = tokens_in;
            editor.ai_session_tokens_out = tokens_out;
            debug!(
                session_usd,
                last_call_usd, tokens_in, tokens_out, "AI cost update"
            );
        }
        AiEvent::BudgetWarning {
            session_usd,
            threshold_usd,
        } => {
            let msg = format!(
                "AI budget warning: session spend ${:.4} crossed ${:.2} threshold. \
                 Hard cap (if set) will abort the next turn.",
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
    }
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
pub fn handle_mcp_request(
    editor: &mut Editor,
    mcp_req: mae_mcp::McpToolRequest,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    deferred_mcp_reply: &mut DeferredMcpReply,
) {
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
        }
        ExecuteResult::Deferred { kind, .. } => {
            info!(
                ?kind,
                pending = deferred_mcp_reply.len(),
                "deferred MCP tool — awaiting LSP response"
            );
            crate::drain_lsp_intents(editor, lsp_command_tx);
            deferred_mcp_reply.push((kind, mcp_req.reply, tokio::time::Instant::now()));
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
        if let Some(result) = crate::try_complete_deferred(lsp_event, kind, tool_call_id) {
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
        if let Some(result) = crate::try_complete_deferred(lsp_event, kind, "mcp") {
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
