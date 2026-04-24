//! Terminal event loop — the main async loop for the TUI backend.

use std::io;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mae_ai::{execute_tool, AiCommand, AiEvent, DeferredKind, ExecuteResult, ToolResult};
use mae_core::{Editor, KeyPress, Mode};
use mae_dap::DapCommand;
use mae_lsp::{LspCommand, LspTaskEvent};
use mae_renderer::{Renderer, TerminalRenderer};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, trace, warn};

use crate::ai_event_handler;
use crate::bootstrap::{debug_dump, find_conversation_buffer_mut, save_history};
use crate::config;
use crate::dap_bridge::{drain_dap_intents, handle_dap_event};
use crate::key_handling::handle_key;
use crate::lsp_bridge::{drain_lsp_intents, handle_lsp_event};
use crate::shell_keys::handle_shell_key;
use crate::shell_lifecycle;

/// Terminal event loop — async, runs inside `rt.block_on()`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_terminal_loop(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    ai_event_rx: &mut tokio::sync::mpsc::Receiver<AiEvent>,
    ai_event_tx: &tokio::sync::mpsc::Sender<AiEvent>,
    ai_command_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    lsp_event_rx: &mut tokio::sync::mpsc::Receiver<LspTaskEvent>,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    dap_event_rx: &mut tokio::sync::mpsc::Receiver<mae_dap::DapTaskEvent>,
    dap_command_tx: &tokio::sync::mpsc::Sender<DapCommand>,
    mcp_tool_rx: &mut tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    mcp_socket_path: &str,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    app_config: &config::Config,
) -> io::Result<()> {
    let mut renderer = TerminalRenderer::new()?;
    let mut event_stream = EventStream::new();
    let mut pending_keys: Vec<KeyPress> = Vec::new();

    let mut deferred_ai_reply: ai_event_handler::DeferredAiReply = None;
    let mut pending_interactive_event: Option<ai_event_handler::PendingInteractiveEvent> = None;
    let mut deferred_mcp_reply: ai_event_handler::DeferredMcpReply = Vec::new();
    let mut last_mcp_activity: Option<tokio::time::Instant> = None;

    let mut shell_terminals: std::collections::HashMap<usize, mae_shell::ShellTerminal> =
        std::collections::HashMap::new();
    let mut shell_last_dims: std::collections::HashMap<usize, (u16, u16)> =
        std::collections::HashMap::new();
    let mut shell_pending_keys: Vec<KeyPress> = Vec::new();
    let mut shell_generations: std::collections::HashMap<usize, u64> =
        std::collections::HashMap::new();
    let mut last_health_check = tokio::time::Instant::now();
    let mut last_theme_name = editor.theme.name.clone();
    let mut tui_dirty = true; // start dirty for initial render

    // Frame rate limiting: render at most once per MIN_FRAME_INTERVAL.
    // First event after idle renders immediately (no input latency).
    // Rapid events coalesce into the next frame slot (Alacritty/Helix pattern).
    const MIN_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_667); // ~60fps
    let mut last_render = std::time::Instant::now() - MIN_FRAME_INTERVAL; // allow first render immediately
    let mut render_pending = false;

    loop {
        // Heartbeat for watchdog — tick each loop iteration so the watchdog
        // thread knows the main thread is alive.
        editor
            .heartbeat
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Watchdog recovery: cancel pending AI work after prolonged stall (>10s).
        if editor
            .watchdog_stall_recovery
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            tracing::warn!("watchdog recovery: cancelling pending AI work after stall");
            if let Some(ref tx) = ai_command_tx {
                let _ = tx.try_send(AiCommand::Cancel);
            }
            deferred_ai_reply = None;
            render_pending = true;
        }

        if last_health_check.elapsed() > std::time::Duration::from_secs(30) {
            shell_lifecycle::health_check(
                editor,
                &mut shell_terminals,
                deferred_ai_reply.is_some(),
                last_mcp_activity.is_some() || !deferred_mcp_reply.is_empty(),
            );
            last_health_check = tokio::time::Instant::now();
        }

        editor.clamp_all_cursors();

        let (term_w, term_h) = renderer.size()?;
        let total_window_area = mae_core::WinRect {
            x: 0,
            y: 0,
            width: term_w,
            height: term_h.saturating_sub(2),
        };
        let viewport_height = editor.focused_window_viewport_height(total_window_area);
        editor.viewport_height = viewport_height;
        editor
            .window_mgr
            .focused_window_mut()
            .ensure_scroll(viewport_height);

        // Horizontal scroll
        {
            let (term_w, term_h) = renderer.size()?;
            let window_area = mae_core::WinRect {
                x: 0,
                y: 0,
                width: term_w,
                height: term_h.saturating_sub(2),
            };
            let focused_id = editor.window_mgr.focused_id();
            let rects = editor.window_mgr.layout_rects(window_area);
            if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
                let inner_w = win_rect.width.saturating_sub(2) as usize;
                let buf = &editor.buffers[editor.active_buffer_idx()];
                let gutter_w = if editor.show_line_numbers {
                    mae_renderer::gutter_width(buf.display_line_count())
                } else {
                    2
                };
                let text_w = inner_w.saturating_sub(gutter_w);
                editor.text_area_width = text_w;
                if !editor.word_wrap {
                    editor
                        .window_mgr
                        .focused_window_mut()
                        .ensure_scroll_horizontal(text_w);
                }
            }
        }

        if tui_dirty {
            let since_last = last_render.elapsed();
            if since_last >= MIN_FRAME_INTERVAL {
                // Enough time has passed — render now (instant response).
                let frame_start = std::time::Instant::now();
                renderer.render(editor, &shell_terminals)?;
                let frame_elapsed = frame_start.elapsed().as_micros() as u64;
                editor.perf_stats.record_frame(frame_elapsed);
                if editor.debug_mode {
                    editor.perf_stats.sample_process_stats();
                }
                last_render = std::time::Instant::now();
                tui_dirty = false;
                render_pending = false;
            } else {
                // Too soon — defer render to next frame slot.
                render_pending = true;
            }
        }

        if !editor.running {
            info!("editor shutting down");

            // Fire app-exit hook.
            editor.fire_hook("app-exit");

            // Persist history
            if let Err(e) = save_history(editor) {
                error!(error = %e, "failed to save history");
            }

            // If debug mode is enabled, save a tombstone dump.
            if editor.debug_mode {
                debug_dump(editor);
            }

            // AI session persistence
            if editor.restore_session {
                if let Some(root) = editor.active_project_root() {
                    let session_path = root.join(".mae/conversation.json");
                    // Ensure directory exists
                    if let Some(parent) = session_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match editor.ai_save(&session_path) {
                        Ok(n) => {
                            info!(path = %session_path.display(), entries = n, "AI session persisted")
                        }
                        Err(e) => {
                            if !e.contains("No conversation buffer") {
                                warn!(path = %session_path.display(), error = %e, "failed to persist AI session");
                            }
                        }
                    }
                }
            }

            if let Some(ref tx) = ai_command_tx {
                if tx.try_send(AiCommand::Shutdown).is_err() {
                    warn!("failed to send shutdown to AI session (channel closed)");
                }
            }
            if lsp_command_tx.try_send(LspCommand::Shutdown).is_err() {
                warn!("failed to send shutdown to LSP task (channel closed)");
            }
            if dap_command_tx.try_send(DapCommand::Shutdown).is_err() {
                warn!("failed to send shutdown to DAP task (channel closed)");
            }
            break;
        }

        trace!("drain_intents_and_lifecycle enter");
        drain_lsp_intents(editor, lsp_command_tx);
        drain_dap_intents(editor, dap_command_tx);

        shell_lifecycle::drain_agent_setup(editor);
        shell_lifecycle::spawn_pending_shells(
            editor,
            &mut shell_terminals,
            &mut shell_last_dims,
            &renderer,
            mcp_socket_path,
            app_config,
        );
        shell_lifecycle::resize_shells(editor, &renderer, &shell_terminals, &mut shell_last_dims);
        shell_lifecycle::manage_shell_lifecycle(editor, &mut shell_terminals);
        trace!("drain_intents_and_lifecycle exit");

        // Detect theme changes and update shell terminal colors.
        if editor.theme.name != last_theme_name {
            last_theme_name = editor.theme.name.clone();
            shell_lifecycle::update_shell_theme_colors(editor, &shell_terminals);
        }

        shell_generations.retain(|idx, _| shell_terminals.contains_key(idx));

        let has_shells = !shell_terminals.is_empty();
        let shell_tick = async {
            if has_shells {
                // 20fps for shell viewport refresh — smooth enough for terminal
                // output while keeping idle CPU reasonable (~40% less than 30fps).
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        ai_event_handler::timeout_deferred_reply(editor, &mut deferred_ai_reply);
        ai_event_handler::timeout_deferred_mcp_reply(editor, &mut deferred_mcp_reply);

        let mcp_idle_tick = async {
            if last_mcp_activity.is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        // Frame timer: fires at the next render slot when a deferred render is pending.
        let frame_timer = async {
            if render_pending {
                let elapsed = last_render.elapsed();
                if elapsed < MIN_FRAME_INTERVAL {
                    tokio::time::sleep(MIN_FRAME_INTERVAL - elapsed).await;
                }
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            _ = frame_timer => {
                // Frame slot arrived — mark dirty so the render section fires.
                tui_dirty = true;
                render_pending = false;
            }
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        tui_dirty = true;
                        if editor.input_lock != mae_core::InputLock::None {
                            use crossterm::event::{KeyCode, KeyModifiers};
                            if key.code == KeyCode::Esc
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                            {
                                editor.input_lock = mae_core::InputLock::None;
                                editor.ai_streaming = false;
                                last_mcp_activity = None;
                                if let Some(ref tx) = ai_command_tx {
                                    let _ = tx.try_send(AiCommand::Cancel);
                                }
                                editor.set_status("AI operation cancelled");
                            } else if editor.mode == Mode::ShellInsert {
                                handle_shell_key(editor, key, &mut shell_terminals, &mut shell_pending_keys);
                            }
                        } else if editor.mode == Mode::ShellInsert {
                            handle_shell_key(editor, key, &mut shell_terminals, &mut shell_pending_keys);
                        } else if key.kind == KeyEventKind::Press {
                            shell_pending_keys.clear();
                            handle_key(editor, scheme, key, &mut pending_keys, ai_command_tx, &mut pending_interactive_event);

                            // Handle cancellation requested via command (e.g. SPC a c)
                            if editor.ai_cancel_requested {
                                editor.ai_cancel_requested = false;
                                if let Some(ref tx) = ai_command_tx {
                                    let _ = tx.try_send(AiCommand::Cancel);
                                }
                                editor.ai_streaming = false;
                                editor.input_lock = mae_core::InputLock::None;
                                pending_interactive_event = None;
                            }
                        }
                    }
                    Some(Ok(Event::Resize(_w, _h))) => {
                        tui_dirty = true;
                    }
                    Some(Err(e)) => {
                        tui_dirty = true;
                        editor.set_status(format!("Input error: {}", e));
                    }
                    None => break,
                    _ => {}
                }
            }
            Some(ai_event) = ai_event_rx.recv() => {
                tui_dirty = true;
                let ctx = ai_event_handler::AiEventContext {
                    all_tools,
                    permission_policy,
                    deferred_ai_reply: &mut deferred_ai_reply,
                    pending_interactive_event: &mut pending_interactive_event,
                    lsp_command_tx,
                    ai_event_tx,
                    ai_command_tx,
                };
                ai_event_handler::handle_ai_event(editor, ai_event, ctx);
            }
            Some(lsp_event) = lsp_event_rx.recv() => {
                tui_dirty = true;
                ai_event_handler::try_resolve_deferred(editor, &lsp_event, &mut deferred_ai_reply);
                if ai_event_handler::try_resolve_deferred_mcp(&lsp_event, &mut deferred_mcp_reply) {
                    last_mcp_activity = Some(tokio::time::Instant::now());
                }
                handle_lsp_event(editor, lsp_command_tx, lsp_event);
            }
            Some(dap_event) = dap_event_rx.recv() => {
                tui_dirty = true;
                handle_dap_event(editor, dap_event);
            }
            _ = shell_tick => {
                // Shell tick — only mark dirty when a shell has new output
                for (idx, term) in &shell_terminals {
                    let gen = term.generation();
                    if shell_generations.get(idx) != Some(&gen) {
                        shell_generations.insert(*idx, gen);
                        tui_dirty = true;
                    }
                }
            }
            _ = mcp_idle_tick => {
                if let Some(ts) = last_mcp_activity {
                    if ts.elapsed() > std::time::Duration::from_millis(500)
                        && deferred_mcp_reply.is_empty()
                    {
                        if editor.input_lock == mae_core::InputLock::McpBusy {
                            editor.set_status("MCP: input unlocked");
                        }
                        editor.input_lock = mae_core::InputLock::None;
                        last_mcp_activity = None;
                        tui_dirty = true;
                    }
                }
            }
            Some(mcp_req) = mcp_tool_rx.recv() => {
                tui_dirty = true;
                editor.input_lock = mae_core::InputLock::McpBusy;
                last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    editor, mcp_req, all_tools, permission_policy,
                    lsp_command_tx, &mut deferred_mcp_reply,
                );
                if immediate && deferred_mcp_reply.is_empty() {
                    editor.input_lock = mae_core::InputLock::None;
                    last_mcp_activity = None;
                }
            }
        }
    }

    renderer.cleanup()?;
    Ok(())
}

/// Remove stale MCP socket files from crashed MAE sessions.
///
/// Scans `/tmp/mae-*.sock` and removes any whose PID no longer exists.
/// Called on startup so that stale sockets from SIGKILL'd or crashed
/// sessions don't accumulate.
pub(crate) fn cleanup_stale_mcp_sockets() {
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with("mae-") || !name_str.ends_with(".sock") {
            continue;
        }
        // Extract PID from mae-{PID}.sock
        let pid_str = &name_str[4..name_str.len() - 5];
        if let Ok(pid) = pid_str.parse::<u32>() {
            // Check if the process is still alive via /proc
            if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                let path = entry.path();
                if std::fs::remove_file(&path).is_ok() {
                    info!(path = %path.display(), "removed stale MCP socket");
                }
            }
        }
    }
}

/// Headless AI self-test: sends the self-test prompt, handles tool calls,
/// prints the report to stdout, and returns an exit code (0 = all pass,
/// 1 = any failures, 2 = AI error / no response).
pub(crate) async fn run_headless_self_test(
    editor: &mut Editor,
    ai_event_rx: &mut tokio::sync::mpsc::Receiver<AiEvent>,
    ai_command_tx: &tokio::sync::mpsc::Sender<AiCommand>,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    categories: &str,
) -> i32 {
    use crate::key_handling::build_self_test_prompt;

    let prompt = build_self_test_prompt(categories);
    eprintln!("mae: sending self-test prompt to AI agent...");

    if ai_command_tx.try_send(AiCommand::Prompt(prompt)).is_err() {
        eprintln!("mae: failed to send self-test prompt (channel full or closed)");
        return 2;
    }

    // Collect all text output from the AI session.
    let mut full_report = String::new();
    let timeout = tokio::time::Duration::from_secs(300); // 5 minute timeout
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            eprintln!("mae: self-test timed out after 5 minutes");
            return 2;
        }

        let event = tokio::select! {
            ev = ai_event_rx.recv() => ev,
            _ = tokio::time::sleep(remaining) => {
                eprintln!("mae: self-test timed out after 5 minutes");
                return 2;
            }
        };

        match event {
            Some(AiEvent::ToolCallRequest { call, reply }) => {
                debug!(tool = %call.name, call_id = %call.id, "self-test tool call");
                eprintln!("  [tool] {}", call.name);

                // Push tool call to conversation buffer for report extraction.
                if let Some(conv) = find_conversation_buffer_mut(editor) {
                    conv.push_tool_call(&call.name);
                }

                let exec_result = execute_tool(editor, &call, all_tools, permission_policy);

                match exec_result {
                    ExecuteResult::Immediate(result) => {
                        if let Some(conv) = find_conversation_buffer_mut(editor) {
                            conv.push_tool_result(result.success, &result.output, None);
                        }
                        if reply.send(result).is_err() {
                            warn!("self-test tool result channel closed");
                        }
                    }
                    ExecuteResult::Deferred { tool_call_id, kind } => {
                        // For headless mode, drain LSP intents and wait for
                        // the result synchronously. This is a simplification —
                        // deferred tools (LSP) may not resolve without a running
                        // LSP server, but that's expected in headless mode.
                        drain_lsp_intents(editor, lsp_command_tx);
                        let result = ToolResult {
                            tool_call_id,
                            tool_name: match kind {
                                DeferredKind::LspDefinition => "lsp_definition",
                                DeferredKind::LspReferences => "lsp_references",
                                DeferredKind::LspHover => "lsp_hover",
                                DeferredKind::LspWorkspaceSymbol => "lsp_workspace_symbol",
                                DeferredKind::LspDocumentSymbols => "lsp_document_symbols",
                            }
                            .into(),
                            success: false,
                            output: format!(
                                "Deferred tool ({:?}) not supported in headless mode",
                                kind
                            ),
                        };
                        if let Some(conv) = find_conversation_buffer_mut(editor) {
                            conv.push_tool_result(result.success, &result.output, None);
                        }
                        if reply.send(result).is_err() {
                            warn!("self-test deferred tool channel closed");
                        }
                    }
                }
            }
            Some(AiEvent::TextResponse { text, .. }) => {
                full_report.push_str(&text);
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.push_assistant(&text);
                }
            }
            Some(AiEvent::StreamChunk { text, .. }) => {
                full_report.push_str(&text);
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.append_streaming_chunk(&text);
                }
            }
            Some(AiEvent::SessionComplete { .. }) => {
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.end_streaming();
                }
                break;
            }
            Some(AiEvent::Error(msg, _)) => {
                eprintln!("mae: AI error during self-test: {}", msg);
                return 2;
            }
            Some(_) => {
                // CostUpdate, BudgetWarning, etc. — ignore in headless mode.
            }
            None => {
                eprintln!("mae: AI event channel closed unexpectedly");
                return 2;
            }
        }
    }

    // Print report to stdout.
    println!("{}", full_report);

    // Parse pass/fail/skip counts from the report.
    let fail_count = full_report.matches("[FAIL]").count();
    let pass_count = full_report.matches("[PASS]").count();
    let skip_count = full_report.matches("[SKIP]").count();

    eprintln!(
        "mae: self-test complete — {} passed, {} failed, {} skipped",
        pass_count, fail_count, skip_count
    );

    if fail_count > 0 {
        1
    } else if pass_count == 0 {
        eprintln!("mae: warning — no PASS results found in report");
        2
    } else {
        0
    }
}
