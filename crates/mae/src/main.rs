mod agents;
mod ai_event_handler;
mod bootstrap;
mod config;
mod key_handling;
mod shell_lifecycle;

use std::io;
use std::panic;

use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt;
use mae_ai::{
    ai_specific_tools, execute_tool, tools_from_registry, AiCommand, AiEvent, DeferredKind,
    ExecuteResult, ToolResult,
};
use mae_core::{
    Buffer, CompletionItem as CoreCompletionItem, DapIntent, Diagnostic as CoreDiagnostic,
    DiagnosticSeverity as CoreSeverity, Editor, KeyPress, LspIntent, LspLocation, LspRange, Mode,
};
use mae_dap::{DapCommand, DapServerConfig, DapTaskEvent, SourceBreakpoint};
use mae_lsp::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, LspCommand, LspTaskEvent, Position,
};
use mae_renderer::{Renderer, TerminalRenderer};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};

use bootstrap::{
    find_conversation_buffer_mut, init_logging, load_init_file, setup_ai, setup_dap, setup_lsp,
};
use key_handling::handle_key;

/// Async event loop for the MAE editor.
///
/// Uses tokio::select! to multiplex keyboard input and AI agent events.
/// The AI agent runs on a spawned tokio task, communicating via channels.
///
/// Emacs lesson: Emacs's event loop is synchronous and single-threaded.
/// Retrofitting concurrency required 23,901 commits across 3 GC branches.
/// We use async from day one so the AI agent can operate as a peer.
#[tokio::main(flavor = "current_thread")]
async fn main() -> io::Result<()> {
    // Create the in-editor message log first, then wire it into both
    // the tracing subscriber (for structured JSON logs to stderr + in-editor capture)
    // and the Editor (for the :messages command).
    let message_log = mae_core::MessageLog::new(1000);
    let log_handle = message_log.handle();
    init_logging(log_handle);

    info!(version = env!("CARGO_PKG_VERSION"), "mae starting");

    // Set up panic hook to restore terminal on crash
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Best-effort terminal cleanup — swallow errors since we're already panicking
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        default_hook(info);
    }));

    let args: Vec<String> = std::env::args().collect();

    // Handle --version / --help / --init-config before the terminal UI starts.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mae {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("mae {} — Modern AI Editor", env!("CARGO_PKG_VERSION"));
        println!();
        println!("USAGE:");
        println!("  mae [FILE]");
        println!();
        println!("OPTIONS:");
        println!("  -h, --help              Print this help");
        println!("  -V, --version           Print version");
        println!("  --init-config [--force] Write a commented template and run wizard");
        println!("  --print-config-path     Print the config file path and exit");
        println!("  --print-config-template Print the default commented template to stdout");
        println!("  --gui                   Launch with GUI backend (winit + skia)");
        println!("  --debug                 Enable debug mode (RSS/CPU/frame time in status bar)");
        println!("  --setup-agents [DIR]    Write .mcp.json for agent auto-discovery");
        println!("  --self-test [CATS]      Run AI self-test headless, exit with pass/fail code");
        println!();
        println!("CONFIG:");
        println!("  {}", config::config_path().display());
        println!();
        println!("ENVIRONMENT:");
        println!("  MAE_AI_PROVIDER     claude | openai | ollama");
        println!("  MAE_AI_MODEL        model identifier");
        println!("  MAE_AI_BASE_URL     custom API base URL (for Ollama/vLLM/proxies)");
        println!("  MAE_AI_TIMEOUT_SECS HTTP timeout (default 300)");
        println!("  ANTHROPIC_API_KEY   Claude API key");
        println!("  OPENAI_API_KEY      OpenAI API key");
        println!("  MAE_AI_PERMISSIONS  readonly | standard | trusted | full");
        println!("  MAE_AGENTS_AUTO_MCP=0 Disable auto .mcp.json on terminal spawn");
        println!("  MAE_SKIP_WIZARD=1   Skip the first-run wizard");
        println!("  MAE_LOG / RUST_LOG  tracing filter (e.g. mae=debug)");
        return Ok(());
    }
    if args.iter().any(|a| a == "--print-config-path") {
        println!("{}", config::config_path().display());
        return Ok(());
    }
    if args.iter().any(|a| a == "--print-config-template") {
        print!("{}", config::default_config_template());
        return Ok(());
    }
    if args.iter().any(|a| a == "--setup-agents") {
        let dir = args
            .iter()
            .position(|a| a == "--setup-agents")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let shim = agents::resolve_shim_path();
        match agents::write_mcp_json(&dir, &shim) {
            Ok(()) => {
                println!("Wrote {}", dir.join(".mcp.json").display());
                return Ok(());
            }
            Err(e) => {
                eprintln!("Failed: {}", e);
                std::process::exit(1);
            }
        }
    }
    if args.iter().any(|a| a == "--init-config") {
        let force = args.iter().any(|a| a == "--force");
        if force || !config::config_path().exists() {
            // Template first (safer than running the wizard blind).
            match config::write_template_config(force) {
                Ok(path) => println!("Wrote template to {}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    eprintln!("{}", e);
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        config::run_wizard()?;
        return Ok(());
    }

    // First-run wizard: runs only when stdin is a TTY, no config file exists,
    // no AI env vars are set, and MAE_SKIP_WIZARD is not set. Must run before
    // the renderer takes over the terminal.
    if let Err(e) = config::maybe_run_first_run_wizard() {
        eprintln!("warning: first-run wizard failed: {}", e);
    }

    // Find the first positional argument (not a flag).
    let file_arg = args.iter().skip(1).find(|a| !a.starts_with('-'));

    let mut editor = if let Some(path) = file_arg {
        match Buffer::from_file(std::path::Path::new(path)) {
            Ok(buf) => {
                info!(path, "opened file from CLI argument");
                let mut ed = Editor::with_buffer(buf);
                // Queue an LSP didOpen for the CLI-loaded buffer.
                ed.lsp_notify_did_open();
                ed
            }
            Err(e) => {
                error!(path, error = %e, "failed to open file");
                return Err(e);
            }
        }
    } else {
        Editor::new()
    };
    editor.message_log = message_log;

    // Auto-detect project from CWD if not already set (e.g. no-file-arg startup).
    if editor.project.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(root) = mae_core::detect_project_root(&cwd) {
                editor.recent_projects.push(root.clone());
                editor.project = Some(mae_core::Project::from_root(root));
            }
        }
    }

    // Apply editor preferences from config file.
    let app_config = config::load_config();
    if let Some(ref theme) = app_config.editor.theme {
        editor.set_theme_by_name(theme);
    }
    if let Some(ref art) = app_config.editor.splash_art {
        editor.splash_art = Some(art.clone());
    }

    // Initialize Scheme runtime
    let mut scheme = match SchemeRuntime::new() {
        Ok(rt) => {
            info!("scheme runtime initialized");
            rt
        }
        Err(e) => {
            error!(error = %e, "failed to initialize scheme runtime");
            return Err(io::Error::other(e.message));
        }
    };

    // Load init.scm if it exists
    load_init_file(&mut scheme, &mut editor);

    // Initialize AI agent (if configured)
    let (mut ai_event_rx, ai_command_tx) = setup_ai(&editor);
    info!(
        ai_configured = ai_command_tx.is_some(),
        "AI agent setup complete"
    );

    // Initialize LSP coordinator task.
    let (mut lsp_event_rx, lsp_command_tx) = setup_lsp();
    info!("LSP task spawned");

    // Initialize DAP coordinator task.
    let (mut dap_event_rx, dap_command_tx) = setup_dap();
    info!("DAP task spawned");

    // Build tool list for AI executor (used when handling tool call requests)
    let all_tools = {
        let mut tools = tools_from_registry(&editor.commands);
        tools.extend(ai_specific_tools());
        tools
    };
    let permission_policy = config::resolve_permission_policy(&app_config);

    // MCP bridge: Unix socket for external agents (Claude Code, etc.)
    // Clean stale sockets from crashed MAE sessions before binding our own.
    cleanup_stale_mcp_sockets();
    let mcp_socket_path = format!("/tmp/mae-{}.sock", std::process::id());
    let (mcp_tool_tx, mut mcp_tool_rx) = tokio::sync::mpsc::channel::<mae_mcp::McpToolRequest>(16);
    {
        let mcp_tools: Vec<mae_mcp::protocol::ToolInfo> = all_tools
            .iter()
            .map(|t| mae_mcp::protocol::ToolInfo {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: serde_json::to_value(&t.parameters).unwrap_or_default(),
            })
            .collect();
        let server = mae_mcp::McpServer::new(&mcp_socket_path, mcp_tool_tx);
        tokio::spawn(server.run(mcp_tools));
        info!(socket = %mcp_socket_path, "MCP server started");
    }

    // --self-test [categories] — headless AI self-test.
    if args.iter().any(|a| a == "--self-test") {
        let categories = args
            .iter()
            .position(|a| a == "--self-test")
            .and_then(|i| args.get(i + 1))
            .filter(|a| !a.starts_with('-'))
            .map(|s| s.as_str())
            .unwrap_or("");

        if ai_command_tx.is_none() {
            eprintln!("mae: --self-test requires an AI provider. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
            std::process::exit(1);
        }

        let exit_code = run_headless_self_test(
            &mut editor,
            &mut ai_event_rx,
            ai_command_tx.as_ref().unwrap(),
            &lsp_command_tx,
            &all_tools,
            &permission_policy,
            categories,
        )
        .await;

        // Clean up MCP socket.
        let _ = std::fs::remove_file(&mcp_socket_path);
        std::process::exit(exit_code);
    }

    // --debug: enable debug mode (RSS/CPU/frame time in status bar)
    if args.iter().any(|a| a == "--debug") {
        editor.debug_mode = true;
        editor.show_fps = true;
        if std::env::var("MAE_LOG").is_err() && std::env::var("RUST_LOG").is_err() {
            std::env::set_var("MAE_LOG", "debug");
        }
        info!("debug mode enabled via --debug flag");
    }

    let use_gui = args.iter().any(|a| a == "--gui");

    if use_gui {
        #[cfg(not(feature = "gui"))]
        {
            eprintln!("mae: GUI backend not compiled in. Rebuild with: cargo build --features gui");
            std::process::exit(1);
        }
        #[cfg(feature = "gui")]
        {
            return run_gui_loop(
                editor,
                scheme,
                ai_event_rx,
                ai_command_tx,
                lsp_event_rx,
                lsp_command_tx,
                dap_event_rx,
                dap_command_tx,
                mcp_tool_rx,
                mcp_socket_path,
                all_tools,
                permission_policy,
                app_config,
            )
            .await;
        }
    }

    let mut renderer = TerminalRenderer::new()?;
    let mut event_stream = EventStream::new();
    let mut pending_keys: Vec<KeyPress> = Vec::new();

    let mut deferred_ai_reply: ai_event_handler::DeferredAiReply = None;
    let mut deferred_mcp_reply: ai_event_handler::DeferredMcpReply = Vec::new();
    let mut last_mcp_activity: Option<tokio::time::Instant> = None;

    // Active shell terminals, keyed by buffer index.
    let mut shell_terminals: std::collections::HashMap<usize, mae_shell::ShellTerminal> =
        std::collections::HashMap::new();
    // Last-known PTY dimensions per shell, for dynamic resize detection.
    let mut shell_last_dims: std::collections::HashMap<usize, (u16, u16)> =
        std::collections::HashMap::new();
    // Accumulated key presses for shell-insert keymap lookup.
    let mut shell_pending_keys: Vec<KeyPress> = Vec::new();
    let mut last_health_check = tokio::time::Instant::now();

    loop {
        // Periodic health check (~30s): scan for zombie shells, stale locks.
        if last_health_check.elapsed() > std::time::Duration::from_secs(30) {
            shell_lifecycle::health_check(
                &mut editor,
                &mut shell_terminals,
                deferred_ai_reply.is_some(),
                last_mcp_activity.is_some() || !deferred_mcp_reply.is_empty(),
            );
            last_health_check = tokio::time::Instant::now();
        }

        // Clamp all window cursors to buffer bounds. This is a safety net:
        // MCP/AI tool calls and user key mashing can leave cursor_row past
        // the end of a modified buffer. Without this, rope.line(cursor_row)
        // panics on the next render or dispatch.
        editor.clamp_all_cursors();

        // Update viewport dimensions and scroll before rendering
        let viewport_height = renderer.viewport_height()?;
        editor.viewport_height = viewport_height;
        editor
            .window_mgr
            .focused_window_mut()
            .ensure_scroll(viewport_height);

        // Horizontal scroll: compute text width from focused window's actual area
        {
            let (term_w, term_h) = renderer.size()?;
            let window_area = mae_core::WinRect {
                x: 0,
                y: 0,
                width: term_w,
                height: term_h.saturating_sub(2), // status bar + command line
            };
            let focused_id = editor.window_mgr.focused_id();
            let rects = editor.window_mgr.layout_rects(window_area);
            if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
                // inner_rect subtracts 2 for border, gutter takes more
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

        let frame_start = std::time::Instant::now();
        renderer.render(&mut editor, &shell_terminals)?;
        let frame_elapsed = frame_start.elapsed().as_micros() as u64;
        editor.perf_stats.record_frame(frame_elapsed);
        if editor.debug_mode {
            editor.perf_stats.sample_process_stats();
        }

        if !editor.running {
            info!("editor shutting down");
            if let Some(ref tx) = ai_command_tx {
                if tx.try_send(AiCommand::Shutdown).is_err() {
                    warn!("failed to send shutdown to AI session (channel closed)");
                }
            }
            // Best-effort LSP shutdown so language servers get a clean exit.
            if lsp_command_tx.try_send(LspCommand::Shutdown).is_err() {
                warn!("failed to send shutdown to LSP task (channel closed)");
            }
            // Best-effort DAP shutdown so the adapter process gets killed.
            if dap_command_tx.try_send(DapCommand::Shutdown).is_err() {
                warn!("failed to send shutdown to DAP task (channel closed)");
            }
            break;
        }

        // Drain any LSP / DAP intents queued by the last command dispatch.
        drain_lsp_intents(&mut editor, &lsp_command_tx);
        drain_dap_intents(&mut editor, &dap_command_tx);

        shell_lifecycle::drain_agent_setup(&mut editor);
        shell_lifecycle::spawn_pending_shells(
            &mut editor,
            &mut shell_terminals,
            &mut shell_last_dims,
            &renderer,
            &mcp_socket_path,
            &app_config,
        );
        shell_lifecycle::resize_shells(&editor, &renderer, &shell_terminals, &mut shell_last_dims);
        shell_lifecycle::manage_shell_lifecycle(&mut editor, &mut shell_terminals);

        // When shell terminals are active, poll for new output at ~30fps.
        // Without this, shell output only renders on the next keypress.
        let has_shells = !shell_terminals.is_empty();
        let shell_tick = async {
            if has_shells {
                tokio::time::sleep(std::time::Duration::from_millis(33)).await;
            } else {
                // No shells — sleep forever (never wake from this branch).
                std::future::pending::<()>().await;
            }
        };

        ai_event_handler::timeout_deferred_reply(&mut editor, &mut deferred_ai_reply);
        ai_event_handler::timeout_deferred_mcp_reply(&mut editor, &mut deferred_mcp_reply);

        // MCP session-scoped input lock: auto-unlock after 500ms of inactivity.
        let mcp_idle_tick = async {
            if last_mcp_activity.is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        // Async event loop: select! over keyboard + AI + shell tick
        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        // Input lock: during AI operations, discard all input
                        // except Esc/Ctrl-C which cancel and release the lock.
                        // Checked here (not in handle_key) so ShellInsert mode
                        // is also covered.
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
                                // Allow shell input even during AI/MCP lock.
                                handle_shell_key(&mut editor, key, &mut shell_terminals, &mut shell_pending_keys);
                            }
                            // All other keys discarded while locked.
                        } else if editor.mode == Mode::ShellInsert {
                            handle_shell_key(&mut editor, key, &mut shell_terminals, &mut shell_pending_keys);
                        } else if key.kind == KeyEventKind::Press {
                            shell_pending_keys.clear();
                            handle_key(&mut editor, &mut scheme, key, &mut pending_keys, &ai_command_tx);
                        }
                    }
                    Some(Ok(Event::Resize(_w, _h))) => {
                        // Terminal resized — per-shell resize happens in the
                        // dynamic resize block at the top of the loop, which
                        // uses layout_rects() to compute per-window dimensions.
                    }
                    Some(Err(e)) => {
                        editor.set_status(format!("Input error: {}", e));
                    }
                    None => break,
                    _ => {}
                }
            }
            Some(ai_event) = ai_event_rx.recv() => {
                ai_event_handler::handle_ai_event(
                    &mut editor, ai_event, &all_tools, &permission_policy,
                    &mut deferred_ai_reply, &lsp_command_tx,
                );
            }
            Some(lsp_event) = lsp_event_rx.recv() => {
                ai_event_handler::try_resolve_deferred(&mut editor, &lsp_event, &mut deferred_ai_reply);
                if ai_event_handler::try_resolve_deferred_mcp(&lsp_event, &mut deferred_mcp_reply) {
                    last_mcp_activity = Some(tokio::time::Instant::now());
                }
                handle_lsp_event(&mut editor, &lsp_command_tx, lsp_event);
            }
            Some(dap_event) = dap_event_rx.recv() => {
                handle_dap_event(&mut editor, dap_event);
            }
            _ = shell_tick => {
                // Shell output tick — just re-render (top of loop handles it).
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
                    }
                }
            }
            Some(mcp_req) = mcp_tool_rx.recv() => {
                editor.input_lock = mae_core::InputLock::McpBusy;
                last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    &mut editor, mcp_req, &all_tools, &permission_policy,
                    &lsp_command_tx, &mut deferred_mcp_reply,
                );
                // Immediate tools: clear lock right away if no deferred calls pending.
                if immediate && deferred_mcp_reply.is_empty() {
                    editor.input_lock = mae_core::InputLock::None;
                    last_mcp_activity = None;
                }
            }
        }
    }

    // Clean up MCP socket.
    let _ = std::fs::remove_file(&mcp_socket_path);

    renderer.cleanup()?;
    info!("mae exited cleanly");
    Ok(())
}

/// Remove stale MCP socket files from crashed MAE sessions.
///
/// Scans `/tmp/mae-*.sock` and removes any whose PID no longer exists.
/// Called on startup so that stale sockets from SIGKILL'd or crashed
/// sessions don't accumulate.
fn cleanup_stale_mcp_sockets() {
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
async fn run_headless_self_test(
    editor: &mut Editor,
    ai_event_rx: &mut tokio::sync::mpsc::Receiver<AiEvent>,
    ai_command_tx: &tokio::sync::mpsc::Sender<AiCommand>,
    lsp_command_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    all_tools: &[mae_ai::ToolDefinition],
    permission_policy: &mae_ai::PermissionPolicy,
    categories: &str,
) -> i32 {
    use key_handling::build_self_test_prompt;

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
            Some(AiEvent::TextResponse(text)) => {
                full_report.push_str(&text);
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.push_assistant(&text);
                }
            }
            Some(AiEvent::StreamChunk(text)) => {
                full_report.push_str(&text);
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.append_streaming_chunk(&text);
                }
            }
            Some(AiEvent::SessionComplete(_)) => {
                if let Some(conv_buf) = find_conversation_buffer_mut(editor) {
                    conv_buf.end_streaming();
                }
                break;
            }
            Some(AiEvent::Error(msg)) => {
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

/// Handle a key event while in ShellInsert mode.
///
/// Keys are checked against the "shell-insert" keymap first. If the key
/// sequence matches a binding, the command is dispatched. If it's a prefix
/// of a binding, the key is held until more keys arrive. Otherwise, all
/// Compute the PTY-appropriate cols/rows for a shell in a given buffer,
/// accounting for split window dimensions via `layout_rects()`.
///
/// Falls back to full terminal dimensions if the buffer isn't visible
/// in any window (shouldn't happen in practice).
pub(crate) fn shell_dims_for_buffer(
    editor: &Editor,
    renderer: &dyn Renderer,
    buf_idx: usize,
) -> (u16, u16) {
    let (term_w, term_h) = renderer.size().unwrap_or((80, 24));
    let window_area = mae_core::WinRect {
        x: 0,
        y: 0,
        width: term_w,
        height: term_h.saturating_sub(2), // status bar + command line
    };
    let rects = editor.window_mgr.layout_rects(window_area);

    // Find the window that owns this buffer.
    for win in editor.window_mgr.iter_windows() {
        if win.buffer_idx == buf_idx {
            if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == win.id) {
                let cols = rect.width.saturating_sub(2); // border
                let rows = rect.height;
                return (cols, rows);
            }
        }
    }

    // Fallback: full terminal minus chrome.
    (term_w.saturating_sub(4), term_h.saturating_sub(4))
}

/// pending keys are translated to PTY byte sequences and forwarded.
///
/// This replaces the previous hardcoded Ctrl-\ Ctrl-n escape sequence with
/// the standard keymap system — the Lisp machine principle that all
/// user-facing behavior must be hot-reloadable.
fn handle_shell_key(
    editor: &mut Editor,
    key: crossterm::event::KeyEvent,
    shell_terminals: &mut std::collections::HashMap<usize, mae_shell::ShellTerminal>,
    shell_pending_keys: &mut Vec<KeyPress>,
) {
    use mae_core::LookupResult;

    let Some(kp) = key_handling::crossterm_to_keypress(&key) else {
        return;
    };

    shell_pending_keys.push(kp);

    // Look up accumulated keys in the shell-insert keymap.
    let lookup = editor
        .keymaps
        .get("shell-insert")
        .map(|km| km.lookup(shell_pending_keys))
        .unwrap_or(LookupResult::None);

    match lookup {
        LookupResult::Exact(cmd) => {
            let cmd = cmd.to_string();
            shell_pending_keys.clear();
            editor.execute_command(&cmd);
        }
        LookupResult::Prefix => {
            // Wait for more keys — don't send anything to PTY yet.
        }
        LookupResult::None => {
            // No binding matches. Flush all pending keys to the PTY.
            let keys_to_send = std::mem::take(shell_pending_keys);

            let Some(shell) = shell_terminals.get(&editor.active_buffer_idx()) else {
                editor.mode = Mode::Normal;
                editor.set_status("Terminal exited — returned to normal mode");
                return;
            };

            if shell.has_exited() {
                editor.mode = Mode::Normal;
                editor.set_status("Terminal process has exited");
                return;
            }

            for kp in &keys_to_send {
                let bytes = keypress_to_pty_bytes(kp);
                if !bytes.is_empty() {
                    shell.write_input(&bytes);
                }
            }
        }
    }
}

/// Convert a mae_core KeyPress into PTY byte sequences for the shell.
fn keypress_to_pty_bytes(kp: &KeyPress) -> Vec<u8> {
    use mae_core::Key;

    match &kp.key {
        Key::Char(c) => {
            if kp.ctrl {
                let byte = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                vec![byte]
            } else if kp.alt {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::F(1) => b"\x1bOP".to_vec(),
        Key::F(2) => b"\x1bOQ".to_vec(),
        Key::F(3) => b"\x1bOR".to_vec(),
        Key::F(4) => b"\x1bOS".to_vec(),
        Key::F(5) => b"\x1b[15~".to_vec(),
        Key::F(6) => b"\x1b[17~".to_vec(),
        Key::F(7) => b"\x1b[18~".to_vec(),
        Key::F(8) => b"\x1b[19~".to_vec(),
        Key::F(9) => b"\x1b[20~".to_vec(),
        Key::F(10) => b"\x1b[21~".to_vec(),
        Key::F(11) => b"\x1b[23~".to_vec(),
        Key::F(12) => b"\x1b[24~".to_vec(),
        _ => vec![],
    }
}

/// Drain all pending LSP intents from the editor and forward them to the LSP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
pub(crate) fn drain_lsp_intents(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
) {
    if editor.pending_lsp_requests.is_empty() {
        return;
    }
    let intents = std::mem::take(&mut editor.pending_lsp_requests);
    for intent in intents {
        let cmd = intent_to_lsp_command(intent);
        if lsp_tx.try_send(cmd).is_err() {
            warn!("LSP command channel full or closed — intent dropped");
        }
    }
}

/// Translate an editor-side `LspIntent` into a transport-layer `LspCommand`.
fn intent_to_lsp_command(intent: LspIntent) -> LspCommand {
    match intent {
        LspIntent::DidOpen {
            uri,
            language_id,
            text,
        } => LspCommand::DidOpen {
            uri,
            language_id,
            text,
        },
        LspIntent::DidChange {
            uri,
            language_id,
            text,
        } => LspCommand::DidChange {
            uri,
            language_id,
            text,
        },
        LspIntent::DidSave {
            uri,
            language_id,
            text,
        } => LspCommand::DidSave {
            uri,
            language_id,
            text,
        },
        LspIntent::DidClose { uri, language_id } => LspCommand::DidClose { uri, language_id },
        LspIntent::GotoDefinition {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::GotoDefinition {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::FindReferences {
            uri,
            language_id,
            line,
            character,
            include_declaration,
        } => LspCommand::FindReferences {
            uri,
            language_id,
            position: Position { line, character },
            include_declaration,
        },
        LspIntent::Hover {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::Hover {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::Completion {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::Completion {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::WorkspaceSymbol { language_id, query } => {
            LspCommand::WorkspaceSymbol { language_id, query }
        }
        LspIntent::DocumentSymbols { uri, language_id } => {
            LspCommand::DocumentSymbols { uri, language_id }
        }
        // Stubs: these intents are queued but the LSP client doesn't
        // handle them yet. Log and ignore until Phase 4a M5.
        LspIntent::CodeAction { .. } | LspIntent::Rename { .. } | LspIntent::Format { .. } => {
            LspCommand::DidClose {
                uri: String::new(),
                language_id: String::new(),
            }
        }
    }
}

/// Handle an event from the LSP task — update editor state or open a new buffer.
fn handle_lsp_event(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    event: LspTaskEvent,
) {
    match event {
        LspTaskEvent::ServerStarted { language_id } => {
            info!(language = %language_id, "LSP server started");
            editor.set_status(format!("[LSP] {} server started", language_id));
        }
        LspTaskEvent::ServerStartFailed { language_id, error } => {
            warn!(language = %language_id, error = %error, "LSP server failed to start");
            editor.set_status(format!("[LSP] {}: {}", language_id, error));
        }
        LspTaskEvent::ServerExited { language_id } => {
            warn!(language = %language_id, "LSP server exited");
            editor.set_status(format!("[LSP] {} server exited", language_id));
        }
        LspTaskEvent::DefinitionResult { uri: _, locations } => {
            let core_locs: Vec<LspLocation> = locations
                .into_iter()
                .map(|l| LspLocation {
                    uri: l.uri,
                    range: LspRange {
                        start_line: l.range.start.line,
                        start_character: l.range.start.character,
                        end_line: l.range.end.line,
                        end_character: l.range.end.character,
                    },
                })
                .collect();
            if let Some(other_file_loc) = editor.apply_definition_result(core_locs) {
                // Different file — open it and jump.
                open_location(editor, lsp_tx, other_file_loc);
            }
        }
        LspTaskEvent::ReferencesResult { uri: _, locations } => {
            let core_locs: Vec<LspLocation> = locations
                .into_iter()
                .map(|l| LspLocation {
                    uri: l.uri,
                    range: LspRange {
                        start_line: l.range.start.line,
                        start_character: l.range.start.character,
                        end_line: l.range.end.line,
                        end_character: l.range.end.character,
                    },
                })
                .collect();
            editor.apply_references_result(core_locs);
        }
        LspTaskEvent::HoverResult { contents, .. } => {
            editor.apply_hover_result(contents);
        }
        LspTaskEvent::DiagnosticsPublished { uri, diagnostics } => {
            let count = diagnostics.len();
            let core_diags: Vec<CoreDiagnostic> =
                diagnostics.into_iter().map(lsp_diag_to_core).collect();
            editor.diagnostics.set(uri.clone(), core_diags);
            debug!(uri = %uri, count, "diagnostics published");
            // Surface a summary in the status line so users notice new
            // problems without having to open the diagnostics buffer.
            let (e, w, _, _) = editor.diagnostics.severity_counts();
            if e + w > 0 {
                editor.set_status(format!("[LSP] {} errors, {} warnings", e, w));
            }
        }
        LspTaskEvent::ServerNotification {
            language_id,
            notification,
        } => {
            debug!(
                language = %language_id,
                method = %notification.method,
                "LSP server notification"
            );
        }
        LspTaskEvent::CompletionResult { uri: _, items, .. } => {
            let core_items: Vec<CoreCompletionItem> = items
                .into_iter()
                .map(|item| CoreCompletionItem {
                    insert_text: item.insert_text.unwrap_or_else(|| item.label.clone()),
                    label: item.label,
                    detail: item.detail,
                    kind_sigil: item.kind.sigil(),
                })
                .collect();
            editor.apply_completion_result(core_items);
        }
        // Workspace/document symbol results are only consumed by the deferred
        // AI tool flow (try_complete_deferred). If no deferred call is pending
        // they are silently dropped here.
        LspTaskEvent::WorkspaceSymbolResult { .. } => {}
        LspTaskEvent::DocumentSymbolResult { .. } => {}
        LspTaskEvent::Error { message } => {
            warn!(error = %message, "LSP error");
            editor.set_status(format!("[LSP] {}", message));
        }
    }
}

/// Check if an incoming LSP event matches a pending deferred AI tool call.
/// If so, format a structured JSON result and return it. The caller is
/// responsible for sending it via the held oneshot reply channel.
pub(crate) fn try_complete_deferred(
    event: &LspTaskEvent,
    kind: DeferredKind,
    tool_call_id: &str,
) -> Option<ToolResult> {
    match (kind, event) {
        (DeferredKind::LspDefinition, LspTaskEvent::DefinitionResult { locations, .. }) => {
            let locs: Vec<serde_json::Value> = locations
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "uri": l.uri,
                        "path": l.uri.strip_prefix("file://").unwrap_or(&l.uri),
                        "line": l.range.start.line + 1,
                        "character": l.range.start.character + 1,
                        "end_line": l.range.end.line + 1,
                        "end_character": l.range.end.character + 1,
                    })
                })
                .collect();
            let output = if locs.is_empty() {
                serde_json::json!({"locations": [], "message": "definition not found"})
            } else {
                serde_json::json!({"locations": locs})
            };
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                success: true,
                output: output.to_string(),
            })
        }
        (DeferredKind::LspReferences, LspTaskEvent::ReferencesResult { locations, .. }) => {
            let locs: Vec<serde_json::Value> = locations
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "uri": l.uri,
                        "path": l.uri.strip_prefix("file://").unwrap_or(&l.uri),
                        "line": l.range.start.line + 1,
                        "character": l.range.start.character + 1,
                    })
                })
                .collect();
            let count = locs.len();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                success: true,
                output: serde_json::json!({"count": count, "references": locs}).to_string(),
            })
        }
        (DeferredKind::LspHover, LspTaskEvent::HoverResult { contents, .. }) => {
            let output = if contents.is_empty() {
                serde_json::json!({"contents": "", "message": "no hover info"})
            } else {
                serde_json::json!({"contents": contents})
            };
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                success: true,
                output: output.to_string(),
            })
        }
        (DeferredKind::LspWorkspaceSymbol, LspTaskEvent::WorkspaceSymbolResult { symbols }) => {
            let syms: Vec<serde_json::Value> = symbols
                .iter()
                .map(|s| {
                    let mut obj = serde_json::json!({
                        "name": s.name,
                        "kind": s.kind.label(),
                        "path": s.location.uri.strip_prefix("file://").unwrap_or(&s.location.uri),
                        "line": s.location.range.start.line + 1,
                        "character": s.location.range.start.character + 1,
                    });
                    if let Some(ref cn) = s.container_name {
                        obj["container_name"] = serde_json::json!(cn);
                    }
                    obj
                })
                .collect();
            let count = syms.len();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                success: true,
                output: serde_json::json!({"count": count, "symbols": syms}).to_string(),
            })
        }
        (DeferredKind::LspDocumentSymbols, LspTaskEvent::DocumentSymbolResult { symbols, .. }) => {
            fn format_doc_symbol(s: &mae_lsp::protocol::DocumentSymbol) -> serde_json::Value {
                let mut obj = serde_json::json!({
                    "name": s.name,
                    "kind": s.kind.label(),
                    "line": s.range.start.line + 1,
                    "end_line": s.range.end.line + 1,
                });
                if let Some(ref d) = s.detail {
                    obj["detail"] = serde_json::json!(d);
                }
                if !s.children.is_empty() {
                    obj["children"] = serde_json::Value::Array(
                        s.children.iter().map(format_doc_symbol).collect(),
                    );
                }
                obj
            }
            let syms: Vec<serde_json::Value> = symbols.iter().map(format_doc_symbol).collect();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                success: true,
                output: serde_json::json!({"symbols": syms}).to_string(),
            })
        }
        // Also handle LSP errors while a deferred call is pending
        (_, LspTaskEvent::Error { message }) => Some(ToolResult {
            tool_call_id: tool_call_id.to_string(),
            success: false,
            output: format!("LSP error: {}", message),
        }),
        _ => None,
    }
}

/// Strip `file://` prefix from a URI to get a filesystem path.
fn uri_to_path(uri: &str) -> Option<&str> {
    uri.strip_prefix("file://")
}

/// Open the buffer at `loc.uri` (if not already open) and jump the cursor to
/// `loc.range.start`. After opening we also forward a fresh didOpen intent
/// so the newly-focused buffer is known to the language server.
fn open_location(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    loc: LspLocation,
) {
    let Some(path) = uri_to_path(&loc.uri) else {
        editor.set_status(format!("[LSP] cannot open non-file URI: {}", loc.uri));
        return;
    };

    // If the buffer is already loaded, just switch to it.
    let existing = editor
        .buffers
        .iter()
        .position(|b| b.file_path().map(|p| p.to_string_lossy()) == Some(path.into()));

    match existing {
        Some(idx) => {
            editor.switch_to_buffer(idx);
        }
        None => {
            // open_file queues a didOpen via file_ops
            editor.open_file(path);
        }
    }

    // Place the cursor.
    let idx = editor.active_buffer_idx();
    let line_count = editor.buffers[idx].line_count();
    let target_row = (loc.range.start_line as usize).min(line_count.saturating_sub(1));
    let target_col = loc.range.start_character as usize;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = target_row;
    win.cursor_col = target_col;
    win.clamp_cursor(&editor.buffers[idx]);

    // Drain any intents produced by open_file.
    drain_lsp_intents(editor, lsp_tx);
    editor.set_status(format!(
        "[LSP] opened {} at {}:{}",
        path,
        target_row + 1,
        target_col + 1
    ));
}

/// Drain all pending DAP intents from the editor and forward them to the DAP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
fn drain_dap_intents(editor: &mut Editor, dap_tx: &tokio::sync::mpsc::Sender<DapCommand>) {
    if editor.pending_dap_intents.is_empty() {
        return;
    }
    let intents = std::mem::take(&mut editor.pending_dap_intents);
    for intent in intents {
        let cmd = intent_to_dap_command(intent);
        let kind = dap_command_name(&cmd);
        if dap_tx.try_send(cmd).is_err() {
            warn!(kind, "DAP command channel full or closed — intent dropped");
        }
    }
}

/// Short name of a DAP command for logging — used only for diagnostics so
/// a dropped intent is attributable to a specific operation.
fn dap_command_name(cmd: &DapCommand) -> &'static str {
    match cmd {
        DapCommand::StartSession { .. } => "start-session",
        DapCommand::SetBreakpoints { .. } => "set-breakpoints",
        DapCommand::Continue { .. } => "continue",
        DapCommand::Next { .. } => "next",
        DapCommand::StepIn { .. } => "step-in",
        DapCommand::StepOut { .. } => "step-out",
        DapCommand::RefreshThreadsAndStack { .. } => "refresh-threads-and-stack",
        DapCommand::RequestScopes { .. } => "request-scopes",
        DapCommand::RequestVariables { .. } => "request-variables",
        DapCommand::Terminate => "terminate",
        DapCommand::Disconnect { .. } => "disconnect",
        DapCommand::Shutdown => "shutdown",
    }
}

/// Translate an editor-side `DapIntent` into a transport-layer `DapCommand`.
/// The core crate has no `mae-dap` dependency, so the binary performs the crosswalk.
fn intent_to_dap_command(intent: DapIntent) -> DapCommand {
    match intent {
        DapIntent::StartSession {
            spawn,
            launch_args,
            attach,
        } => DapCommand::StartSession {
            config: DapServerConfig {
                command: spawn.command,
                args: spawn.args,
                adapter_id: spawn.adapter_id,
            },
            launch_args,
            attach,
        },
        DapIntent::SetBreakpoints { source_path, lines } => DapCommand::SetBreakpoints {
            source_path,
            breakpoints: lines
                .into_iter()
                .map(|line| SourceBreakpoint {
                    line,
                    condition: None,
                    hit_condition: None,
                })
                .collect(),
        },
        DapIntent::Continue { thread_id } => DapCommand::Continue { thread_id },
        DapIntent::Next { thread_id } => DapCommand::Next { thread_id },
        DapIntent::StepIn { thread_id } => DapCommand::StepIn { thread_id },
        DapIntent::StepOut { thread_id } => DapCommand::StepOut { thread_id },
        DapIntent::RefreshThreadsAndStack { thread_id } => {
            DapCommand::RefreshThreadsAndStack { thread_id }
        }
        DapIntent::RequestScopes { frame_id } => DapCommand::RequestScopes { frame_id },
        DapIntent::RequestVariables {
            scope_name,
            variables_reference,
        } => DapCommand::RequestVariables {
            scope_name,
            variables_reference,
        },
        DapIntent::Terminate => DapCommand::Terminate,
        DapIntent::Disconnect { terminate_debuggee } => {
            DapCommand::Disconnect { terminate_debuggee }
        }
    }
}

/// Handle an event from the DAP task — update editor state via `apply_dap_*`.
fn handle_dap_event(editor: &mut Editor, event: DapTaskEvent) {
    match event {
        DapTaskEvent::SessionStarted {
            adapter_id,
            capabilities: _,
        } => {
            info!(adapter = %adapter_id, "DAP session started");
            editor.apply_dap_session_started(adapter_id);
        }
        DapTaskEvent::SessionStartFailed { error } => {
            warn!(error = %error, "DAP session start failed");
            editor.apply_dap_session_start_failed(error);
        }
        DapTaskEvent::Stopped {
            reason,
            thread_id,
            text,
        } => {
            debug!(reason = %reason, thread_id = ?thread_id, "DAP stopped");
            editor.apply_dap_stopped(reason, thread_id, text);
        }
        DapTaskEvent::Continued {
            thread_id,
            all_threads,
        } => {
            editor.apply_dap_continued(thread_id, all_threads);
        }
        DapTaskEvent::ThreadEvent {
            reason: _,
            thread_id: _,
        } => {
            // Drive a thread-list refresh on any thread start/exit so the UI
            // stays in sync with reality.
            editor.dap_refresh();
        }
        DapTaskEvent::Output { category, output } => {
            editor.apply_dap_output(category, output);
        }
        DapTaskEvent::Terminated => {
            editor.apply_dap_terminated();
        }
        DapTaskEvent::AdapterExited => {
            editor.apply_dap_adapter_exited();
        }
        DapTaskEvent::Error { message } => {
            warn!(error = %message, "DAP error");
            editor.apply_dap_error(message);
        }
        DapTaskEvent::ThreadsResult { threads } => {
            let core_threads: Vec<(i64, String)> =
                threads.into_iter().map(|t| (t.id, t.name)).collect();
            editor.apply_dap_threads(core_threads);
        }
        DapTaskEvent::StackTraceResult { thread_id, frames } => {
            let core_frames: Vec<(i64, String, Option<String>, i64, i64)> = frames
                .into_iter()
                .map(|f| {
                    let src = f.source.and_then(|s| s.path.or(s.name));
                    (f.id, f.name, src, f.line, f.column)
                })
                .collect();
            editor.apply_dap_stack_trace(thread_id, core_frames);
        }
        DapTaskEvent::ScopesResult { frame_id, scopes } => {
            let core_scopes: Vec<(String, i64, bool)> = scopes
                .into_iter()
                .map(|s| (s.name, s.variables_reference, s.expensive))
                .collect();
            editor.apply_dap_scopes(frame_id, core_scopes);
        }
        DapTaskEvent::VariablesResult {
            scope_name,
            variables,
        } => {
            let core_vars: Vec<(String, String, Option<String>, i64)> = variables
                .into_iter()
                .map(|v| (v.name, v.value, v.type_field, v.variables_reference))
                .collect();
            editor.apply_dap_variables(scope_name, core_vars);
        }
        DapTaskEvent::BreakpointsSet {
            source_path,
            breakpoints,
        } => {
            let entries: Vec<(i64, bool, i64)> = breakpoints
                .into_iter()
                .filter_map(|b| b.line.map(|line| (b.id.unwrap_or(0), b.verified, line)))
                .collect();
            editor.apply_dap_breakpoints_set(source_path, entries);
        }
    }
}

// ---------------------------------------------------------------------------
// GUI event loop (Phase 8 M2)
// ---------------------------------------------------------------------------

/// Run the GUI event loop using winit's `pump_app_events()`.
///
/// This integrates winit into the existing tokio `current_thread` runtime:
/// each iteration pumps winit events (window/keyboard/resize), then yields
/// to tokio::select! to drain AI/LSP/DAP/MCP channels.
///
/// Platform notes:
/// - Linux/Windows: pump_app_events works well.
/// - macOS: documented as "best effort" — full macOS support is a future milestone.
#[cfg(feature = "gui")]
#[allow(clippy::too_many_arguments)]
async fn run_gui_loop(
    mut editor: Editor,
    mut scheme: SchemeRuntime,
    mut ai_event_rx: tokio::sync::mpsc::Receiver<AiEvent>,
    ai_command_tx: Option<tokio::sync::mpsc::Sender<AiCommand>>,
    mut lsp_event_rx: tokio::sync::mpsc::Receiver<LspTaskEvent>,
    lsp_command_tx: tokio::sync::mpsc::Sender<LspCommand>,
    mut dap_event_rx: tokio::sync::mpsc::Receiver<DapTaskEvent>,
    dap_command_tx: tokio::sync::mpsc::Sender<DapCommand>,
    mut mcp_tool_rx: tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    mcp_socket_path: String,
    all_tools: Vec<mae_ai::ToolDefinition>,
    permission_policy: mae_ai::PermissionPolicy,
    app_config: config::Config,
) -> io::Result<()> {
    use mae_gui::GuiRenderer;
    use mae_renderer::Renderer;
    use std::time::Duration;
    use winit::event_loop::EventLoop;
    use winit::platform::pump_events::EventLoopExtPumpEvents;

    let mut renderer = GuiRenderer::new();
    renderer.set_font_config(
        app_config.editor.font_family.clone(),
        app_config.editor.font_size,
    );
    editor.renderer_name = "gui".to_string();
    let mut event_loop = EventLoop::new().map_err(|e| io::Error::other(e.to_string()))?;

    // State shared between the winit callback and the outer loop.
    let mut should_exit = false;
    let mut pending_keys: Vec<KeyPress> = Vec::new();
    let mut shell_terminals: std::collections::HashMap<usize, mae_shell::ShellTerminal> =
        std::collections::HashMap::new();
    let mut shell_last_dims: std::collections::HashMap<usize, (u16, u16)> =
        std::collections::HashMap::new();
    let mut shell_pending_keys: Vec<KeyPress> = Vec::new();
    let mut deferred_ai_reply: ai_event_handler::DeferredAiReply = None;
    let mut deferred_mcp_reply: ai_event_handler::DeferredMcpReply = Vec::new();
    let mut last_mcp_activity: Option<tokio::time::Instant> = None;

    // Track modifier state across winit events (winit delivers modifiers
    // separately from key events).
    let mut ctrl_held = false;
    let mut alt_held = false;
    let mut mcp_cancelled = false;
    let mut dirty = true;
    let mut cursor_x: f64 = 0.0;
    let mut cursor_y: f64 = 0.0;
    let mut last_health_check = tokio::time::Instant::now();
    // Accumulator for fractional pixel scroll (Wayland touchpad sends small deltas).
    let mut scroll_accumulator: f64 = 0.0;

    info!("entering GUI event loop");

    loop {
        ai_event_handler::timeout_deferred_reply(&mut editor, &mut deferred_ai_reply);
        ai_event_handler::timeout_deferred_mcp_reply(&mut editor, &mut deferred_mcp_reply);

        // Periodic health check (~30s): scan for zombie shells, stale locks.
        if last_health_check.elapsed() > std::time::Duration::from_secs(30) {
            shell_lifecycle::health_check(
                &mut editor,
                &mut shell_terminals,
                deferred_ai_reply.is_some(),
                last_mcp_activity.is_some() || !deferred_mcp_reply.is_empty(),
            );
            last_health_check = tokio::time::Instant::now();
        }

        // --- Pre-render bookkeeping (same as terminal loop) ---
        editor.clamp_all_cursors();
        let viewport_height = renderer.viewport_height()?;
        editor.viewport_height = viewport_height;
        editor
            .window_mgr
            .focused_window_mut()
            .ensure_scroll(viewport_height);

        // --- Pump winit events (non-blocking, ~16ms timeout for 60fps) ---
        let pump_status = event_loop.pump_app_events(
            Some(Duration::from_millis(16)),
            &mut WinitCallback {
                renderer: &mut renderer,
                editor: &mut editor,
                scheme: &mut scheme,
                pending_keys: &mut pending_keys,
                shell_terminals: &mut shell_terminals,
                shell_pending_keys: &mut shell_pending_keys,
                ai_command_tx: &ai_command_tx,
                should_exit: &mut should_exit,
                ctrl_held: &mut ctrl_held,
                alt_held: &mut alt_held,
                mcp_cancelled: &mut mcp_cancelled,
                dirty: &mut dirty,
                cursor_x: &mut cursor_x,
                cursor_y: &mut cursor_y,
                scroll_accumulator: &mut scroll_accumulator,
            },
        );

        if should_exit
            || matches!(
                pump_status,
                winit::platform::pump_events::PumpStatus::Exit(..)
            )
        {
            break;
        }

        if !editor.running {
            info!("editor shutting down (GUI)");
            if let Some(ref tx) = ai_command_tx {
                let _ = tx.try_send(AiCommand::Shutdown);
            }
            let _ = lsp_command_tx.try_send(LspCommand::Shutdown);
            let _ = dap_command_tx.try_send(DapCommand::Shutdown);
            break;
        }

        // --- Drain editor intents (LSP / DAP / agents / shells) ---
        drain_lsp_intents(&mut editor, &lsp_command_tx);
        drain_dap_intents(&mut editor, &dap_command_tx);

        shell_lifecycle::drain_agent_setup(&mut editor);
        shell_lifecycle::spawn_pending_shells(
            &mut editor,
            &mut shell_terminals,
            &mut shell_last_dims,
            &renderer,
            &mcp_socket_path,
            &app_config,
        );
        shell_lifecycle::resize_shells(&editor, &renderer, &shell_terminals, &mut shell_last_dims);
        shell_lifecycle::manage_shell_lifecycle(&mut editor, &mut shell_terminals);

        // --- Poll async channels (AI/LSP/DAP/MCP) with short timeout ---
        let has_shells = !shell_terminals.is_empty();
        let shell_tick = async {
            if has_shells {
                tokio::time::sleep(Duration::from_millis(33)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        // Clear MCP cancel flag if set by WinitCallback during pump.
        if mcp_cancelled {
            last_mcp_activity = None;
            mcp_cancelled = false;
        }

        let mcp_idle_tick = async {
            if last_mcp_activity.is_some() {
                tokio::time::sleep(Duration::from_millis(500)).await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        // Poll async channels; set dirty when state changes so we redraw.
        tokio::select! {
            biased;

            Some(ai_event) = ai_event_rx.recv() => {
                ai_event_handler::handle_ai_event(
                    &mut editor, ai_event, &all_tools, &permission_policy,
                    &mut deferred_ai_reply, &lsp_command_tx,
                );
                dirty = true;
            }
            Some(lsp_event) = lsp_event_rx.recv() => {
                ai_event_handler::try_resolve_deferred(&mut editor, &lsp_event, &mut deferred_ai_reply);
                if ai_event_handler::try_resolve_deferred_mcp(&lsp_event, &mut deferred_mcp_reply) {
                    last_mcp_activity = Some(tokio::time::Instant::now());
                }
                handle_lsp_event(&mut editor, &lsp_command_tx, lsp_event);
                dirty = true;
            }
            Some(dap_event) = dap_event_rx.recv() => {
                handle_dap_event(&mut editor, dap_event);
                dirty = true;
            }
            _ = mcp_idle_tick => {
                let was_locked = editor.input_lock;
                if let Some(ts) = last_mcp_activity {
                    if ts.elapsed() > Duration::from_millis(500)
                        && deferred_mcp_reply.is_empty()
                    {
                        if editor.input_lock == mae_core::InputLock::McpBusy {
                            editor.set_status("MCP: input unlocked");
                        }
                        editor.input_lock = mae_core::InputLock::None;
                        last_mcp_activity = None;
                    }
                }
                if was_locked != editor.input_lock {
                    dirty = true;
                }
            }
            Some(mcp_req) = mcp_tool_rx.recv() => {
                editor.input_lock = mae_core::InputLock::McpBusy;
                last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    &mut editor, mcp_req, &all_tools, &permission_policy,
                    &lsp_command_tx, &mut deferred_mcp_reply,
                );
                // Immediate tools: clear lock right away if no deferred calls pending.
                if immediate && deferred_mcp_reply.is_empty() {
                    editor.input_lock = mae_core::InputLock::None;
                    last_mcp_activity = None;
                }
                dirty = true;
            }
            _ = shell_tick => {
                // Always redraw when shells are active. The PTY I/O thread
                // writes directly to the Term grid via FairMutex — only
                // metadata events (title/bell/exit) go through the mpsc
                // channel that increments the generation counter.  Programs
                // like htop produce continuous output without triggering
                // ShellEvent, so we must redraw unconditionally at ~30fps.
                if has_shells {
                    dirty = true;
                }
            }
            // Fallback: don't block forever — return to pump_app_events for winit events.
            // Without this, when no async channels have data AND no shells are active,
            // ALL select branches block forever and the GUI becomes unresponsive.
            // 16ms matches the vsync cadence set by pump_app_events(16ms).
            _ = tokio::time::sleep(Duration::from_millis(16)) => {}
        }

        // Only request a redraw when something actually changed.
        if dirty {
            renderer.request_redraw();
            dirty = false;
        }
    }

    let _ = std::fs::remove_file(&mcp_socket_path);
    renderer.cleanup()?;
    info!("mae (GUI) exited cleanly");
    Ok(())
}

/// Winit callback struct used with `pump_app_events()`.
///
/// Borrows all state from the outer loop. Handles window creation on `Resumed`,
/// keyboard input translation, resize, and close-requested.
#[cfg(feature = "gui")]
struct WinitCallback<'a> {
    renderer: &'a mut mae_gui::GuiRenderer,
    editor: &'a mut Editor,
    scheme: &'a mut SchemeRuntime,
    pending_keys: &'a mut Vec<KeyPress>,
    shell_terminals: &'a mut std::collections::HashMap<usize, mae_shell::ShellTerminal>,
    shell_pending_keys: &'a mut Vec<KeyPress>,
    ai_command_tx: &'a Option<tokio::sync::mpsc::Sender<AiCommand>>,
    should_exit: &'a mut bool,
    ctrl_held: &'a mut bool,
    alt_held: &'a mut bool,
    mcp_cancelled: &'a mut bool,
    dirty: &'a mut bool,
    cursor_x: &'a mut f64,
    cursor_y: &'a mut f64,
    scroll_accumulator: &'a mut f64,
}

#[cfg(feature = "gui")]
impl winit::application::ApplicationHandler for WinitCallback<'_> {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.renderer.window().is_none() {
            if let Err(e) = self.renderer.init_window(event_loop) {
                error!(error = %e, "failed to init GUI window");
                *self.should_exit = true;
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        use winit::event::WindowEvent;

        match event {
            WindowEvent::CloseRequested => {
                *self.should_exit = true;
                *self.dirty = true;
            }
            WindowEvent::Resized(size) => {
                self.renderer.handle_resize(size.width, size.height);
                *self.dirty = true;
            }
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                *self.ctrl_held = state.control_key();
                *self.alt_held = state.alt_key();
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == winit::event::ElementState::Pressed =>
            {
                *self.dirty = true;
                if let Some(mae_core::InputEvent::Key(kp)) =
                    mae_gui::winit_event_to_input(&event, *self.ctrl_held, *self.alt_held)
                {
                    // Input lock: allow Esc/Ctrl-C to cancel, shell input
                    // to pass through, and discard everything else.
                    if self.editor.input_lock != mae_core::InputLock::None {
                        if kp.key == mae_core::Key::Escape
                            || (kp.key == mae_core::Key::Char('c') && kp.ctrl)
                        {
                            self.editor.input_lock = mae_core::InputLock::None;
                            self.editor.ai_streaming = false;
                            *self.mcp_cancelled = true;
                            if let Some(tx) = self.ai_command_tx {
                                let _ = tx.try_send(AiCommand::Cancel);
                            }
                            self.editor.set_status("AI operation cancelled");
                        } else if self.editor.mode == Mode::ShellInsert {
                            // Allow shell input even during AI/MCP lock.
                            let ct_event = key_handling::keypress_to_crossterm(&kp);
                            handle_shell_key(
                                self.editor,
                                ct_event,
                                self.shell_terminals,
                                self.shell_pending_keys,
                            );
                        }
                    } else if self.editor.mode == Mode::ShellInsert {
                        let ct_event = key_handling::keypress_to_crossterm(&kp);
                        handle_shell_key(
                            self.editor,
                            ct_event,
                            self.shell_terminals,
                            self.shell_pending_keys,
                        );
                    } else {
                        key_handling::handle_key_from_keypress(
                            self.editor,
                            self.scheme,
                            kp,
                            self.pending_keys,
                            self.ai_command_tx,
                        );
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                *self.cursor_x = position.x;
                *self.cursor_y = position.y;
                // Don't set dirty — cursor movement alone doesn't need a redraw.
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button,
                ..
            } => {
                if let Some(mae_button) = mae_gui::winit_mouse_button(&button) {
                    // Convert pixel position to cell coordinates.
                    let (cell_w, cell_h) = self.renderer.cell_dimensions();
                    if cell_w > 0.0 && cell_h > 0.0 {
                        let col = (*self.cursor_x / cell_w as f64) as u16;
                        let row = (*self.cursor_y / cell_h as f64) as u16;
                        self.editor
                            .handle_mouse_click(row as usize, col as usize, mae_button);
                        *self.dirty = true;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as i16,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        // Accumulate fractional pixel deltas (Wayland touchpads
                        // send small values per event that would truncate to 0).
                        *self.scroll_accumulator += pos.y;
                        let whole_lines = (*self.scroll_accumulator / 20.0) as i16;
                        if whole_lines != 0 {
                            *self.scroll_accumulator -= whole_lines as f64 * 20.0;
                        }
                        whole_lines
                    }
                };
                if lines != 0 {
                    self.editor.handle_mouse_scroll(lines);
                    *self.dirty = true;
                }
            }
            WindowEvent::RedrawRequested => {
                let frame_start = std::time::Instant::now();
                if let Err(e) = self.renderer.render(self.editor, self.shell_terminals) {
                    warn!(error = %e, "GUI render error");
                }
                let frame_elapsed = frame_start.elapsed().as_micros() as u64;
                self.editor.perf_stats.record_frame(frame_elapsed);
                if self.editor.debug_mode {
                    self.editor.perf_stats.sample_process_stats();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        // Intentionally empty — redraws are now driven by the dirty flag
        // in the outer loop, not unconditionally every pump cycle.
    }
}

/// Translate an `mae_lsp::Diagnostic` into the core representation.
/// The core crate has no LSP dependency, so the binary performs the crosswalk.
fn lsp_diag_to_core(d: LspDiagnostic) -> CoreDiagnostic {
    CoreDiagnostic {
        line: d.range.start.line,
        col_start: d.range.start.character,
        col_end: d.range.end.character,
        end_line: d.range.end.line,
        severity: match d.severity {
            DiagnosticSeverity::Error => CoreSeverity::Error,
            DiagnosticSeverity::Warning => CoreSeverity::Warning,
            DiagnosticSeverity::Information => CoreSeverity::Information,
            DiagnosticSeverity::Hint => CoreSeverity::Hint,
        },
        message: d.message,
        source: d.source,
        code: d.code,
    }
}
