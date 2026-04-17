mod bootstrap;
mod config;
mod key_handling;

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
    DiagnosticSeverity as CoreSeverity, Editor, KeyPress, LspIntent, LspLocation, LspRange,
};
use mae_dap::{DapCommand, DapServerConfig, DapTaskEvent, SourceBreakpoint};
use mae_lsp::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, LspCommand, LspTaskEvent, Position,
};
use mae_renderer::TerminalRenderer;
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

    let mut editor = if args.len() > 1 {
        let path = &args[1];
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

    // Apply editor preferences from config file.
    {
        let cfg = config::load_config();
        if let Some(ref theme) = cfg.editor.theme {
            editor.set_theme_by_name(theme);
        }
        if let Some(ref art) = cfg.editor.splash_art {
            editor.splash_art = Some(art.clone());
        }
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
    let permission_policy = mae_ai::PermissionPolicy::default();

    let mut renderer = TerminalRenderer::new()?;
    let mut event_stream = EventStream::new();
    let mut pending_keys: Vec<KeyPress> = Vec::new();

    // When an AI tool call is deferred (e.g. LSP request), we hold the
    // reply channel here until the async result arrives.
    let mut deferred_ai_reply: Option<(
        DeferredKind,
        String, // tool_call_id
        tokio::sync::oneshot::Sender<ToolResult>,
    )> = None;

    loop {
        // Update viewport dimensions and scroll before rendering
        let viewport_height = renderer.viewport_height()?;
        editor.viewport_height = viewport_height;
        editor
            .window_mgr
            .focused_window_mut()
            .ensure_scroll(viewport_height);

        // Horizontal scroll: compute text width from focused window's actual area
        {
            let (term_w, term_h) = renderer.terminal_size()?;
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

        renderer.render(&mut editor)?;

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

        // Async event loop: select! over keyboard + AI channels
        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut editor, &mut scheme, key, &mut pending_keys, &ai_command_tx);
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Terminal resized — next loop iteration will re-render
                    }
                    Some(Err(e)) => {
                        editor.set_status(format!("Input error: {}", e));
                    }
                    None => break,
                    _ => {}
                }
            }
            Some(ai_event) = ai_event_rx.recv() => {
                match ai_event {
                    AiEvent::ToolCallRequest { call, reply } => {
                        debug!(tool = %call.name, call_id = %call.id, "executing tool call");

                        // Push tool call to conversation buffer
                        if let Some(conv) = find_conversation_buffer_mut(&mut editor) {
                            conv.push_tool_call(&call.name);
                        }

                        let exec_result = execute_tool(
                            &mut editor, &call, &all_tools, &permission_policy,
                        );

                        match exec_result {
                            ExecuteResult::Immediate(result) => {
                                debug!(tool = %call.name, success = result.success, "tool call complete");
                                if let Some(conv) = find_conversation_buffer_mut(&mut editor) {
                                    conv.push_tool_result(result.success, &result.output);
                                }
                                if reply.send(result).is_err() {
                                    warn!(tool = %call.name, "tool result channel closed — AI session may have been cancelled");
                                }
                            }
                            ExecuteResult::Deferred { tool_call_id, kind } => {
                                debug!(tool = %call.name, ?kind, "tool call deferred — awaiting async result");
                                // Drain the LSP intent we just queued so it's
                                // sent to the LSP task immediately.
                                drain_lsp_intents(&mut editor, &lsp_command_tx);
                                deferred_ai_reply = Some((kind, tool_call_id, reply));
                            }
                        }
                    }
                    AiEvent::TextResponse(text) => {
                        // Route to conversation buffer if one exists
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
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
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
                            conv_buf.append_streaming_chunk(&text);
                        }
                    }
                    AiEvent::SessionComplete(_text) => {
                        info!("AI session complete");
                        // Don't push text here — TextResponse already did that.
                        // Just mark streaming as done.
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
                            conv_buf.streaming = false;
                            conv_buf.streaming_start = None;
                        }
                        editor.set_status("[AI] Done");
                    }
                    AiEvent::Error(msg) => {
                        error!(error = %msg, "AI error");
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
                            conv_buf.push_system(format!("Error: {}", msg));
                            conv_buf.streaming = false;
                            conv_buf.streaming_start = None;
                        }
                        editor.set_status(format!("[AI error] {}", msg));
                    }
                    AiEvent::CostUpdate { session_usd, last_call_usd, tokens_in, tokens_out } => {
                        editor.ai_session_cost_usd = session_usd;
                        editor.ai_session_tokens_in = tokens_in;
                        editor.ai_session_tokens_out = tokens_out;
                        debug!(
                            session_usd,
                            last_call_usd,
                            tokens_in,
                            tokens_out,
                            "AI cost update"
                        );
                    }
                    AiEvent::BudgetWarning { session_usd, threshold_usd } => {
                        let msg = format!(
                            "AI budget warning: session spend ${:.4} crossed ${:.2} threshold. \
                             Hard cap (if set) will abort the next turn.",
                            session_usd, threshold_usd
                        );
                        warn!(session_usd, threshold_usd, "AI budget threshold crossed");
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
                            conv_buf.push_system(msg.clone());
                        }
                        editor.set_status(msg);
                    }
                    AiEvent::BudgetExceeded { session_usd, cap_usd } => {
                        let msg = format!(
                            "AI budget exceeded: session spend ${:.4} reached cap ${:.2}. \
                             Raise `ai.budget.session_hard_cap_usd` in config.toml or restart \
                             the editor to reset.",
                            session_usd, cap_usd
                        );
                        error!(session_usd, cap_usd, "AI session hard cap reached");
                        if let Some(conv_buf) = find_conversation_buffer_mut(&mut editor) {
                            conv_buf.push_system(msg.clone());
                            conv_buf.streaming = false;
                            conv_buf.streaming_start = None;
                        }
                        editor.set_status(msg);
                    }
                }
            }
            Some(lsp_event) = lsp_event_rx.recv() => {
                // Check if this event completes a deferred AI tool call.
                if let Some((kind, ref tool_call_id, _)) = deferred_ai_reply {
                    if let Some(result) = try_complete_deferred(&lsp_event, kind, tool_call_id) {
                        let (_, _, reply) = deferred_ai_reply.take().unwrap();
                        debug!(tool_call_id = %result.tool_call_id, "deferred tool call completed");
                        if let Some(conv) = find_conversation_buffer_mut(&mut editor) {
                            conv.push_tool_result(result.success, &result.output);
                        }
                        if reply.send(result).is_err() {
                            warn!("deferred tool result channel closed");
                        }
                        // Still let the normal handler apply editor-side effects
                        // (e.g. jumping cursor for definition).
                    }
                }
                handle_lsp_event(&mut editor, &lsp_command_tx, lsp_event);
            }
            Some(dap_event) = dap_event_rx.recv() => {
                handle_dap_event(&mut editor, dap_event);
            }
        }
    }

    renderer.cleanup()?;
    info!("mae exited cleanly");
    Ok(())
}

/// Drain all pending LSP intents from the editor and forward them to the LSP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
fn drain_lsp_intents(editor: &mut Editor, lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>) {
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
        LspTaskEvent::Error { message } => {
            warn!(error = %message, "LSP error");
            editor.set_status(format!("[LSP] {}", message));
        }
    }
}

/// Check if an incoming LSP event matches a pending deferred AI tool call.
/// If so, format a structured JSON result and return it. The caller is
/// responsible for sending it via the held oneshot reply channel.
fn try_complete_deferred(
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
