//! Terminal event loop — the main async loop for the TUI backend.

use std::io;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mae_ai::{execute_tool, AiCommand, AiEvent, ExecuteResult, ToolResult};
use mae_core::{Editor, KeyPress, Mode};
use mae_dap::DapCommand;
use mae_lsp::{LspCommand, LspTaskEvent};
use mae_renderer::{Renderer, TerminalRenderer};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, trace, warn};

use crate::ai_event_handler;
use crate::bootstrap::{debug_dump, find_conversation_buffer_mut, save_history_on_exit};
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
    collab_event_rx: &mut tokio::sync::mpsc::Receiver<crate::collab_bridge::CollabEvent>,
    collab_command_tx: &tokio::sync::mpsc::Sender<crate::collab_bridge::CollabCommand>,
    mcp_socket_path: &str,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    app_config: &config::Config,
    mcp_client_mgr: &ai_event_handler::McpClientMgrRef,
    sync_broadcaster: &mae_mcp::broadcast::SharedBroadcaster,
) -> io::Result<()> {
    let mut renderer = TerminalRenderer::new()?;
    let mut event_stream = EventStream::new();
    let mut pending_keys: Vec<KeyPress> = Vec::new();

    let mut deferred_ai_reply: ai_event_handler::DeferredAiReply = None;
    let mut deferred_dap_reply: ai_event_handler::DeferredDapReply = None;
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

    // Set initial layout area for per-window viewport height calculations.
    if let Ok((w, h)) = renderer.size() {
        editor.last_layout_area = mae_core::WinRect {
            x: 0,
            y: 0,
            width: w,
            height: h.saturating_sub(2),
        };
    }

    loop {
        // Heartbeat for watchdog — tick each loop iteration so the watchdog
        // thread knows the main thread is alive.
        editor
            .heartbeat
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Phase 1a: consume the background primary-store preload when it finishes
        // (the GUI drains this in idle_work; the TUI loop has no idle_work, so do it
        // here — cheap Option check, populates the mirror once loading completes).
        editor.drain_kb_preload();
        // Phase 4: cross-instance mirror refresh on external store change.
        editor.drain_kb_store_watch();

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
            deferred_dap_reply = None;
            render_pending = true;
        }

        if last_health_check.elapsed() > std::time::Duration::from_secs(30) {
            shell_lifecycle::health_check(
                editor,
                &mut shell_terminals,
                deferred_ai_reply.is_some(),
                last_mcp_activity.is_some() || !deferred_mcp_reply.is_empty(),
            );
            // Rekey after health_check zombie cleanup.
            for removed_idx in std::mem::take(&mut editor.pending_buffer_removals) {
                mae_core::editor::rekey_after_remove(&mut shell_terminals, removed_idx);
                mae_core::editor::rekey_after_remove(&mut shell_last_dims, removed_idx);
                mae_core::editor::rekey_after_remove(&mut shell_generations, removed_idx);
            }
            // Autosave + swap writes (same 30s cadence as GUI HealthTick).
            editor.try_autosave();
            // On-demand daemon supervision (ADR-035 PR B2) — parity with the GUI
            // HealthCheck: restart a daemon we own if it has died (bounded).
            crate::daemon_supervisor::supervise_daemon(editor);
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
        // TUI: 1 char = 1 cell (no pixel scaling).
        editor.gui_cell_width = 1.0;
        editor.gui_cell_height = 1.0;
        // Horizontal scroll + text_area_width
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
                let gutter_w = if !mae_core::BufferMode::has_gutter(&buf.kind) {
                    0
                } else if editor.show_line_numbers {
                    mae_renderer::gutter_width(buf.display_line_count())
                } else {
                    2
                };
                let scrollbar_w: usize = if editor.scrollbar { 1 } else { 0 };
                let text_w = inner_w.saturating_sub(gutter_w).saturating_sub(scrollbar_w);
                editor.text_area_width = text_w;
                if !editor.word_wrap {
                    editor
                        .window_mgr
                        .focused_window_mut()
                        .ensure_scroll_horizontal(text_w);
                }
            }
        }

        {
            let buf_idx = editor.active_buffer_idx();
            let cursor_row = editor.window_mgr.focused_window().cursor_row;
            let scroll = editor.window_mgr.focused_window().scroll_offset;
            let so = editor.scrolloff;
            // Pass tight needed range — populate_visual_rows_cache adds padding internally.
            let cache_start = scroll.min(cursor_row).saturating_sub(1);
            let cache_end = (scroll.max(cursor_row) + viewport_height + 2)
                .min(editor.buffers[buf_idx].display_line_count());
            editor.populate_visual_rows_cache(buf_idx, cache_start, cache_end);

            // Snapshot cache Vec<u8> to avoid borrow conflict with window_mgr.
            let (cache_rows, cache_line_start) = {
                let buf = &editor.buffers[buf_idx];
                match &buf.visual_rows_cache {
                    Some(c) => (c.rows.clone(), c.line_start),
                    None => (Vec::new(), 0),
                }
            };

            let line_count = editor.buffers[buf_idx].display_line_count();
            let win = editor.window_mgr.focused_window_mut();
            if win.scroll_locked && win.cursor_row == win.scroll_locked_cursor {
                // Cursor hasn't moved since scroll command; keep lock active
            } else {
                win.scroll_locked = false;
                win.ensure_scroll_wrapped_with_margin(viewport_height, so, line_count, |line| {
                    if line >= cache_line_start && line < cache_line_start + cache_rows.len() {
                        let v = cache_rows[line - cache_line_start] as usize;
                        if v > 0 {
                            v
                        } else {
                            1
                        }
                    } else {
                        1
                    }
                });
            }
        }

        // Debounced syntax reparse: drain pending reparses after configured ms idle.
        let reparse_debounce_ms = editor.syntax_reparse_debounce_ms;
        if !editor.syntax_reparse_pending.is_empty()
            && editor.last_edit_time.elapsed()
                >= std::time::Duration::from_millis(reparse_debounce_ms)
        {
            mae_core::syntax::drain_pending_reparses(editor);
            tui_dirty = true;
        }

        // Debounced document highlight: request after 300ms cursor idle.
        if editor.lsp.highlight_ranges.is_empty()
            && editor.last_edit_time.elapsed() >= std::time::Duration::from_millis(300)
        {
            editor.lsp_request_document_highlight();
        }

        // Breadcrumbs: request/refresh on cursor idle.
        if editor.show_breadcrumbs {
            editor.request_breadcrumb_symbols();
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
                // Record frame snapshot for perf_profile tool.
                if editor.event_recorder.is_recording() {
                    let ps = &editor.perf_stats;
                    let snapshot = mae_core::event_record::FrameSnapshot {
                        offset_us: editor.event_recorder.duration_us(),
                        frame_time_us: frame_elapsed,
                        total_render_us: 0, // TUI: no separate render timing
                        render_syntax_us: ps.render_syntax_us,
                        render_layout_us: ps.render_layout_us,
                        render_draw_us: ps.render_draw_us,
                        redraw_level: format!("{:?}", editor.redraw_level),
                        scroll_offset: editor.window_mgr.focused_window().scroll_offset,
                        syntax_cache_hit: ps.syntax_cache_hits > 0 && ps.syntax_cache_misses == 0,
                        visual_rows_cache_hit: ps.visual_rows_cache_hits > 0
                            && ps.visual_rows_cache_misses == 0,
                    };
                    editor.event_recorder.record_frame_snapshot(snapshot);
                }
                editor.clear_redraw();
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

            // Phase D3b: snapshot the daemon-hosted primary mirror back to the local
            // store so per-edit-retired edits land in the daemon-less fallback.
            if editor.kb.daemon_hosts_primary() {
                editor.kb_snapshot_primary_to_store();
            }

            // Persist history (skipped in clean mode)
            if !editor.clean_mode {
                if let Err(e) = save_history_on_exit(editor) {
                    error!(error = %e, "failed to save history");
                }
                // Save persistent project list
                if let Some(data_dir) = editor.mae_data_dir() {
                    crate::bootstrap::save_project_list_on_exit(editor, &data_dir);
                }
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
        crate::scheme_lsp_bridge::drain_scheme_lsp_intents(editor, scheme);
        drain_lsp_intents(editor, lsp_command_tx);
        crate::scheme_dap_bridge::drain_scheme_dap_intents(editor, scheme);
        drain_dap_intents(editor, dap_command_tx);
        crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
        crate::collab_bridge::queue_awareness_update(editor);
        crate::collab_bridge::cleanup_stale_awareness(editor);

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

        // Rekey binary-owned shell maps after any buffer removals this tick.
        for removed_idx in std::mem::take(&mut editor.pending_buffer_removals) {
            mae_core::editor::rekey_after_remove(&mut shell_terminals, removed_idx);
            mae_core::editor::rekey_after_remove(&mut shell_last_dims, removed_idx);
            mae_core::editor::rekey_after_remove(&mut shell_generations, removed_idx);
        }

        // Process module reload requests.
        let reloads = std::mem::take(&mut editor.pending_module_reloads);
        for module_name in reloads {
            if module_name == "__all__" {
                // Full reload pipeline (init → modules → config.scm → default_mode),
                // not modules-only, so `:reload-modules` matches startup (C1/H2).
                crate::bootstrap::reload_everything(scheme, editor, None);
            } else if let Some(flavor) = module_name.strip_prefix("__flavor:") {
                crate::bootstrap::switch_keymap_flavor(scheme, editor, flavor);
            } else {
                crate::bootstrap::reload_module(&module_name, scheme, editor);
            }
        }
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
        ai_event_handler::timeout_deferred_dap_reply(editor, &mut deferred_dap_reply);
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

        // Syntax reparse timer: fires after configured ms when reparses are pending.
        let has_pending_reparse = !editor.syntax_reparse_pending.is_empty();
        let reparse_sleep_dur = if has_pending_reparse {
            let debounce = std::time::Duration::from_millis(reparse_debounce_ms);
            debounce.checked_sub(editor.last_edit_time.elapsed())
        } else {
            None
        };
        let syntax_reparse_timer = async {
            if let Some(dur) = reparse_sleep_dur {
                tokio::time::sleep(dur).await;
            } else if has_pending_reparse {
                // debounce already expired, fire immediately
            } else {
                std::future::pending::<()>().await;
            }
        };

        tokio::select! {
            _ = frame_timer => {
                // Frame slot arrived — mark dirty so the render section fires.
                tui_dirty = true;
                render_pending = false;
                // Drain sync updates on frame tick (~16ms max latency).
                crate::sync_broadcast::drain_and_broadcast(editor, sync_broadcaster, Some(collab_command_tx));
            }
            _ = syntax_reparse_timer => {
                // Debounce expired — drain pending reparses.
                mae_core::syntax::drain_pending_reparses(editor);
                tui_dirty = true;
            }
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        tui_dirty = true;
                        editor.last_edit_time = std::time::Instant::now();
                        editor.clear_highlights();
                        if editor.ai.input_lock != mae_core::InputLock::None {
                            use crossterm::event::{KeyCode, KeyModifiers};
                            if key.code == KeyCode::Esc
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                            {
                                editor.ai.input_lock = mae_core::InputLock::None;
                                editor.ai.streaming = false;
                                last_mcp_activity = None;
                                if let Some(ref tx) = ai_command_tx {
                                    let _ = tx.try_send(AiCommand::Cancel);
                                }
                                if editor.cleanup_self_test() {
                                    editor.set_status("[AI] Cancelled — self-test state restored");
                                } else {
                                    editor.set_status("AI operation cancelled");
                                }
                            } else if editor.mode == Mode::ShellInsert {
                                handle_shell_key(editor, key, &mut shell_terminals, &mut shell_pending_keys);
                            }
                        } else if editor.mode == Mode::ShellInsert {
                            handle_shell_key(editor, key, &mut shell_terminals, &mut shell_pending_keys);
                        } else if key.kind == KeyEventKind::Press {
                            shell_pending_keys.clear();
                            editor.mark_cursor_moved();
                            handle_key(editor, scheme, key, &mut pending_keys, ai_command_tx, &mut pending_interactive_event);

                            // Handle cancellation requested via command (e.g. SPC a c)
                            if editor.ai.cancel_requested {
                                editor.ai.cancel_requested = false;
                                if let Some(ref tx) = ai_command_tx {
                                    let _ = tx.try_send(AiCommand::Cancel);
                                }
                                editor.ai.streaming = false;
                                editor.ai.input_lock = mae_core::InputLock::None;
                                pending_interactive_event = None;
                                if editor.cleanup_self_test() {
                                    editor.set_status("[AI] Cancelled — self-test state restored");
                                }
                            }
                        }
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        use crossterm::event::{MouseButton as XButton, MouseEventKind};
                        tui_dirty = true;
                        match mouse.kind {
                            MouseEventKind::Down(XButton::Left) => {
                                let shift = mouse.modifiers.contains(crossterm::event::KeyModifiers::SHIFT);
                                // Try focus window at click position first.
                                editor.focus_window_at(mouse.column, mouse.row);
                                editor.handle_mouse_click_shift(
                                    mouse.row as usize,
                                    mouse.column as usize,
                                    mae_core::MouseButton::Left,
                                    shift,
                                );
                            }
                            MouseEventKind::Down(XButton::Right) => {
                                editor.handle_mouse_click(
                                    mouse.row as usize,
                                    mouse.column as usize,
                                    mae_core::MouseButton::Right,
                                );
                            }
                            MouseEventKind::Down(XButton::Middle) => {
                                editor.handle_mouse_click(
                                    mouse.row as usize,
                                    mouse.column as usize,
                                    mae_core::MouseButton::Middle,
                                );
                            }
                            MouseEventKind::Drag(XButton::Left) => {
                                editor.handle_mouse_drag(mouse.row as usize, mouse.column as usize);
                            }
                            MouseEventKind::Up(XButton::Left) => {
                                editor.handle_mouse_release(mouse.row as usize, mouse.column as usize);
                            }
                            MouseEventKind::ScrollUp => {
                                editor.handle_mouse_scroll(1);
                            }
                            MouseEventKind::ScrollDown => {
                                editor.handle_mouse_scroll(-1);
                            }
                            MouseEventKind::ScrollLeft => {
                                editor.handle_mouse_scroll_horizontal(-1);
                            }
                            MouseEventKind::ScrollRight => {
                                editor.handle_mouse_scroll_horizontal(1);
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Event::FocusGained)) => {
                        // Check if current buffer's file changed on disk
                        let idx = editor.active_buffer_idx();
                        if editor.mini_dialog.is_none() {
                            editor.check_and_reload_buffer(idx);
                        }
                        tui_dirty = true;
                    }
                    Some(Ok(Event::Resize(w, h))) => {
                        editor.last_layout_area = mae_core::WinRect {
                            x: 0,
                            y: 0,
                            width: w,
                            height: h.saturating_sub(2),
                        };
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
                    deferred_dap_reply: &mut deferred_dap_reply,
                    pending_interactive_event: &mut pending_interactive_event,
                    lsp_command_tx,
                    dap_command_tx,
                    ai_event_tx,
                    scheme,
                    mcp_client_mgr,
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
                // Try to resolve deferred DAP tool first (promise/await)
                let dap_action = ai_event_handler::try_resolve_deferred_dap(
                    editor, &dap_event, &mut deferred_dap_reply,
                );
                // Always process the event for editor state updates
                handle_dap_event(editor, dap_event);
                // After handle_dap_event queues RefreshThreadsAndStack,
                // drain intents immediately so the DAP task gets it
                if dap_action == ai_event_handler::DapResolveAction::TransitionedToStackTrace {
                    drain_dap_intents(editor, dap_command_tx);
                }
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
                        if editor.ai.input_lock == mae_core::InputLock::McpBusy {
                            editor.set_status("MCP: input unlocked");
                        }
                        editor.ai.input_lock = mae_core::InputLock::None;
                        last_mcp_activity = None;
                        tui_dirty = true;
                    }
                }
            }
            Some(mcp_req) = mcp_tool_rx.recv() => {
                tui_dirty = true;
                editor.ai.input_lock = mae_core::InputLock::McpBusy;
                last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    editor, mcp_req, all_tools, permission_policy,
                    lsp_command_tx, &mut deferred_mcp_reply, scheme,
                );
                if immediate && deferred_mcp_reply.is_empty() {
                    editor.ai.input_lock = mae_core::InputLock::None;
                    last_mcp_activity = None;
                }
                // Drain hooks queued by MCP-driven commands (e.g. mode-change).
                crate::key_handling::drain_hook_evals(editor, scheme);
                // Drain sync updates immediately after MCP-driven edits.
                crate::sync_broadcast::drain_and_broadcast(editor, sync_broadcaster, Some(collab_command_tx));
            }
            Some(collab_event) = collab_event_rx.recv() => {
                tui_dirty = true;
                crate::collab_bridge::handle_collab_event(editor, collab_event);
            }
        }
    }

    renderer.cleanup()?;
    Ok(())
}

/// Parse the PID out of a `mae-{pid}.sock` / `mae-{pid}-agent.sock` /
/// `mae-{pid}.psk` file name, or `None` if it doesn't match any of those
/// shapes. Pure string logic, split out from [`cleanup_stale_mcp_sockets`] so
/// it's unit-testable without touching the real filesystem.
fn extract_pid_from_mcp_file_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("mae-")?;
    let pid_str = rest
        .strip_suffix("-agent.sock")
        .or_else(|| rest.strip_suffix(".sock"))
        .or_else(|| rest.strip_suffix(".psk"))?;
    pid_str.parse::<u32>().ok()
}

/// Remove stale MCP socket/PSK files from crashed MAE sessions.
///
/// Scans `/tmp/mae-*.sock`, `/tmp/mae-*-agent.sock` (ADR-048's PSK-required
/// agent socket), and `/tmp/mae-*.psk` (its per-process secret), removing any
/// whose PID no longer exists. Called on startup so that stale sockets/secrets
/// from SIGKILL'd or crashed sessions don't accumulate — a stale `.psk` file is
/// worth cleaning up promptly since it's a live secret, not just clutter.
pub(crate) fn cleanup_stale_mcp_sockets() {
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Some(pid) = extract_pid_from_mcp_file_name(name_str) else {
            continue;
        };
        // Check if the process is still alive via /proc
        if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
            let path = entry.path();
            if std::fs::remove_file(&path).is_ok() {
                info!(path = %path.display(), "removed stale MCP socket/key file");
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
                        // For headless mode, drain LSP intents but can't resolve
                        // deferred tools (LSP/DAP) without running servers.
                        if kind.is_lsp() {
                            drain_lsp_intents(editor, lsp_command_tx);
                        }
                        // DAP intents: no dap_command_tx in headless mode
                        let result = ToolResult {
                            tool_call_id,
                            tool_name: kind.tool_name().into(),
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

#[cfg(test)]
mod tests {
    use super::extract_pid_from_mcp_file_name;

    #[test]
    fn extracts_pid_from_plain_socket() {
        assert_eq!(extract_pid_from_mcp_file_name("mae-1234.sock"), Some(1234));
    }

    #[test]
    fn extracts_pid_from_agent_socket() {
        // ADR-048's PSK-required agent socket — must be matched before the
        // plain `.sock` suffix (a naive `.sock`-only strip would leave a
        // trailing `-agent` that fails to parse as a PID).
        assert_eq!(
            extract_pid_from_mcp_file_name("mae-5678-agent.sock"),
            Some(5678)
        );
    }

    #[test]
    fn extracts_pid_from_psk_file() {
        assert_eq!(extract_pid_from_mcp_file_name("mae-9999.psk"), Some(9999));
    }

    #[test]
    fn rejects_non_mae_prefixed_names() {
        assert_eq!(extract_pid_from_mcp_file_name("other-1234.sock"), None);
        assert_eq!(extract_pid_from_mcp_file_name("1234.sock"), None);
    }

    #[test]
    fn rejects_unrecognized_suffixes() {
        assert_eq!(extract_pid_from_mcp_file_name("mae-1234.txt"), None);
        assert_eq!(extract_pid_from_mcp_file_name("mae-1234"), None);
    }

    #[test]
    fn rejects_non_numeric_pid() {
        assert_eq!(extract_pid_from_mcp_file_name("mae-abc.sock"), None);
        assert_eq!(extract_pid_from_mcp_file_name("mae-abc-agent.sock"), None);
        assert_eq!(extract_pid_from_mcp_file_name("mae-abc.psk"), None);
    }
}
