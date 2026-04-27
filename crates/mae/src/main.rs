mod agents;
mod ai_event_handler;
mod bootstrap;
mod config;
mod dap_bridge;
#[cfg(feature = "gui")]
mod gui_event;
mod key_handling;
mod lsp_bridge;
mod shell_keys;
mod shell_lifecycle;
mod terminal_loop;
mod watchdog;

use std::io;
use std::panic;

use mae_ai::{ai_specific_tools, tools_from_registry};
#[cfg(feature = "gui")]
use mae_ai::{AiCommand, AiEvent};
use mae_core::{Buffer, Editor};
#[cfg(feature = "gui")]
use mae_dap::DapCommand;
#[cfg(feature = "gui")]
use mae_lsp::LspCommand;
#[cfg(feature = "gui")]
use mae_renderer::Renderer;
use mae_scheme::SchemeRuntime;
use tracing::{error, info, warn};

use bootstrap::{init_logging, load_history, load_init_file, setup_ai, setup_dap, setup_lsp};
use terminal_loop::{cleanup_stale_mcp_sockets, run_headless_self_test, run_terminal_loop};

/// Entry point for the MAE editor.
///
/// Plain `fn main()` — the tokio runtime is constructed manually so that
/// the GUI path can use winit's `EventLoop::run_app()` on the main thread
/// (required by Wayland/macOS compositors) with tokio on a background thread.
///
/// Emacs lesson: Emacs's event loop is synchronous and single-threaded.
/// Retrofitting concurrency required 23,901 commits across 3 GC branches.
/// We use async from day one so the AI agent can operate as a peer.
fn main() -> io::Result<()> {
    // Create the in-editor message log first, then wire it into both
    // the tracing subscriber (for structured JSON logs to stderr + in-editor capture)
    // and the Editor (for the :messages command).
    let message_log = mae_core::MessageLog::new(1000);
    let log_handle = message_log.handle();
    init_logging(log_handle);

    info!(version = env!("CARGO_PKG_VERSION"), "mae starting");

    // Sync PATH from user's shell (login/interactive) so we can find binaries
    // even when launched from a desktop environment with a minimal PATH.
    mae_shell::path::sync_path_from_shell();

    // Set up panic hook to restore terminal on crash
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Best-effort terminal cleanup — swallow errors since we're already panicking
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        default_hook(info);
    }));

    let args: Vec<String> = std::env::args().collect(); // Handle --version / --help / --init-config before the terminal UI starts.
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
        println!("  --setup-agents [DIR]    Write .mcp.json & agent settings for discovery");
        println!("  --check-config          Validate init.scm + config.toml and exit (for CI)");
        println!("  --self-test [CATS]      Run AI self-test headless, exit with pass/fail code");
        println!();
        println!("CONFIG:");
        println!("  {}", config::config_path().display());
        println!();
        println!("ENVIRONMENT:");
        println!("  MAE_AI_PROVIDER     claude | openai | gemini | ollama | deepseek");
        println!("  MAE_AI_MODEL        model identifier");
        println!("  MAE_AI_BASE_URL     custom API base URL (for Ollama/vLLM/proxies)");
        println!("  MAE_AI_TIMEOUT_SECS HTTP timeout (default 300)");
        println!("  ANTHROPIC_API_KEY   Claude API key");
        println!("  OPENAI_API_KEY      OpenAI API key");
        println!("  GEMINI_API_KEY      Gemini API key");
        println!("  DEEPSEEK_API_KEY    DeepSeek API key");
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

        for agent in agents::builtin_agents() {
            match agents::setup_agent(agent.name, &dir) {
                Ok(msg) => println!("  {}: {}", agent.name, msg),
                Err(e) => eprintln!("  {}: Failed: {}", agent.name, e),
            }
        }
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

    // --check-config: bootstrap editor + Scheme, load init.scm, exit with status.
    // Useful in CI to validate that init.scm parses and evaluates cleanly.
    if args.iter().any(|a| a == "--check-config") {
        let mut editor = Editor::new();
        let (app_config, _) = config::load_config();
        if let Some(ref theme) = app_config.editor.theme {
            editor.set_theme_by_name(theme);
        }
        let mut scheme = match SchemeRuntime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("mae: scheme runtime init failed: {}", e.message);
                std::process::exit(1);
            }
        };
        load_init_file(&mut scheme, &mut editor);
        // Check if init.scm set an error status
        let status = &editor.status_msg;
        if status.starts_with("Error in") {
            eprintln!("mae: {}", status);
            std::process::exit(1);
        }
        println!("mae: config OK");
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
        let mut ed = Editor::new();
        ed.install_dashboard();
        ed
    };
    editor.message_log = message_log;

    // Spawn the watchdog thread and wire heartbeat into the editor.
    let watchdog_state = watchdog::spawn_watchdog();
    editor.heartbeat = watchdog_state.heartbeat.clone();
    editor.watchdog_stall_count = watchdog_state.stall_count.clone();
    editor.watchdog_stall_recovery = watchdog_state.stall_recovery.clone();

    // Auto-detect project from CWD if not already set (e.g. no-file-arg startup).
    if editor.project.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(root) = mae_core::detect_project_root(&cwd) {
                editor.recent_projects.push(root.clone());
                editor.project = Some(mae_core::Project::from_root(root));
            }
        }
    }

    // Cache the current git branch for status line display.
    editor.refresh_git_branch();

    // Apply editor preferences from config file.
    let (app_config, config_error) = config::load_config();
    if let Some(ref err_msg) = config_error {
        editor.status_msg = err_msg.clone();
    }
    if let Some(ref theme) = app_config.editor.theme {
        editor.set_theme_by_name(theme);
    }
    if let Some(ref art) = app_config.editor.splash_art {
        editor.splash_art = Some(art.clone());
    }
    if let Some(ref cmd) = app_config.ai.editor {
        editor.ai_editor = cmd.clone();
    }
    if let Some(restore) = app_config.editor.restore_session {
        editor.restore_session = restore;
    }

    // Apply font settings from config early (init.scm can override).
    if let Some(size) = app_config.editor.font_size {
        editor.gui_font_size = size;
    }
    if let Some(ref family) = app_config.editor.font_family {
        editor.gui_font_family = family.clone();
    }
    if let Some(ref icon_family) = app_config.editor.icon_font_family {
        editor.gui_icon_font_family = icon_family.clone();
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

    // Load init.scm and history.scm
    load_init_file(&mut scheme, &mut editor);
    load_history(&mut scheme, &mut editor);

    // Fire app-start hook after initialization is complete.
    editor.fire_hook("app-start");

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

    // Build the tokio runtime manually. The GUI path needs the event loop
    // on the main thread (winit requirement) with tokio on a background
    // thread. The terminal path runs tokio on the main thread as before.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))?;

    // Bootstrap async tasks (AI/LSP/DAP/MCP) inside the runtime context.
    // `setup_ai`/`setup_lsp`/`setup_dap` call `tokio::spawn` internally.
    let (
        mut ai_event_rx,
        ai_event_tx,
        ai_command_tx,
        mut lsp_event_rx,
        lsp_command_tx,
        mut dap_event_rx,
        dap_command_tx,
        mut mcp_tool_rx,
        mcp_socket_path,
        all_tools,
        permission_policy,
    ) = rt.block_on(async {
        let (ai_event_rx, ai_event_tx, ai_command_tx) = setup_ai(&editor);
        info!(
            ai_configured = ai_command_tx.is_some(),
            "AI agent setup complete"
        );

        let (lsp_event_rx, lsp_command_tx) = {
            let root_uri = editor
                .active_project_root()
                .map(|p| format!("file://{}", p.display()));
            setup_lsp(root_uri)
        };
        info!("LSP task spawned");

        // AI session restoration
        if editor.restore_session {
            if let Some(root) = editor.active_project_root() {
                let session_path = root.join(".mae/conversation.json");
                if session_path.exists() {
                    match editor.ai_load(&session_path) {
                        Ok(n) => info!(path = %session_path.display(), entries = n, "AI session restored"),
                        Err(e) => warn!(path = %session_path.display(), error = %e, "failed to restore AI session"),
                    }
                }
            }
        }

        let (dap_event_rx, dap_command_tx) = setup_dap();
        info!("DAP task spawned");

        let all_tools = {
            let mut tools = tools_from_registry(&editor.commands);
            tools.extend(ai_specific_tools(&editor.option_registry));
            tools
        };
        let permission_policy = config::resolve_permission_policy(&app_config);

        // MCP bridge: Unix socket for external agents (Claude Code, etc.)
        cleanup_stale_mcp_sockets();
        let mcp_socket_path = format!("/tmp/mae-{}.sock", std::process::id());
        let (mcp_tool_tx, mcp_tool_rx) = tokio::sync::mpsc::channel::<mae_mcp::McpToolRequest>(16);
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

        (
            ai_event_rx,
            ai_event_tx,
            ai_command_tx,
            lsp_event_rx,
            lsp_command_tx,
            dap_event_rx,
            dap_command_tx,
            mcp_tool_rx,
            mcp_socket_path,
            all_tools,
            permission_policy,
        )
    });

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

        let exit_code = rt.block_on(run_headless_self_test(
            &mut editor,
            &mut ai_event_rx,
            ai_command_tx.as_ref().unwrap(),
            &lsp_command_tx,
            &all_tools,
            &permission_policy,
            categories,
        ));

        let _ = std::fs::remove_file(&mcp_socket_path);
        std::process::exit(exit_code);
    }

    if use_gui {
        #[cfg(not(feature = "gui"))]
        {
            eprintln!("mae: GUI backend not compiled in. Rebuild with: cargo build --features gui");
            std::process::exit(1);
        }
        #[cfg(feature = "gui")]
        {
            return run_gui(
                rt,
                editor,
                scheme,
                ai_event_rx,
                ai_event_tx,
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
            );
        }
    }

    // Terminal path: run the async event loop on the main thread.
    rt.block_on(run_terminal_loop(
        &mut editor,
        &mut scheme,
        &mut ai_event_rx,
        &ai_event_tx,
        &ai_command_tx,
        &mut lsp_event_rx,
        &lsp_command_tx,
        &mut dap_event_rx,
        &dap_command_tx,
        &mut mcp_tool_rx,
        &mcp_socket_path,
        &all_tools,
        &permission_policy,
        &app_config,
    ))?;

    let _ = std::fs::remove_file(&mcp_socket_path);
    info!("mae exited cleanly");
    Ok(())
}

// ---------------------------------------------------------------------------
// GUI event loop (Phase 8 M4: run_app + EventLoopProxy)
// ---------------------------------------------------------------------------
//
// Architecture: main thread runs EventLoop::run_app(&mut GuiApp) (blocking).
// Background thread runs a tokio current_thread runtime with the bridge_task
// that reads AI/LSP/DAP/MCP channels and forwards events via EventLoopProxy.
// This replaces the pump_app_events anti-pattern that broke Wayland.

/// Launch the GUI event loop. Consumes the tokio runtime (moved to a
/// background thread) and blocks the main thread on `run_app`.
#[cfg(feature = "gui")]
#[allow(clippy::too_many_arguments)]
fn run_gui(
    rt: tokio::runtime::Runtime,
    mut editor: Editor,
    scheme: SchemeRuntime,
    ai_event_rx: tokio::sync::mpsc::Receiver<AiEvent>,
    ai_event_tx: tokio::sync::mpsc::Sender<AiEvent>,
    ai_command_tx: Option<tokio::sync::mpsc::Sender<AiCommand>>,
    lsp_event_rx: tokio::sync::mpsc::Receiver<mae_lsp::LspTaskEvent>,
    lsp_command_tx: tokio::sync::mpsc::Sender<LspCommand>,
    dap_event_rx: tokio::sync::mpsc::Receiver<mae_dap::DapTaskEvent>,
    dap_command_tx: tokio::sync::mpsc::Sender<DapCommand>,
    mcp_tool_rx: tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    mcp_socket_path: String,
    all_tools: Vec<mae_ai::ToolDefinition>,
    permission_policy: mae_ai::PermissionPolicy,
    app_config: config::Config,
) -> io::Result<()> {
    use gui_event::MaeEvent;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use winit::event_loop::EventLoop;

    let mut renderer = mae_gui::GuiRenderer::new();
    renderer.set_font_config(
        if editor.gui_font_family.is_empty() {
            None
        } else {
            Some(editor.gui_font_family.clone())
        },
        if editor.gui_icon_font_family.is_empty() {
            None
        } else {
            Some(editor.gui_icon_font_family.clone())
        },
        Some(editor.gui_font_size),
    );
    editor.renderer_name = "gui".to_string();
    editor.org_hide_emphasis_markers = app_config.editor.org_hide_emphasis_markers.unwrap_or(false);
    editor.clipboard = "unnamedplus".to_string();

    // Create typed event loop with user events — must happen on main thread
    // before the tokio runtime moves to the background.
    let event_loop = EventLoop::<MaeEvent>::with_user_event()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let proxy = event_loop.create_proxy();

    // Shared atomics so the bridge task only sends ticks when relevant.
    let shell_active = Arc::new(AtomicBool::new(false));
    let mcp_active = Arc::new(AtomicBool::new(false));

    // Move the tokio runtime + bridge task to a background thread.
    let shell_active_bg = shell_active.clone();
    let mcp_active_bg = mcp_active.clone();
    std::thread::spawn(move || {
        rt.block_on(bridge_task(
            proxy,
            ai_event_rx,
            lsp_event_rx,
            dap_event_rx,
            mcp_tool_rx,
            shell_active_bg,
            mcp_active_bg,
        ));
    });

    info!("entering GUI event loop (run_app + EventLoopProxy)");

    let last_theme_name = editor.theme.name.clone();
    let mut app = GuiApp {
        renderer,
        editor,
        scheme,
        pending_keys: Vec::new(),
        shell_pending_keys: Vec::new(),
        shell_terminals: std::collections::HashMap::new(),
        shell_last_dims: std::collections::HashMap::new(),
        ai_event_tx,
        ai_command_tx,
        deferred_ai_reply: None,
        deferred_dap_reply: None,
        pending_interactive_event: None,
        deferred_mcp_reply: Vec::new(),
        last_mcp_activity: None,
        all_tools,
        permission_policy,
        lsp_command_tx,
        dap_command_tx,
        mcp_socket_path,
        app_config,
        ctrl_held: false,
        alt_held: false,
        shift_held: false,
        dirty: true,
        cursor_x: 0.0,
        cursor_y: 0.0,
        scroll_accumulator: 0.0,
        mouse_pressed: false,
        shell_generations: std::collections::HashMap::new(),
        last_render: std::time::Instant::now(),
        input_dirty: false,
        bell_sent: false,
        last_theme_name,
        shell_active,
        mcp_active,
    };

    event_loop
        .run_app(&mut app)
        .map_err(|e| io::Error::other(e.to_string()))?;

    // Cleanup.
    let _ = std::fs::remove_file(&app.mcp_socket_path);
    let _ = app.renderer.cleanup();
    info!("mae (GUI) exited cleanly");
    Ok(())
}

/// Async bridge task — runs on the background tokio thread, reads all async
/// channels and forwards events to the main thread via `EventLoopProxy`.
///
/// This is the Alacritty pattern: the event loop sleeps until an OS event
/// *or* a proxy wakeup. No polling, no 16ms fallback sleep needed.
#[cfg(feature = "gui")]
async fn bridge_task(
    proxy: winit::event_loop::EventLoopProxy<gui_event::MaeEvent>,
    mut ai_rx: tokio::sync::mpsc::Receiver<AiEvent>,
    mut lsp_rx: tokio::sync::mpsc::Receiver<mae_lsp::LspTaskEvent>,
    mut dap_rx: tokio::sync::mpsc::Receiver<mae_dap::DapTaskEvent>,
    mut mcp_rx: tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    shell_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mcp_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use gui_event::MaeEvent;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    let mut shell_interval = tokio::time::interval(Duration::from_millis(33));
    let mut mcp_interval = tokio::time::interval(Duration::from_millis(500));
    let mut health_interval = tokio::time::interval(Duration::from_secs(30));

    // Skip the initial immediate tick from each interval.
    shell_interval.tick().await;
    mcp_interval.tick().await;
    health_interval.tick().await;

    loop {
        tokio::select! {
            biased;

            Some(ev) = ai_rx.recv() => {
                if proxy.send_event(MaeEvent::AiEvent(ev)).is_err() { break; }
            }
            Some(ev) = lsp_rx.recv() => {
                if proxy.send_event(MaeEvent::LspEvent(ev)).is_err() { break; }
            }
            Some(ev) = dap_rx.recv() => {
                if proxy.send_event(MaeEvent::DapEvent(ev)).is_err() { break; }
            }
            Some(ev) = mcp_rx.recv() => {
                if proxy.send_event(MaeEvent::McpToolRequest(ev)).is_err() { break; }
            }
            _ = shell_interval.tick() => {
                if shell_active.load(Relaxed) {
                    let _ = proxy.send_event(MaeEvent::ShellTick);
                }
            }
            _ = mcp_interval.tick() => {
                if mcp_active.load(Relaxed) {
                    let _ = proxy.send_event(MaeEvent::McpIdleTick);
                }
            }
            _ = health_interval.tick() => {
                let _ = proxy.send_event(MaeEvent::HealthCheck);
            }
        }
    }
}

/// GUI application state — owns all editor state on the main thread.
///
/// Implements `ApplicationHandler<MaeEvent>` for winit's `run_app()`.
/// This replaces the old `WinitCallback<'a>` which borrowed everything
/// via mutable references (required by `pump_app_events`).
#[cfg(feature = "gui")]
struct GuiApp {
    // Rendering
    renderer: mae_gui::GuiRenderer,

    // Core state (owned on main thread — not Send, which is fine)
    editor: Editor,
    scheme: SchemeRuntime,

    // Key state
    pending_keys: Vec<mae_core::KeyPress>,
    shell_pending_keys: Vec<mae_core::KeyPress>,

    // Shell terminals
    shell_terminals: std::collections::HashMap<usize, mae_shell::ShellTerminal>,
    shell_last_dims: std::collections::HashMap<usize, (u16, u16)>,

    // AI/MCP state
    ai_event_tx: tokio::sync::mpsc::Sender<AiEvent>,
    ai_command_tx: Option<tokio::sync::mpsc::Sender<AiCommand>>,
    deferred_ai_reply: ai_event_handler::DeferredAiReply,
    deferred_dap_reply: ai_event_handler::DeferredDapReply,
    pending_interactive_event: Option<ai_event_handler::PendingInteractiveEvent>,
    deferred_mcp_reply: ai_event_handler::DeferredMcpReply,
    last_mcp_activity: Option<tokio::time::Instant>,
    all_tools: Vec<mae_ai::ToolDefinition>,
    permission_policy: mae_ai::PermissionPolicy,

    // Command senders (main thread → background tokio thread)
    lsp_command_tx: tokio::sync::mpsc::Sender<LspCommand>,
    dap_command_tx: tokio::sync::mpsc::Sender<DapCommand>,

    // Config
    mcp_socket_path: String,
    app_config: config::Config,

    // Input state
    ctrl_held: bool,
    alt_held: bool,
    shift_held: bool,
    dirty: bool,
    cursor_x: f64,
    cursor_y: f64,
    scroll_accumulator: f64,
    mouse_pressed: bool,

    // Shell generation tracking (dirty-check optimisation — TUI parity)
    shell_generations: std::collections::HashMap<usize, u64>,

    // Frame cap (60fps) + input-pending bypass (Emacs dispnew.c:3254 pattern)
    last_render: std::time::Instant,
    /// Keyboard/mouse input needs immediate visual feedback.
    /// Bypasses the 60fps frame cap so scroll/movement is never delayed.
    input_dirty: bool,

    // Bell urgency state
    bell_sent: bool,

    // Theme change tracking for shell color sync.
    last_theme_name: String,

    // Shared atomics (read by bridge_task to gate conditional ticks)
    shell_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mcp_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "gui")]
impl GuiApp {
    /// Drain editor intents to LSP/DAP, manage shells and agents.
    fn drain_intents_and_lifecycle(&mut self) {
        lsp_bridge::drain_lsp_intents(&mut self.editor, &self.lsp_command_tx);
        dap_bridge::drain_dap_intents(&mut self.editor, &self.dap_command_tx);

        shell_lifecycle::drain_agent_setup(&mut self.editor);
        shell_lifecycle::spawn_pending_shells(
            &mut self.editor,
            &mut self.shell_terminals,
            &mut self.shell_last_dims,
            &self.renderer,
            &self.mcp_socket_path,
            &self.app_config,
        );
        shell_lifecycle::resize_shells(
            &self.editor,
            &self.renderer,
            &self.shell_terminals,
            &mut self.shell_last_dims,
        );
        shell_lifecycle::manage_shell_lifecycle(&mut self.editor, &mut self.shell_terminals);

        // Detect theme changes and update shell terminal colors.
        if self.editor.theme.name != self.last_theme_name {
            self.last_theme_name = self.editor.theme.name.clone();
            shell_lifecycle::update_shell_theme_colors(&self.editor, &self.shell_terminals);
        }

        // Clean up generation tracking for removed shells.
        self.shell_generations
            .retain(|idx, _| self.shell_terminals.contains_key(idx));
    }

    /// Send shutdown commands to AI/LSP/DAP tasks.
    fn shutdown(&mut self) {
        info!("editor shutting down (GUI)");

        // Fire app-exit hook.
        self.editor.fire_hook("app-exit");

        // Persist history
        if let Err(e) = bootstrap::save_history(&self.editor) {
            error!(error = %e, "failed to save history");
        }

        // If debug mode is enabled, save a tombstone dump.
        if self.editor.debug_mode {
            bootstrap::debug_dump(&self.editor);
        }

        // AI session persistence
        if self.editor.restore_session {
            if let Some(root) = self.editor.active_project_root() {
                let session_path = root.join(".mae/conversation.json");
                if let Some(parent) = session_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match self.editor.ai_save(&session_path) {
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

        if let Some(ref tx) = self.ai_command_tx {
            let _ = tx.try_send(AiCommand::Shutdown);
        }
        let _ = self.lsp_command_tx.try_send(LspCommand::Shutdown);
        let _ = self.dap_command_tx.try_send(DapCommand::Shutdown);
    }
}

#[cfg(feature = "gui")]
impl winit::application::ApplicationHandler<gui_event::MaeEvent> for GuiApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.renderer.window().is_none() {
            if let Err(e) = self.renderer.init_window(event_loop) {
                error!(error = %e, "failed to init GUI window");
                event_loop.exit();
            }
        }
    }

    fn user_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        event: gui_event::MaeEvent,
    ) {
        use gui_event::MaeEvent;

        match event {
            MaeEvent::AiEvent(ai_event) => {
                let ctx = ai_event_handler::AiEventContext {
                    all_tools: &self.all_tools,
                    permission_policy: &self.permission_policy,
                    deferred_ai_reply: &mut self.deferred_ai_reply,
                    deferred_dap_reply: &mut self.deferred_dap_reply,
                    pending_interactive_event: &mut self.pending_interactive_event,
                    lsp_command_tx: &self.lsp_command_tx,
                    dap_command_tx: &self.dap_command_tx,
                    ai_event_tx: &self.ai_event_tx,
                    ai_command_tx: &self.ai_command_tx,
                };
                ai_event_handler::handle_ai_event(&mut self.editor, ai_event, ctx);
                self.dirty = true;
            }
            MaeEvent::LspEvent(lsp_event) => {
                ai_event_handler::try_resolve_deferred(
                    &mut self.editor,
                    &lsp_event,
                    &mut self.deferred_ai_reply,
                );
                if ai_event_handler::try_resolve_deferred_mcp(
                    &lsp_event,
                    &mut self.deferred_mcp_reply,
                ) {
                    self.last_mcp_activity = Some(tokio::time::Instant::now());
                }
                if lsp_bridge::handle_lsp_event(&mut self.editor, &self.lsp_command_tx, lsp_event) {
                    self.dirty = true;
                }
            }
            MaeEvent::DapEvent(dap_event) => {
                // Try to resolve deferred DAP tool first (promise/await)
                let dap_action = ai_event_handler::try_resolve_deferred_dap(
                    &mut self.editor,
                    &dap_event,
                    &mut self.deferred_dap_reply,
                );
                dap_bridge::handle_dap_event(&mut self.editor, dap_event);
                if dap_action == ai_event_handler::DapResolveAction::TransitionedToStackTrace {
                    dap_bridge::drain_dap_intents(&mut self.editor, &self.dap_command_tx);
                }
                self.dirty = true;
            }
            MaeEvent::McpToolRequest(mcp_req) => {
                self.editor.input_lock = mae_core::InputLock::McpBusy;
                self.last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    &mut self.editor,
                    mcp_req,
                    &self.all_tools,
                    &self.permission_policy,
                    &self.lsp_command_tx,
                    &mut self.deferred_mcp_reply,
                );
                if immediate && self.deferred_mcp_reply.is_empty() {
                    self.editor.input_lock = mae_core::InputLock::None;
                    self.last_mcp_activity = None;
                }
                self.dirty = true;
            }
            MaeEvent::ShellTick => {
                for (idx, term) in &self.shell_terminals {
                    let gen = term.generation();
                    if self.shell_generations.get(idx) != Some(&gen) {
                        self.shell_generations.insert(*idx, gen);
                        self.dirty = true;
                    }
                }
            }
            MaeEvent::McpIdleTick => {
                if let Some(ts) = self.last_mcp_activity {
                    if ts.elapsed() > std::time::Duration::from_millis(500)
                        && self.deferred_mcp_reply.is_empty()
                    {
                        if self.editor.input_lock == mae_core::InputLock::McpBusy {
                            self.editor.set_status("MCP: input unlocked");
                        }
                        self.editor.input_lock = mae_core::InputLock::None;
                        self.last_mcp_activity = None;
                        self.dirty = true;
                    }
                }
            }
            MaeEvent::HealthCheck => {
                shell_lifecycle::health_check(
                    &mut self.editor,
                    &mut self.shell_terminals,
                    self.deferred_ai_reply.is_some(),
                    self.last_mcp_activity.is_some() || !self.deferred_mcp_reply.is_empty(),
                );
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        use winit::event::WindowEvent;

        match event {
            WindowEvent::CloseRequested => {
                self.shutdown();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.renderer.handle_resize(size.width, size.height);
                self.dirty = true;
            }
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                self.ctrl_held = state.control_key();
                self.alt_held = state.alt_key();
                self.shift_held = state.shift_key();
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == winit::event::ElementState::Pressed =>
            {
                self.dirty = true;
                self.input_dirty = true;
                self.editor.last_edit_time = std::time::Instant::now();
                if let Some(mae_core::InputEvent::Key(kp)) = mae_gui::winit_event_to_input(
                    &event,
                    self.ctrl_held,
                    self.alt_held,
                    self.shift_held,
                ) {
                    if self.editor.input_lock != mae_core::InputLock::None {
                        if kp.key == mae_core::Key::Escape
                            || (kp.key == mae_core::Key::Char('c') && kp.ctrl)
                        {
                            self.editor.input_lock = mae_core::InputLock::None;
                            self.editor.ai_streaming = false;
                            self.last_mcp_activity = None;
                            if let Some(ref tx) = self.ai_command_tx {
                                let _ = tx.try_send(AiCommand::Cancel);
                            }
                            self.editor.set_status("AI operation cancelled");
                        } else if self.editor.mode == mae_core::Mode::ShellInsert {
                            let ct_event = key_handling::keypress_to_crossterm(&kp);
                            shell_keys::handle_shell_key(
                                &mut self.editor,
                                ct_event,
                                &mut self.shell_terminals,
                                &mut self.shell_pending_keys,
                            );
                        }
                    } else if self.editor.mode == mae_core::Mode::ShellInsert {
                        let ct_event = key_handling::keypress_to_crossterm(&kp);
                        shell_keys::handle_shell_key(
                            &mut self.editor,
                            ct_event,
                            &mut self.shell_terminals,
                            &mut self.shell_pending_keys,
                        );
                    } else {
                        key_handling::handle_key_from_keypress(
                            &mut self.editor,
                            &mut self.scheme,
                            kp,
                            &mut self.pending_keys,
                            &self.ai_command_tx,
                            &mut self.pending_interactive_event,
                        );

                        if self.editor.ai_cancel_requested {
                            self.editor.ai_cancel_requested = false;
                            if let Some(ref tx) = self.ai_command_tx {
                                let _ = tx.try_send(AiCommand::Cancel);
                            }
                            self.editor.ai_streaming = false;
                            self.editor.input_lock = mae_core::InputLock::None;
                            self.pending_interactive_event = None;
                        }
                    }

                    // Check for editor shutdown after key handling.
                    if !self.editor.running {
                        self.shutdown();
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_x = position.x;
                self.cursor_y = position.y;
                if self.mouse_pressed {
                    let (cell_w, cell_h) = self.renderer.cell_dimensions();
                    if cell_w > 0.0 && cell_h > 0.0 {
                        let col = (self.cursor_x / cell_w as f64) as u16;
                        let row = (self.cursor_y / cell_h as f64) as u16;
                        self.editor.handle_mouse_drag(row as usize, col as usize);
                        self.dirty = true;
                    }
                }
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Pressed,
                button,
                ..
            } => {
                if let Some(mae_button) = mae_gui::winit_mouse_button(&button) {
                    if matches!(mae_button, mae_core::input::MouseButton::Left) {
                        self.mouse_pressed = true;
                    }
                    let (cell_w, cell_h) = self.renderer.cell_dimensions();
                    if cell_w > 0.0 && cell_h > 0.0 {
                        let col = (self.cursor_x / cell_w as f64) as u16;
                        let row = (self.cursor_y / cell_h as f64) as u16;
                        self.editor
                            .handle_mouse_click(row as usize, col as usize, mae_button);
                        self.dirty = true;
                    }
                }
            }
            WindowEvent::MouseInput {
                state: winit::event::ElementState::Released,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                self.mouse_pressed = false;
                let (cell_w, cell_h) = self.renderer.cell_dimensions();
                if cell_w > 0.0 && cell_h > 0.0 {
                    let col = (self.cursor_x / cell_w as f64) as u16;
                    let row = (self.cursor_y / cell_h as f64) as u16;
                    self.editor.handle_mouse_release(row as usize, col as usize);
                    self.dirty = true;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                use tracing::debug;
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        debug!(y, "MouseWheel: LineDelta");
                        y as i16
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        self.scroll_accumulator += pos.y;
                        let whole_lines = (self.scroll_accumulator / 20.0) as i16;
                        debug!(
                            pos_y = pos.y,
                            accum = self.scroll_accumulator,
                            whole_lines,
                            "MouseWheel: PixelDelta"
                        );
                        if whole_lines != 0 {
                            self.scroll_accumulator -= whole_lines as f64 * 20.0;
                        }
                        whole_lines
                    }
                };
                if lines != 0 {
                    self.editor.handle_mouse_scroll(lines);
                    self.dirty = true;
                    self.input_dirty = true;
                }
            }
            WindowEvent::RedrawRequested => {
                let render_start = std::time::Instant::now();
                if let Err(e) = self
                    .renderer
                    .render(&mut self.editor, &self.shell_terminals)
                {
                    warn!(error = %e, "GUI render error");
                }
                self.last_render = std::time::Instant::now();
                let frame_elapsed = render_start.elapsed().as_micros() as u64;
                self.editor.perf_stats.record_frame(frame_elapsed);
                if self.editor.debug_mode {
                    self.editor.perf_stats.sample_process_stats();
                }
                self.dirty = false;
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        use std::sync::atomic::Ordering::Relaxed;

        // Timeout deferred replies.
        ai_event_handler::timeout_deferred_reply(&mut self.editor, &mut self.deferred_ai_reply);
        ai_event_handler::timeout_deferred_dap_reply(
            &mut self.editor,
            &mut self.deferred_dap_reply,
        );
        ai_event_handler::timeout_deferred_mcp_reply(
            &mut self.editor,
            &mut self.deferred_mcp_reply,
        );

        // Font hot-reload: lisp-machine contract.
        if self.editor.gui_font_size != self.renderer.current_font_size() {
            self.renderer.apply_font_size(self.editor.gui_font_size);
            let viewport_height = self.renderer.viewport_height().unwrap_or(40);
            self.editor.viewport_height = viewport_height;
            self.dirty = true;
        }

        // Pre-render bookkeeping.
        self.editor.clamp_all_cursors();
        if let Ok((w, h)) = self.renderer.size() {
            let total_area = mae_core::WinRect {
                x: 0,
                y: 0,
                width: w,
                height: h.saturating_sub(2),
            };
            let vh = self.editor.focused_window_viewport_height(total_area);
            self.editor.viewport_height = vh;

            // Compute text_area_width for word-wrap cursor movement.
            let focused_id = self.editor.window_mgr.focused_id();
            let rects = self.editor.window_mgr.layout_rects(total_area);
            if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
                let inner_w = win_rect.width.saturating_sub(2) as usize;
                let buf = &self.editor.buffers[self.editor.active_buffer_idx()];
                let gutter_w = if self.editor.show_line_numbers {
                    mae_renderer::gutter_width(buf.display_line_count())
                } else {
                    2 // marker column + padding
                };
                self.editor.text_area_width = inner_w.saturating_sub(gutter_w);
            }

            if self.editor.word_wrap && self.editor.text_area_width > 0 {
                let tw = self.editor.text_area_width;
                let bi = self.editor.break_indent;
                let sb_w = self.editor.show_break.chars().count();
                let buf_idx = self.editor.active_buffer_idx();
                let rope = self.editor.buffers[buf_idx].rope().clone();
                let line_count = rope.len_lines();
                self.editor
                    .window_mgr
                    .focused_window_mut()
                    .ensure_scroll_wrapped(vh, |line| {
                        if line >= line_count {
                            return 1;
                        }
                        let rope_line = rope.line(line);
                        let text: String = rope_line.chars().collect();
                        let text = text.trim_end_matches('\n');
                        mae_core::wrap::wrap_line_display_rows(text, tw, bi, sb_w)
                    });
            } else {
                self.editor
                    .window_mgr
                    .focused_window_mut()
                    .ensure_scroll(vh);
            }
        }

        // Shell lifecycle (runs after every event batch).
        self.drain_intents_and_lifecycle();

        // Update shared atomics so the bridge task knows when to send ticks.
        self.shell_active
            .store(!self.shell_terminals.is_empty(), Relaxed);
        self.mcp_active.store(
            self.last_mcp_activity.is_some() || !self.deferred_mcp_reply.is_empty(),
            Relaxed,
        );

        // Bell → Wayland urgency hint (sway workspace highlight).
        if self.editor.bell_active() {
            if !self.bell_sent {
                if let Some(window) = self.renderer.window() {
                    window.request_user_attention(Some(winit::window::UserAttentionType::Critical));
                }
                self.bell_sent = true;
            }
        } else {
            self.bell_sent = false;
        }

        // Debounced syntax reparse: drain pending reparses after 50ms idle.
        if !self.editor.syntax_reparse_pending.is_empty()
            && self.editor.last_edit_time.elapsed() >= std::time::Duration::from_millis(50)
        {
            mae_core::syntax::drain_pending_reparses(&mut self.editor);
            self.dirty = true;
        }

        // Frame-capped redraw (60fps = 16.667ms).
        // Emacs pattern (dispnew.c:3254): input-pending bypasses frame cap
        // so keyboard/scroll never waits for the next frame boundary.
        if self.dirty {
            let elapsed = self.last_render.elapsed();
            let frame_budget = std::time::Duration::from_micros(16_667);
            if self.input_dirty || elapsed >= frame_budget {
                self.renderer.request_redraw();
                self.input_dirty = false;
            } else {
                // Schedule wakeup for remaining budget.
                event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                    std::time::Instant::now() + (frame_budget - elapsed),
                ));
            }
        } else if !self.editor.syntax_reparse_pending.is_empty() {
            // Pending reparses but not otherwise dirty — wake up when debounce expires.
            let debounce = std::time::Duration::from_millis(50);
            let wake_at = self.editor.last_edit_time + debounce;
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(wake_at));
        } else {
            // Not dirty — sleep until next event (no busy-loop).
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        }
    }
}
