// @stability: stable
// @ai-caution: [architecture-debt] Editor entry point. CLI dispatch, GuiApp,
// and config/KB-federation/daemon-connect bootstrapping were extracted (see
// cli.rs, gui_app.rs, bootstrap.rs) — this file went from 3,329 to ~950
// lines, 2026-07. Residual is sequential entry-point glue with no obvious
// further seam. Tracked in .claude/commands/mae-audit.md's "Known
// exceptions" and ROADMAP.md's "Architecture Debt" section — see both
// before further growing this file; prefer extracting a new module over
// adding here.

mod agents;
mod ai_event_handler;
mod ai_residency;
mod bootstrap;
mod cli;
mod collab_bridge;
mod config;
mod daemon_supervisor;
mod dap_bridge;
mod doctor;
#[cfg(feature = "gui")]
mod graph_layout_bridge;
#[cfg(feature = "gui")]
mod gui_app;
#[cfg(feature = "gui")]
mod gui_event;
mod key_handling;
mod lsp_bridge;
mod manual_kb;
mod mdns_discovery;
pub mod pkg;
mod practices_kb;
mod scheme_dap_bridge;
mod scheme_lsp_bridge;
mod shell_keys;
mod shell_lifecycle;
mod sync_broadcast;
mod terminal_loop;
mod test_runner;
mod upgrade;
mod watchdog;

use std::io;
use std::panic;

use mae_ai::{ai_specific_tools, tools_from_registry};
use mae_core::{Buffer, Editor};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};

/// Short git SHA of this build (`-dirty` if the working tree had uncommitted
/// changes, "unknown" if built outside a git checkout). Set by `build.rs`. Used
/// in the startup log, `--version`, and `collab-doctor` so a running editor can
/// be pinned to an exact commit across machines (the cross-machine deploy
/// discipline the live two-machine test depends on).
pub(crate) const BUILD_SHA: &str = match option_env!("MAE_BUILD_SHA") {
    Some(s) => s,
    None => "unknown",
};

use bootstrap::{init_logging, load_history, load_init_file, setup_ai, setup_dap, setup_lsp};
use terminal_loop::{cleanup_stale_mcp_sockets, run_headless_self_test, run_terminal_loop};

/// Pure policy: given environment signals, is a graphical display available?
///
/// Extracted from [`gui_display_available`] so the decision is unit-testable
/// without touching process-global environment variables (see `mod tests`).
fn display_available_from_env(ssh_session: bool, x11: bool, wayland: bool, is_macos: bool) -> bool {
    if ssh_session {
        // A remote shell has no local GUI session, regardless of platform.
        return false;
    }
    if is_macos {
        // Local macOS sessions run the Aqua window server (SSH ruled out above).
        return true;
    }
    // X11 or Wayland must be present (Linux / other unix).
    x11 || wayland
}

/// Heuristic: is a graphical display available for the GUI backend?
///
/// `mae` defaults to the GUI on a desktop session but must fall back to the
/// terminal UI when there is no graphics frontend — over SSH, on a bare tty,
/// or on a headless server. Explicit `--gui` overrides this (e.g. the MAE.app
/// launcher); `--no-gui`/`--tui`/`-nw` force the terminal UI.
fn gui_display_available() -> bool {
    #[cfg(not(unix))]
    {
        return true;
    }
    #[cfg(unix)]
    {
        let ssh =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        let x11 = std::env::var_os("DISPLAY").is_some();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        display_available_from_env(ssh, x11, wayland, cfg!(target_os = "macos"))
    }
}

/// Pure policy: should the GUI backend be launched, given all signals?
///
/// `--no-gui`/`-nw` (force_tui) always wins; `--gui` (force_gui) overrides
/// display detection (e.g. the MAE.app launcher); otherwise the GUI launches
/// only when compiled in and a display is available.
fn should_use_gui(
    gui_compiled: bool,
    force_tui: bool,
    force_gui: bool,
    display_available: bool,
) -> bool {
    gui_compiled && !force_tui && (force_gui || display_available)
}

/// Parse a boolean from an environment variable's **value** (not its mere
/// presence). Returns `None` when unset so callers can leave a config-derived
/// default untouched. Recognised falsy: `0/false/no/off` and empty; anything
/// else non-empty is truthy. This is the fix for the footgun where
/// `MAE_COLLAB_AUTO_CONNECT=false` still enabled auto-connect because the old
/// check keyed on `is_ok()` (presence).
fn env_bool(key: &str) -> Option<bool> {
    std::env::var(key).ok().map(|v| parse_truthy(&v))
}

/// Interpret a string as a boolean flag value. Falsy: `0/false/no/off` and
/// empty/whitespace (case-insensitive); everything else is truthy.
fn parse_truthy(v: &str) -> bool {
    !matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off" | ""
    )
}

/// Apply **per-launch** collaboration overrides (env vars) AFTER the config
/// files (config.toml + init.scm) have been applied, so they take precedence
/// per the standard chain: defaults < config files < env.
///
/// Before this, `init.scm` (loaded last) could beat the env vars, which were
/// either ignored entirely (GUI/TUI path never read `MAE_COLLAB_AUTO_CONNECT`)
/// or beaten by a later `(set-option! …)` in `init.scm`. Env: `MAE_COLLAB_SERVER`
/// (address), `MAE_COLLAB_AUTO_CONNECT` (parsed truthy/falsy).
/// ADR-020 B-16 / ADR-023: derive this peer's STABLE yrs `client_id` (and remember its
/// principal fingerprint) from the durable collab identity, so KB node edits + the
/// share lineage author under `derive_kb_client_id(fp, epoch)` — the SAME id the daemon's
/// epoch fence expects. **Shared by the interactive launch AND the headless `--test`
/// runner.** Without it on the `--test` path, scenarios fell back to `client_id = 1`,
/// disagreeing with the daemon's `c_now = derive_kb_client_id(fp, 0)` and tripping the
/// fence on every node edit (#166 — a test-harness gap, not a production bug). Idempotent.
fn init_collab_kb_client_id(editor: &mut Editor) {
    if editor.collab.local_kb_client_id != 0 {
        return;
    }
    if let Some(dir) = mae_mcp::identity::default_collab_dir() {
        let label = editor.collab.user_name.clone();
        if let Ok(id) = mae_mcp::identity::Identity::load_or_generate(&dir, &label) {
            let fp = id.fingerprint();
            let cid = mae_core::editor::derive_kb_client_id(&fp, 0);
            editor.collab.local_kb_client_id = cid;
            // Remember our own principal so node edits can be re-derived under a rotated
            // per-KB authorization epoch (see kb_client_id_for).
            editor.collab.local_fingerprint = fp;
            info!(
                client_id = cid,
                "KB CRDT client_id derived from collab identity"
            );
        }
    }
}

fn apply_collab_launch_overrides(editor: &mut Editor) {
    if let Ok(addr) = std::env::var("MAE_COLLAB_SERVER") {
        if !addr.trim().is_empty() {
            let _ = editor.set_option("collab_server_address", addr.trim());
        }
    }
    if let Some(v) = env_bool("MAE_COLLAB_AUTO_CONNECT") {
        let _ = editor.set_option("collab_auto_connect", &v.to_string());
        info!(
            auto_connect = v,
            "env MAE_COLLAB_AUTO_CONNECT override applied"
        );
    }
}

/// Phase D3 (ADR-029): cheap, bounded startup probe — does a local daemon already
/// host the primary KB (`kbc:default`)? If so, the editor skips the expensive
/// `load_all` mirror preload and resolves reads via the daemon instead (the open
/// store still yields individual nodes lazily on edit). Fast-fails when no daemon
/// is listening; a short read timeout bounds the worst case so startup never hangs
/// on a wedged daemon — on any error we fall through to the full local init.
fn probe_daemon_hosts_primary(socket: &std::path::Path) -> bool {
    let mut client = mae_mcp::daemon_client::DaemonClient::new(socket);
    client.set_timeout(std::time::Duration::from_millis(750));
    if client.connect().is_err() {
        return false;
    }
    match client.call("daemon/status", serde_json::json!({})) {
        Ok(v) => v
            .get("primary_exists")
            .and_then(|p| p.as_bool())
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// ADR-035 version-skew guardrail (editor side): compare the editor's own
/// version against a daemon's reported `version` (added to `daemon/status`).
/// Returns a human-readable warning when they differ — version skew is the #1
/// long-lived-daemon failure mode (a daemon built from a different release can
/// speak a drifted protocol/schema). MAE's crates are version-bumped in lockstep
/// by the release pipeline, so an equal version means a matching build.
///
/// Returns `None` when the versions match or the daemon didn't report one (an
/// older daemon predating the field — nothing to compare, so don't cry wolf).
fn daemon_version_skew(editor_version: &str, daemon_status: &serde_json::Value) -> Option<String> {
    let daemon_version = daemon_status.get("version").and_then(|v| v.as_str())?;
    if daemon_version == editor_version {
        return None;
    }
    Some(format!(
        "mae-daemon version {daemon_version} differs from this editor's version \
         {editor_version}; a version-skewed daemon may behave unexpectedly. Restart it \
         with the matching build (the `mae-daemon` from this install) to clear this."
    ))
}

/// Entry point for the MAE editor.
///
/// Plain `fn main()` — the tokio runtime is constructed manually so that
/// the GUI path can use winit's `EventLoop::run_app()` on the main thread
/// (required by Wayland/macOS compositors) with tokio on a background thread.
///
/// Binary-side [`mae_core::DaemonControl`] impl: a [`DaemonClient`] behind a
/// `Mutex` (the trait method is `&self`, but `DaemonClient::call` needs `&mut
/// self`). Injected into `editor.kb` so the editor's P2P share surfaces reach the
/// daemon control socket without `mae-core` depending on `mae-mcp`.
struct DaemonControlClient(std::sync::Mutex<mae_mcp::daemon_client::DaemonClient>);

impl mae_core::DaemonControl for DaemonControlClient {
    fn share_kb_p2p(
        &self,
        kb_id: &str,
        transport: Option<&str>,
        policy: Option<&str>,
    ) -> Result<String, String> {
        self.0
            .lock()
            .map_err(|_| "daemon control channel is poisoned".to_string())?
            .share_kb_p2p(kb_id, transport, policy)
            .map_err(|e| e.to_string())
    }
    fn mint_p2p_ticket(&self, kb_id: &str) -> Result<String, String> {
        self.0
            .lock()
            .map_err(|_| "daemon control channel is poisoned".to_string())?
            .mint_p2p_ticket(kb_id)
            .map_err(|e| e.to_string())
    }
    fn join_p2p_ticket(&self, ticket: &str) -> Result<String, String> {
        self.0
            .lock()
            .map_err(|_| "daemon control channel is poisoned".to_string())?
            .join_p2p_ticket(ticket)
            .map_err(|e| e.to_string())
    }
}

/// Enable the P2P daemon mesh (ADR-025) by writing `[collab.p2p]` to the local
/// `daemon.toml` (XDG-resolved, same dir as `config.toml`). Ensures key-mode auth
/// (the mesh authenticates peers by Ed25519 key) without clobbering an existing
/// mode. Value-based TOML edit: preserves other keys (not comments). Returns the
/// path written. For a *remote* daemon the admin sets `[collab.p2p]` there.
fn enable_daemon_p2p(relay: &str) -> io::Result<std::path::PathBuf> {
    let path = config::config_path()
        .parent()
        .map(|p| p.join("daemon.toml"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot resolve config dir"))?;

    let mut doc: toml::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_else(|| toml::Value::Table(Default::default()));

    let root = doc.as_table_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon.toml root is not a table",
        )
    })?;
    let collab = root
        .entry("collab".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "[collab] is not a table"))?;

    // The mesh has no PSK/anonymous path — ensure key mode if unset (don't
    // override a deliberate existing choice; `--check-config` flags a mismatch).
    {
        let auth = collab
            .entry("auth".to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()));
        if let Some(auth) = auth.as_table_mut() {
            auth.entry("mode".to_string())
                .or_insert_with(|| toml::Value::String("key".to_string()));
        }
    }

    let p2p = collab
        .entry("p2p".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "[collab.p2p] is not a table"))?;
    p2p.insert("enabled".to_string(), toml::Value::Boolean(true));
    p2p.insert("relay".to_string(), toml::Value::String(relay.to_string()));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(&doc).map_err(io::Error::other)?,
    )?;
    Ok(path)
}

/// Emacs lesson: Emacs's event loop is synchronous and single-threaded.
/// Retrofitting concurrency required 23,901 commits across 3 GC branches.
/// We use async from day one so the AI agent can operate as a peer.
fn main() -> io::Result<()> {
    // Create the in-editor message log first, then wire it into both
    // the tracing subscriber (for structured JSON logs to stderr + in-editor capture)
    // and the Editor (for the :messages command).
    // Pre-check --debug before init_logging so the env filter sees MAE_LOG=debug.
    // The flag is also processed later (line ~576) for editor.debug_mode/show_fps.
    if std::env::args().any(|a| a == "--debug")
        && std::env::var("MAE_LOG").is_err()
        && std::env::var("RUST_LOG").is_err()
    {
        std::env::set_var("MAE_LOG", "debug");
    }

    let message_log = mae_core::MessageLog::new(1000);
    let log_handle = message_log.handle();
    init_logging(log_handle);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        build = BUILD_SHA,
        "mae starting"
    );

    // Sync PATH from user's shell (login/interactive) so we can find binaries
    // even when launched from a desktop environment with a minimal PATH.
    debug!("syncing PATH from user shell");
    mae_shell::path::sync_path_from_shell();
    debug!("PATH sync complete");

    // Set up panic hook to restore terminal on crash
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Best-effort terminal cleanup — swallow errors since we're already panicking
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        default_hook(info);
    }));

    let args: Vec<String> = std::env::args().collect(); // Handle --version / --help / --init-config before the terminal UI starts.
    if let Some(result) = cli::handle_version(&args) {
        return result;
    }
    cli::handle_upgrade(&args);
    if let Some(result) = cli::handle_help(&args) {
        return result;
    }
    cli::handle_pkg(&args);
    cli::handle_flat_subcommands(&args);
    cli::handle_doctor(&args);
    if let Some(result) = cli::handle_print_config_path(&args) {
        return result;
    }
    if let Some(result) = cli::handle_print_config_template(&args) {
        return result;
    }
    if let Some(result) = cli::handle_collab_identity(&args) {
        return result;
    }
    cli::handle_kb_share_p2p(&args);
    cli::handle_kb_join(&args);
    if let Some(result) = cli::handle_setup_collab(&args) {
        return result;
    }
    if let Some(result) = cli::handle_setup_agents(&args) {
        return result;
    }
    if let Some(result) = cli::handle_init_config(&args) {
        return result;
    }
    if let Some(result) = cli::handle_check_config(&args) {
        return result;
    }
    cli::handle_test_mode(&args)?;

    // First-run wizard: runs only when stdin is a TTY, no config file exists,
    // no AI env vars are set, and MAE_SKIP_WIZARD is not set. Must run before
    // the renderer takes over the terminal.
    debug!("checking first-run wizard");
    if let Err(e) = config::maybe_run_first_run_wizard() {
        eprintln!("warning: first-run wizard failed: {}", e);
    }
    debug!("first-run wizard check complete");

    // --clean / -q: skip user config, init.scm, history, and project detection (like emacs -q)
    let clean_mode = args.iter().any(|a| a == "--clean" || a == "-q");

    // Find the first positional argument (not a flag).
    let file_arg = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, a)| !a.starts_with('-'))
        .map(|(_, a)| a.as_str());

    debug!("creating editor instance");
    let mut editor = if let Some(path) = file_arg {
        match Buffer::from_file(std::path::Path::new(&path)) {
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

    debug!("editor created, spawning watchdog");
    // Spawn the watchdog thread and wire heartbeat into the editor.
    let watchdog_state = watchdog::spawn_watchdog();
    editor.heartbeat = watchdog_state.heartbeat.clone();
    editor.watchdog_stall_count = watchdog_state.stall_count.clone();
    editor.watchdog_stall_recovery = watchdog_state.stall_recovery.clone();

    // Load persistent project list from XDG data dir.
    if !clean_mode {
        if let Some(data_dir) = editor.mae_data_dir() {
            // Prune stale entries (nonexistent dirs, temp dirs) and notify.
            // Goes through the locked reload-fresh-then-mutate path (rather
            // than a bare load+save) since another `mae` process could be
            // starting up and writing this same file at the same moment.
            let (project_list, pruned, saved) =
                mae_core::ProjectList::update(&data_dir, |pl| pl.prune_stale());
            editor.project_list = project_list;
            if !pruned.is_empty() {
                if let Err(e) = saved {
                    tracing::warn!(error = %e, "failed to persist pruned project list");
                }
                let msg = format!(
                    "Pruned {} stale project(s): {}",
                    pruned.len(),
                    pruned.join(", ")
                );
                tracing::info!("{}", msg);
                editor.set_status(&msg);
            }

            editor
                .project_list
                .sync_to_recent(&mut editor.recent_projects);
        }
    }

    // Auto-detect project from CWD if not already set (skipped in clean mode).
    if !clean_mode && editor.project.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(root) = mae_core::detect_project_root(&cwd) {
                editor.recent_projects.push(root.clone());
                let proj = mae_core::Project::from_root(root.clone());
                editor.project_list.touch(root, proj.name.clone());
                editor.project = Some(proj);
            }
        }
    }

    // Cache the current git branch for status line display.
    editor.refresh_git_branch();

    if clean_mode {
        editor.clean_mode = true;
        info!("clean mode: skipping config.toml, init.scm, and history.scm");
    }

    debug!("loading config file");
    // Apply editor preferences from config file.
    let (app_config, config_error) = if clean_mode {
        (config::Config::default(), None)
    } else {
        config::load_config()
    };
    if let Some(ref err_msg) = config_error {
        editor.status_msg = err_msg.clone();
    }
    bootstrap::apply_app_config(&mut editor, &app_config);

    debug!("config applied, initializing scheme runtime");
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

    // Load init.scm and history.scm (skipped in clean mode)
    if !clean_mode {
        debug!("loading init.scm and history");
        let _module_registry = load_init_file(&mut scheme, &mut editor);
        load_history(&mut scheme, &mut editor);
        debug!("init.scm and history loaded");
    }

    // Per-launch overrides win over config files (config.toml + init.scm):
    // defaults < config files < env. Must run AFTER init.scm.
    apply_collab_launch_overrides(&mut editor);

    bootstrap::init_kb_federation(&mut editor, clean_mode);

    bootstrap::init_daemon_connection(&mut editor);

    // Fire app-start hook after initialization is complete.
    editor.fire_hook("app-start");

    // --debug: enable debug mode (RSS/CPU/frame time in status bar)
    if args.iter().any(|a| a == "--debug") {
        editor.debug_mode = true;
        editor.show_fps = true;
        // MAE_LOG is already set before init_logging() (see main() top)
        info!("debug mode enabled via --debug flag");
    }

    // --debug-init: verbose init file loading
    if args.iter().any(|a| a == "--debug-init") {
        editor.debug_init = true;
        info!("debug-init mode enabled");
    }

    // GUI is the default when compiled with the gui feature (like emacs), but
    // only when a graphical display is actually available. `--no-gui`/`--tui`/
    // `-nw` force terminal mode; `--gui` forces the GUI backend, overriding
    // display detection (used by the MAE.app launcher). With no flags, `mae`
    // opens the GUI on a desktop session and transparently falls back to the
    // terminal UI over SSH, on a tty, or on a headless server.
    let force_tui = args
        .iter()
        .any(|a| a == "--no-gui" || a == "--tui" || a == "-nw");
    let force_gui = args.iter().any(|a| a == "--gui");
    let display_available = gui_display_available();
    let use_gui = should_use_gui(
        cfg!(feature = "gui"),
        force_tui,
        force_gui,
        display_available,
    );
    if cfg!(feature = "gui") && !force_tui && !force_gui && !display_available {
        info!("no graphical display detected (SSH/tty/headless) — using terminal UI; pass --gui to force GUI");
    }

    debug!("building tokio runtime");
    // Build the tokio runtime manually. The GUI path needs the event loop
    // on the main thread (winit requirement) with tokio on a background
    // thread. The terminal path runs tokio on the main thread as before.
    //
    // B-22: use a MULTI-threaded pool. The host-key TOFU verifier is called
    // synchronously by rustls mid-handshake and blocks (up to 120s) waiting for
    // the user's prompt answer. On a single-threaded runtime that one worker is
    // also the `bridge_task` forwarder (and MCP/AI/LSP/DAP), so the blocking wait
    // starved it — the `HostKeyPrompt` event never reached the GUI/MCP and the
    // modal never rendered (the GUI twin of the #66 TUI deadlock). A small worker
    // pool lets the forwarder keep running on another worker while a connect
    // blocks on the prompt, so the modal surfaces and the answer flows back.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
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
        mcp_client_mgr,
        sync_broadcaster,
    ) = rt.block_on(async {
        let (ai_event_rx, ai_event_tx, ai_command_tx) = setup_ai(&editor);
        info!(
            ai_configured = ai_command_tx.is_some(),
            "AI agent setup complete"
        );

        let (lsp_event_rx, lsp_command_tx, lsp_server_info) = {
            let root_uri = editor
                .active_project_root()
                .map(|p| format!("file://{}", p.display()));
            setup_lsp(root_uri, &app_config)
        };
        editor.lsp.servers = lsp_server_info;
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

        let mut all_tools = {
            let mut tools = tools_from_registry(&editor.commands);
            tools.extend(ai_specific_tools(&editor.option_registry));
            tools.extend(mae_ai::scheme_tools_to_definitions(&editor.ai.scheme_tools));
            tools
        };
        let permission_policy = config::resolve_permission_policy(&app_config);

        // MCP client: connect to external MCP servers configured in config.toml
        let mcp_client_configs = {
            let raw_toml: toml::Value = std::fs::read_to_string(config::config_path())
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(toml::Value::Table(Default::default()));
            mae_mcp::client_mgr::McpClientManager::parse_configs(&raw_toml)
        };
        let mcp_client_mgr = {
            let mut mgr = mae_mcp::client_mgr::McpClientManager::new(mcp_client_configs);
            mgr.start_all().await;
            // Convert external tools to ToolDefinitions for the AI agent
            for ext in mgr.external_tools() {
                let prefixed_name = format!("mcp_{}_{}", ext.server_name, ext.name);
                all_tools.push(mae_ai::ToolDefinition {
                    name: prefixed_name,
                    description: format!("[{}] {}", ext.server_name, ext.description),
                    parameters: mae_ai::ToolParameters {
                        schema_type: "object".into(),
                        properties: std::collections::HashMap::new(), // external schema not mapped
                        required: vec![],
                    },
                    permission: Some(mae_ai::PermissionTier::Privileged),
                });
            }
            if mgr.has_servers() {
                let status = mgr.status();
                info!(
                    server_count = status.len(),
                    total_tools = mgr.external_tools().len(),
                    "MCP external servers initialized"
                );
            }
            std::sync::Arc::new(tokio::sync::Mutex::new(mgr))
        };

        // MCP bridge: Unix socket for external agents (Claude Code, etc.)
        debug!("setting up MCP server");
        cleanup_stale_mcp_sockets();
        let mcp_socket_path = format!("/tmp/mae-{}.sock", std::process::id());
        let (mcp_tool_tx, mcp_tool_rx) = tokio::sync::mpsc::channel::<mae_mcp::McpToolRequest>(16);
        let sync_broadcaster: mae_mcp::broadcast::SharedBroadcaster =
            std::sync::Arc::new(std::sync::Mutex::new(mae_mcp::broadcast::EventBroadcaster::new()));
        {
            let mcp_tools: Vec<mae_mcp::protocol::ToolInfo> = all_tools
                .iter()
                .map(|t| {
                    // ADR-050 D2: annotations are mechanically derived from
                    // the tool's own PermissionTier, never hand-authored --
                    // see mae_ai::annotations_for_tier for the single source
                    // of truth. A tool with no declared tier gets no
                    // annotations at all (never guess readOnlyHint: true).
                    let annotations = t.permission.map(|tier| {
                        let (read_only_hint, destructive_hint, idempotent_hint) =
                            mae_ai::annotations_for_tier(tier);
                        mae_mcp::protocol::ToolAnnotations {
                            title: None,
                            read_only_hint: Some(read_only_hint),
                            destructive_hint: Some(destructive_hint),
                            idempotent_hint: Some(idempotent_hint),
                        }
                    });
                    mae_mcp::protocol::ToolInfo {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        input_schema: serde_json::to_value(&t.parameters).unwrap_or_default(),
                        permission: t.permission.map(|p| format!("{p:?}")),
                        annotations,
                    }
                })
                .collect();
            // Always-on AI guidance (gap closed alongside mae-agent-cli's
            // system prompt, mae_ai::guidance): surface the designated
            // guidance KB (ai_guidance_kb option) + every registered KB
            // instance's name via the MCP `initialize` response's
            // `instructions` field, so ANY MCP-connected client — not just
            // mae-agent-cli — gets pointed at relevant KBs on connect.
            // `None` (both unset/empty) omits the field entirely, matching
            // today's behavior for editors with nothing configured.
            let mcp_instructions: Option<String> = {
                let guidance_kb = editor
                    .get_option("ai_guidance_kb")
                    .map(|(v, _)| v)
                    .unwrap_or_default();
                let registered: Vec<String> = editor
                    .kb
                    .registry
                    .instances
                    .iter()
                    .map(|i| i.name.clone())
                    .collect();
                if guidance_kb.is_empty() && registered.is_empty() {
                    None
                } else {
                    let mut s = String::new();
                    if !guidance_kb.is_empty() {
                        s.push_str(&format!(
                            "Before acting, consult KB '{guidance_kb}' for required practices. "
                        ));
                    }
                    if !registered.is_empty() {
                        s.push_str(&format!("Registered KBs: {}.", registered.join(", ")));
                    }
                    Some(s)
                }
            };

            let mut server = mae_mcp::McpServer::new(
                &mcp_socket_path,
                mcp_tool_tx.clone(),
                sync_broadcaster.clone(),
            );
            if let Some(ref instructions) = mcp_instructions {
                server = server.with_instructions(instructions.clone());
            }
            tokio::spawn(server.run(mcp_tools.clone()));
            info!(socket = %mcp_socket_path, "MCP server started");

            // ADR-048: a SECOND, PSK-required socket dedicated to first-party
            // local-agent harnesses (e.g. `mae-agent-cli`) that want to declare an
            // AI provider trusted for `LocalModelsOnly` KBs. The primary socket
            // above is untouched (no PSK, same behavior as always) — every
            // existing MCP client (Claude Code CLI via `mae-mcp-shim`, etc.) is
            // completely unaffected by this. The PSK is a fresh per-process
            // secret, written 0600 so only this OS user's processes can read it
            // (the same trust boundary SECURITY.md already documents for the
            // plain tool socket — this does not raise or lower it).
            let agent_socket_path = format!("/tmp/mae-{}-agent.sock", std::process::id());
            let psk_path = format!("/tmp/mae-{}.psk", std::process::id());
            let psk = mae_mcp::auth::generate_psk();
            match mae_mcp::keystore::write_secure(std::path::Path::new(&psk_path), &psk) {
                Ok(()) => {
                    let mut agent_server = mae_mcp::McpServer::new(
                        &agent_socket_path,
                        mcp_tool_tx,
                        sync_broadcaster.clone(),
                    )
                    .with_psk_auth(mae_mcp::auth::PskAuth::new(&psk));
                    if let Some(ref instructions) = mcp_instructions {
                        agent_server = agent_server.with_instructions(instructions.clone());
                    }
                    tokio::spawn(agent_server.run(mcp_tools));
                    info!(socket = %agent_socket_path, psk_file = %psk_path, "MCP agent (PSK-required) server started");
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %psk_path, "failed to write PSK file — agent socket not started");
                }
            }
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
            mcp_client_mgr,
            sync_broadcaster,
        )
    });

    editor.ai.configured = ai_command_tx.is_some();

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
            return gui_app::run_gui(
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
                mcp_client_mgr,
                sync_broadcaster,
            );
        }
    }

    // Set up collab bridge channels (no runtime needed yet).
    let (mut collab_event_rx, collab_command_tx, collab_spawn) =
        collab_bridge::setup_collab_channels(&editor);

    // Terminal path: run the async event loop on the main thread.
    // Spawn collab task inside block_on where tokio runtime is active.
    info!("entering terminal event loop");
    rt.block_on(async {
        collab_bridge::spawn_collab_task(collab_spawn);
        run_terminal_loop(
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
            &mut collab_event_rx,
            &collab_command_tx,
            &mcp_socket_path,
            &all_tools,
            &permission_policy,
            &app_config,
            &mcp_client_mgr,
            &sync_broadcaster,
        )
        .await
    })?;

    let _ = std::fs::remove_file(&mcp_socket_path);
    info!("mae exited cleanly");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{daemon_version_skew, display_available_from_env, parse_truthy, should_use_gui};

    // --- daemon_version_skew (ADR-035 version-pin) ------------------------

    #[test]
    fn version_skew_matches_is_silent() {
        let status = serde_json::json!({"version": "0.14.2", "uptime_secs": 3});
        assert_eq!(daemon_version_skew("0.14.2", &status), None);
    }

    #[test]
    fn version_skew_mismatch_warns_with_both_versions() {
        let status = serde_json::json!({"version": "0.13.9"});
        let msg = daemon_version_skew("0.14.2", &status).expect("mismatch must warn");
        assert!(msg.contains("0.13.9"), "names daemon version: {msg}");
        assert!(msg.contains("0.14.2"), "names editor version: {msg}");
    }

    #[test]
    fn version_skew_absent_field_is_silent() {
        // An older daemon predating the version field — nothing to compare.
        let status = serde_json::json!({"uptime_secs": 1});
        assert_eq!(daemon_version_skew("0.14.2", &status), None);
    }

    // --- gui_display_available policy -------------------------------------

    #[test]
    fn ssh_session_has_no_display_regardless_of_platform() {
        // A remote shell never gets the GUI, even on macOS or with X11/Wayland.
        assert!(!display_available_from_env(true, false, false, true));
        assert!(!display_available_from_env(true, true, true, false));
    }

    #[test]
    fn local_macos_session_has_a_display() {
        assert!(display_available_from_env(false, false, false, true));
    }

    #[test]
    fn linux_needs_x11_or_wayland() {
        // Headless (no DISPLAY/WAYLAND_DISPLAY) → no display.
        assert!(!display_available_from_env(false, false, false, false));
        // X11 present.
        assert!(display_available_from_env(false, true, false, false));
        // Wayland present.
        assert!(display_available_from_env(false, false, true, false));
    }

    // --- should_use_gui decision ------------------------------------------

    #[test]
    fn never_gui_when_not_compiled_in() {
        assert!(!should_use_gui(false, false, false, true));
        assert!(!should_use_gui(false, false, true, true));
    }

    #[test]
    fn force_tui_always_wins() {
        // -nw / --no-gui / --tui beats both --gui and an available display.
        assert!(!should_use_gui(true, true, false, true));
        assert!(!should_use_gui(true, true, true, true));
    }

    #[test]
    fn force_gui_overrides_missing_display() {
        // The MAE.app launcher passes --gui; honor it even if detection is
        // conservative.
        assert!(should_use_gui(true, false, true, false));
    }

    #[test]
    fn default_follows_display_availability() {
        // No flags: GUI iff a display is available.
        assert!(should_use_gui(true, false, false, true));
        assert!(!should_use_gui(true, false, false, false));
    }

    /// The auto-connect footgun fix: a boolean env var must be read by VALUE, not
    /// presence — `MAE_COLLAB_AUTO_CONNECT=false` (or `0`/empty) must disable.
    #[test]
    fn parse_truthy_reads_value_not_presence() {
        for t in ["1", "true", "TRUE", "yes", "on", "anything", " true "] {
            assert!(parse_truthy(t), "{t:?} should be truthy");
        }
        for f in ["0", "false", "FALSE", "no", "off", "", "  ", " off "] {
            assert!(!parse_truthy(f), "{f:?} should be falsy");
        }
    }
}
