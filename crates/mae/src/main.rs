use std::io;
use std::panic;
use std::path::PathBuf;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use mae_ai::{
    ai_specific_tools, execute_tool, tools_from_registry, AgentSession, AiCommand, AiEvent,
    ClaudeProvider, OpenAiProvider, PermissionPolicy, ProviderConfig,
};
use mae_core::{Buffer, CommandSource, Editor, Key, KeyPress, LookupResult, Mode};
use mae_renderer::TerminalRenderer;
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

    let mut editor = if args.len() > 1 {
        let path = &args[1];
        match Buffer::from_file(std::path::Path::new(path)) {
            Ok(buf) => {
                info!(path, "opened file from CLI argument");
                Editor::with_buffer(buf)
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

    // Build tool list for AI executor (used when handling tool call requests)
    let all_tools = {
        let mut tools = tools_from_registry(&editor.commands);
        tools.extend(ai_specific_tools());
        tools
    };
    let permission_policy = PermissionPolicy::default();

    let mut renderer = TerminalRenderer::new()?;
    let mut event_stream = EventStream::new();
    let mut pending_keys: Vec<KeyPress> = Vec::new();

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
                let gutter_w = mae_renderer::gutter_width(buf.line_count());
                let text_w = inner_w.saturating_sub(gutter_w);
                editor
                    .window_mgr
                    .focused_window_mut()
                    .ensure_scroll_horizontal(text_w);
            }
        }

        renderer.render(&editor)?;

        if !editor.running {
            info!("editor shutting down");
            if let Some(ref tx) = ai_command_tx {
                if tx.try_send(AiCommand::Shutdown).is_err() {
                    warn!("failed to send shutdown to AI session (channel closed)");
                }
            }
            break;
        }

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

                        let result = execute_tool(
                            &mut editor, &call, &all_tools, &permission_policy,
                        );
                        debug!(tool = %call.name, success = result.success, "tool call complete");

                        // Push tool result to conversation buffer
                        if let Some(conv) = find_conversation_buffer_mut(&mut editor) {
                            conv.push_tool_result(result.success, &result.output);
                        }

                        if reply.send(result).is_err() {
                            warn!(tool = %call.name, "tool result channel closed — AI session may have been cancelled");
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
                }
            }
            // Phase 4+: LSP channel
            // msg = lsp_rx.recv() => { handle_lsp_response(&mut editor, msg); }
        }
    }

    renderer.cleanup()?;
    info!("mae exited cleanly");
    Ok(())
}

/// Initialize structured logging with two outputs:
///
/// 1. **stderr** — newline-delimited JSON for container log aggregation
///    (`docker logs`, `kubectl logs`, `journalctl`, Datadog, etc.)
/// 2. **In-editor MessageLog** — ring buffer viewable via `:messages`
///
/// The TUI owns stdout; logs must never interfere with it.
///
/// Control via MAE_LOG env var (falls back to RUST_LOG):
///   MAE_LOG=info        — startup/shutdown, AI events, file ops
///   MAE_LOG=debug       — command dispatch, scheme eval, key sequences
///   MAE_LOG=mae=trace   — full trace including per-key events
///   (default)           — warn (only errors and warnings)
fn init_logging(log_handle: mae_core::MessageLogHandle) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_env("MAE_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    let json_layer = fmt::layer()
        .with_writer(io::stderr)
        .json()
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(fmt::format::FmtSpan::CLOSE);

    let editor_layer = EditorLogLayer { handle: log_handle };

    tracing_subscriber::registry()
        .with(filter)
        .with(json_layer)
        .with(editor_layer)
        .init();
}

/// Tracing layer that captures events into the in-editor MessageLog.
/// This makes log entries viewable via `:messages` without requiring
/// external log tooling — the Emacs `*Messages*` pattern.
struct EditorLogLayer {
    handle: mae_core::MessageLogHandle,
}

impl<S> tracing_subscriber::Layer<S> for EditorLogLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = match *event.metadata().level() {
            tracing::Level::TRACE => mae_core::MessageLevel::Trace,
            tracing::Level::DEBUG => mae_core::MessageLevel::Debug,
            tracing::Level::INFO => mae_core::MessageLevel::Info,
            tracing::Level::WARN => mae_core::MessageLevel::Warn,
            tracing::Level::ERROR => mae_core::MessageLevel::Error,
        };

        // Extract the message field from the event
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);

        let target = event.metadata().target();
        self.handle.push(level, target, visitor.0);
    }
}

/// Visitor that extracts the "message" field from a tracing event.
struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{:?}", value);
            // Strip surrounding quotes from debug format
            if self.0.starts_with('"') && self.0.ends_with('"') {
                self.0 = self.0[1..self.0.len() - 1].to_string();
            }
        } else if !self.0.is_empty() {
            self.0.push_str(&format!(" {}={:?}", field.name(), value));
        } else {
            self.0 = format!("{}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        } else if !self.0.is_empty() {
            self.0.push_str(&format!(" {}={}", field.name(), value));
        } else {
            self.0 = format!("{}={}", field.name(), value);
        }
    }
}

/// Set up the AI agent session if an API key is configured.
/// Returns (event_receiver, command_sender).
fn setup_ai(
    editor: &Editor,
) -> (
    tokio::sync::mpsc::Receiver<AiEvent>,
    Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AiEvent>(32);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AiCommand>(8);

    let config = load_ai_config();

    if let Some(config) = config {
        let provider_name = config.provider_type.clone();
        info!(provider = %provider_name, model = %config.model, "initializing AI provider");
        let provider: Box<dyn mae_ai::AgentProvider> = match provider_name.as_str() {
            "openai" => Box::new(OpenAiProvider::new(config)),
            _ => Box::new(ClaudeProvider::new(config)), // default to Claude
        };

        let tools = {
            let mut t = tools_from_registry(&editor.commands);
            t.extend(ai_specific_tools());
            t
        };

        let session = AgentSession::new(provider, tools, build_system_prompt(), event_tx, cmd_rx);

        tokio::spawn(session.run());

        (event_rx, Some(cmd_tx))
    } else {
        // No AI configured — event channel exists but nothing sends to it
        (event_rx, None)
    }
}

fn load_ai_config() -> Option<ProviderConfig> {
    // Check for provider type
    let provider_type = std::env::var("MAE_AI_PROVIDER").unwrap_or_else(|_| "claude".into());

    let api_key = match provider_type.as_str() {
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        _ => std::env::var("ANTHROPIC_API_KEY").ok(),
    };

    let has_custom_base = std::env::var("MAE_AI_BASE_URL").is_ok();

    // No API key = no AI (unless using a local provider like Ollama)
    if api_key.is_none() && !has_custom_base {
        return None;
    }

    // If custom base URL is set, default to openai-compatible provider
    let provider_type = if has_custom_base && provider_type == "claude" {
        "openai".into()
    } else {
        provider_type
    };

    let model = std::env::var("MAE_AI_MODEL").unwrap_or_else(|_| match provider_type.as_str() {
        "openai" => "gpt-4o".into(),
        _ => "claude-sonnet-4-20250514".into(),
    });

    let timeout_secs: u64 = std::env::var("MAE_AI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    Some(ProviderConfig {
        provider_type,
        api_key,
        model,
        base_url: std::env::var("MAE_AI_BASE_URL").ok(),
        max_tokens: 4096,
        temperature: None,
        timeout_secs,
    })
}

fn build_system_prompt() -> String {
    include_str!("system_prompt.md").into()
}

/// Load init.scm from standard locations.
fn load_init_file(scheme: &mut SchemeRuntime, editor: &mut Editor) {
    let candidates: Vec<PathBuf> = vec![
        dirs_candidate("mae/init.scm"),
        Some(PathBuf::from("init.scm")),
        Some(PathBuf::from("scheme/init.scm")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in candidates {
        if path.exists() {
            info!(path = %path.display(), "loading init file");
            match scheme.load_file(&path) {
                Ok(()) => {
                    scheme.apply_to_editor(editor);
                    info!(path = %path.display(), "init file loaded successfully");
                    editor.set_status(format!("Loaded {}", path.display()));
                    return;
                }
                Err(e) => {
                    error!(path = %path.display(), error = %e, "init file load failed");
                    editor.set_status(format!("Error in {}: {}", path.display(), e));
                    return;
                }
            }
        }
    }
    debug!("no init file found");
}

/// Find the first conversation buffer's Conversation, if any.
fn find_conversation_buffer_mut(editor: &mut Editor) -> Option<&mut mae_core::Conversation> {
    editor
        .buffers
        .iter_mut()
        .find_map(|b| b.conversation.as_mut())
}

fn dirs_candidate(rel: &str) -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .map(|base| base.join(rel))
}

/// Convert a crossterm KeyEvent into a mae_core KeyPress.
fn crossterm_to_keypress(key: &KeyEvent) -> Option<KeyPress> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mae_key = match key.code {
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Esc => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };

    Some(KeyPress {
        key: mae_key,
        ctrl,
        alt,
    })
}

fn handle_key(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    if editor.mode != Mode::Command {
        editor.status_msg.clear();
    }

    let mode_before = editor.mode;

    match editor.mode {
        Mode::Normal => handle_normal_mode(editor, scheme, key, pending_keys),
        Mode::Insert => handle_insert_mode(editor, scheme, key, pending_keys),
        Mode::Visual(_) => handle_visual_mode(editor, scheme, key, pending_keys),
        Mode::Command => handle_command_mode(editor, scheme, key, pending_keys, ai_tx),
        Mode::ConversationInput => {
            handle_conversation_input(editor, key, ai_tx);
        }
        Mode::Search => handle_search_mode(editor, key),
        Mode::FilePicker => handle_file_picker_mode(editor, key),
    }

    if editor.mode != mode_before {
        pending_keys.clear();
    }
}

/// Dispatch a command by name, handling both builtins and Scheme commands.
fn dispatch_command(editor: &mut Editor, scheme: &mut SchemeRuntime, name: &str) {
    let source = editor.commands.get(name).map(|c| c.source.clone());

    match source {
        Some(CommandSource::Builtin) => {
            debug!(command = name, source = "builtin", "dispatching command");
            editor.dispatch_builtin(name);
        }
        Some(CommandSource::Scheme(fn_name)) => {
            debug!(command = name, scheme_fn = %fn_name, "dispatching scheme command");
            scheme.inject_editor_state(editor);
            match scheme.call_function(&fn_name) {
                Ok(result) => {
                    scheme.apply_to_editor(editor);
                    if !result.is_empty() {
                        editor.set_status(result);
                    }
                }
                Err(e) => {
                    error!(command = name, scheme_fn = %fn_name, error = %e, "scheme command failed");
                    editor.set_status(format!("Scheme error: {}", e));
                }
            }
        }
        None => {
            if !editor.dispatch_builtin(name) {
                warn!(command = name, "unknown command");
                editor.set_status(format!("Unknown command: {}", name));
            }
        }
    }
}

fn handle_search_mode(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
            editor.search_input.clear();
            editor.search_state.highlight_active = false;
        }
        KeyCode::Enter => {
            editor.mode = Mode::Normal;
            editor.execute_search();
        }
        KeyCode::Backspace => {
            if editor.search_input.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.search_input.pop();
            }
        }
        KeyCode::Char(ch) => {
            editor.search_input.push(ch);
        }
        _ => {}
    }
}

fn handle_file_picker_mode(editor: &mut Editor, key: KeyEvent) {
    let picker = match editor.file_picker.as_mut() {
        Some(p) => p,
        None => {
            editor.mode = Mode::Normal;
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            editor.file_picker = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            if let Some(path) = picker.selected_path() {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                editor.open_file(&path.to_string_lossy());
            } else {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                editor.set_status("No file selected");
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            picker.move_up();
        }
        KeyCode::Down | KeyCode::Tab => {
            picker.move_down();
        }
        KeyCode::Backspace => {
            if picker.query.is_empty() {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
            } else {
                picker.query.pop();
                picker.update_filter();
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_up();
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_down();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.file_picker = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Char(ch) => {
            picker.query.push(ch);
            picker.update_filter();
        }
        _ => {}
    }
}

fn handle_keymap_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        editor.running = false;
        return;
    }

    let Some(kp) = crossterm_to_keypress(&key) else {
        return;
    };

    pending_keys.push(kp);

    let mode_name = match editor.mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Visual(_) => "visual",
        Mode::Command | Mode::ConversationInput | Mode::Search | Mode::FilePicker => "command",
    };

    let result = editor
        .keymaps
        .get(mode_name)
        .map(|km| km.lookup(pending_keys))
        .unwrap_or(LookupResult::None);

    match result {
        LookupResult::Exact(cmd) => {
            let cmd = cmd.to_string();
            pending_keys.clear();
            editor.which_key_prefix.clear();
            dispatch_command(editor, scheme, &cmd);
        }
        LookupResult::Prefix => {
            editor.which_key_prefix = pending_keys.clone();
        }
        LookupResult::None => {
            pending_keys.clear();
            if !editor.which_key_prefix.is_empty() {
                editor.set_status("Key not bound");
            }
            editor.which_key_prefix.clear();
        }
    }
}

fn handle_normal_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // If a char-argument command is pending (f/F/t/T or text objects), capture the next char
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            // Try text object dispatch first, then fall back to char motion
            if !editor.dispatch_text_object(&cmd, ch) {
                editor.dispatch_char_motion(&cmd, ch);
            }
        }
        // Any key (including Escape) clears the pending state
        return;
    }

    // Count prefix accumulation: digits 1-9 start a count, 0 continues it
    if let KeyCode::Char(ch @ '1'..='9') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && pending_keys.is_empty()
        {
            let digit = (ch as usize) - ('0' as usize);
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10 + digit).min(99999));
            return;
        }
    }
    if let KeyCode::Char('0') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && editor.count_prefix.is_some()
            && pending_keys.is_empty()
        {
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10).min(99999));
            return;
        }
    }

    // Escape dismisses which-key popup if active, and clears count prefix
    if key.code == KeyCode::Esc {
        editor.count_prefix = None;
        if !editor.which_key_prefix.is_empty() {
            pending_keys.clear();
            editor.which_key_prefix.clear();
            return;
        }
    }
    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn handle_visual_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // Handle pending char-argument commands (f/F/t/T or text objects)
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            if !editor.dispatch_text_object(&cmd, ch) {
                editor.dispatch_char_motion(&cmd, ch);
            }
        }
        return;
    }

    // Count prefix accumulation (same as normal mode)
    if let KeyCode::Char(ch @ '1'..='9') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && pending_keys.is_empty()
        {
            let digit = (ch as usize) - ('0' as usize);
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10 + digit).min(99999));
            return;
        }
    }
    if let KeyCode::Char('0') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && editor.count_prefix.is_some()
            && pending_keys.is_empty()
        {
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10).min(99999));
            return;
        }
    }

    if key.code == KeyCode::Esc {
        editor.count_prefix = None;
    }

    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn handle_insert_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    match key.code {
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, ch);
        }
        KeyCode::Enter => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
        }
        KeyCode::Backspace => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
        }
        _ => {
            handle_keymap_mode(editor, scheme, key, pending_keys);
        }
    }
}

fn handle_conversation_input(
    editor: &mut Editor,
    key: KeyEvent,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            let buf_idx = editor.active_buffer_idx();
            let mut input = String::new();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                input = conv.input_line.clone();
                if !input.is_empty() {
                    conv.push_user(&input);
                    conv.input_line.clear();
                    conv.streaming = true;
                    conv.streaming_start = Some(std::time::Instant::now());
                }
            }
            if !input.is_empty() {
                if let Some(tx) = ai_tx {
                    if tx.try_send(AiCommand::Prompt(input)).is_err() {
                        warn!("AI command channel full or closed — prompt dropped");
                    }
                    editor.set_status("[AI] Thinking...");
                } else {
                    warn!("AI prompt submitted but no AI provider configured");
                    editor
                        .set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
                    if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                        conv.streaming = false;
                        conv.streaming_start = None;
                    }
                }
            }
            editor.mode = Mode::Normal;
        }
        KeyCode::Backspace => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_line.pop();
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Cancel streaming if active, otherwise exit mode
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                if conv.streaming {
                    info!("user cancelled AI streaming");
                    conv.streaming = false;
                    conv.streaming_start = None;
                    conv.push_system("[cancelled]");
                    if let Some(tx) = ai_tx {
                        if tx.try_send(AiCommand::Cancel).is_err() {
                            warn!("failed to send cancel to AI session");
                        }
                    }
                    return;
                }
            }
            editor.running = false;
        }
        KeyCode::Char(ch) => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_line.push(ch);
            }
        }
        _ => {}
    }
}

fn handle_command_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    pending_keys.clear();
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
            editor.command_line.clear();
        }
        KeyCode::Enter => {
            let cmd = editor.command_line.clone();
            editor.mode = Mode::Normal;
            editor.command_line.clear();

            // Record in command history before executing
            editor.push_command_history(&cmd);

            // :ai-status — show AI configuration
            if cmd == "ai-status" {
                let config = load_ai_config();
                if let Some(ref cfg) = config {
                    editor.set_status(format!(
                        "AI: provider={}, model={}, connected={}",
                        cfg.provider_type,
                        cfg.model,
                        ai_tx.is_some()
                    ));
                } else {
                    editor.set_status(
                        "AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY env var.",
                    );
                }
                return;
            }

            // :ai <prompt> — send to AI agent
            if let Some(prompt) = cmd.strip_prefix("ai ") {
                let prompt = prompt.trim();
                if prompt.is_empty() {
                    editor.set_status("Usage: :ai <prompt>");
                    return;
                }
                if let Some(tx) = ai_tx {
                    info!(
                        prompt_len = prompt.len(),
                        "sending AI prompt via command mode"
                    );
                    if tx.try_send(AiCommand::Prompt(prompt.to_string())).is_err() {
                        warn!("AI command channel full or closed — prompt dropped");
                    }
                    editor.set_status("[AI] Thinking...");
                } else {
                    warn!("AI prompt submitted but no AI provider configured");
                    editor
                        .set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
                }
                return;
            }

            // :eval <scheme> — Scheme REPL
            if let Some(code) = cmd.strip_prefix("eval ") {
                let code = code.trim();
                if code.is_empty() {
                    editor.set_status("eval: no expression given");
                    return;
                }
                debug!(code, "evaluating scheme expression");
                scheme.inject_editor_state(editor);
                match scheme.eval(code) {
                    Ok(result) => {
                        scheme.apply_to_editor(editor);
                        debug!(result = %result, "scheme eval succeeded");
                        if result.is_empty() {
                            editor.set_status("(ok)");
                        } else {
                            editor.set_status(result);
                        }
                    }
                    Err(e) => {
                        error!(code, error = %e, "scheme eval failed");
                        editor.set_status(format!("Scheme error: {}", e));
                    }
                }
                return;
            }

            // Registered command name (e.g., :move-down, :count-lines)
            let cmd_name = cmd.split_whitespace().next().unwrap_or("");
            if editor.commands.contains(cmd_name) {
                dispatch_command(editor, scheme, cmd_name);
            } else {
                // Fall back to ex commands (:w, :q, :q!, :wq, :e path)
                editor.execute_command(&cmd);
            }
        }
        KeyCode::Tab => {
            // Tab completion for :e <path>
            if editor.command_line.starts_with("e ") {
                let path_part = &editor.command_line[2..];
                if editor.tab_completions.is_empty() {
                    editor.tab_completions = mae_core::file_picker::complete_path(path_part);
                    editor.tab_completion_idx = 0;
                } else {
                    editor.tab_completion_idx =
                        (editor.tab_completion_idx + 1) % editor.tab_completions.len();
                }
                if !editor.tab_completions.is_empty() {
                    let completion = editor.tab_completions[editor.tab_completion_idx].clone();
                    editor.command_line = format!("e {}", completion);
                }
            }
        }
        KeyCode::Up => {
            editor.command_history_prev();
        }
        KeyCode::Down => {
            editor.command_history_next();
        }
        KeyCode::Backspace => {
            if editor.command_line.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.command_line.pop();
                editor.tab_completions.clear();
            }
        }
        KeyCode::Char(ch) => {
            editor.command_line.push(ch);
            editor.tab_completions.clear();
        }
        _ => {}
    }
}
