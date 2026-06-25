mod agents;
mod ai_event_handler;
mod bootstrap;
mod collab_bridge;
mod config;
mod dap_bridge;
mod doctor;
#[cfg(feature = "gui")]
mod gui_event;
mod key_handling;
mod lsp_bridge;
mod manual_kb;
mod mdns_discovery;
pub mod pkg;
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
#[cfg(feature = "gui")]
use mae_ai::{AiCommand, AiEvent};
use mae_core::{Buffer, Editor};
#[cfg(feature = "gui")]
use mae_dap::DapCommand;
use mae_kb::KbStore;
#[cfg(feature = "gui")]
use mae_lsp::LspCommand;
#[cfg(feature = "gui")]
use mae_renderer::Renderer;
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

/// Apply **per-launch** collaboration overrides (env vars, then CLI `--connect`)
/// AFTER the config files (config.toml + init.scm) have been applied, so they
/// take precedence per the standard chain: defaults < config files < env < CLI.
///
/// Before this, `init.scm` (loaded last) could override `--connect` and the env
/// vars were either ignored entirely (GUI/TUI path never read
/// `MAE_COLLAB_AUTO_CONNECT`) or beaten by a later `(set-option! …)` in
/// `init.scm`. Env: `MAE_COLLAB_SERVER` (address), `MAE_COLLAB_AUTO_CONNECT`
/// (parsed truthy/falsy). CLI `--connect` wins last.
fn apply_collab_launch_overrides(editor: &mut Editor, connect_addr: Option<&str>) {
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
    if let Some(addr) = connect_addr {
        let _ = editor.set_option("collab_server_address", addr);
        let _ = editor.set_option("collab_auto_connect", "true");
        info!(address = %addr, "CLI --connect: auto-connect enabled");
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
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mae {} ({})", env!("CARGO_PKG_VERSION"), BUILD_SHA);
        return Ok(());
    }
    // `mae upgrade` owns its own flags (incl. `--help`), so route it before the
    // greedy global `--help` check below — otherwise `mae upgrade --help` would
    // print the global help instead of the upgrade-specific usage.
    if args.get(1).is_some_and(|a| a == "upgrade") {
        std::process::exit(upgrade::run_upgrade_cli(&args[2..]));
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
        println!("  --collab-identity       Print this editor's collab peer identity (for `mae-daemon authorize`)");
        println!("  setup-collab [--server ADDR] [--ssh-key PATH] [--p2p]");
        println!("                          One-command key-mode setup: identity + init.scm (--p2p also enables the daemon mesh)");
        println!("  kb-share-p2p [KB-ID] [--socket PATH]");
        println!("                          Mint a P2P join ticket (mae://join/…) via the daemon and print it");
        println!("  kb-join <ticket> [--socket PATH]");
        println!("                          Queue a P2P join from a mae://join/… ticket (the dialer pulls the KB)");
        println!("  --gui                   Force GUI backend (default on a desktop session; auto-off over SSH/tty)");
        println!("  --no-gui, --tui, -nw    Force terminal mode (like emacs -nw)");
        println!("  --connect [ADDR]        Connect to daemon (like emacsclient -c)");
        println!("  --debug                 Enable debug mode (RSS/CPU/frame time in status bar)");
        println!("  --setup-agents [DIR]    Write .mcp.json & agent settings for discovery");
        println!("  --check-config          Validate init.scm + config.toml and exit (for CI)");
        println!("  --check-config --report Print configuration health report and exit");
        println!("  --debug-init            Verbose init file loading (show errors in *Messages*)");
        println!("  -q, --clean             Skip config, init.scm, and history (like emacs -q)");
        println!("  --self-test [CATS]      Run AI self-test headless, exit with pass/fail code");
        println!("  --test PATH             Run Scheme tests headless (file or directory)");
        println!("  --test-filter PATTERN   Filter tests by name pattern");
        println!("  --test-output FORMAT    Output format: tap (default) | human");
        println!("  sync                    Materialize declared state (clone/update packages)");
        println!("  upgrade                 Upgrade MAE itself (channel-aware) [--check|--yes|--packages]");
        println!("  purge                   Remove packages not declared in init.scm");
        println!("  list                    List all discovered modules");
        println!("  info <NAME>             Show module details");
        println!("  create <NAME>           Scaffold a new module");
        println!("  doctor [NAME]           Validate manifests, check LSP/DAP, AI provider");
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
        println!("  MAE_AI_PERMISSIONS  readonly | write | shell | privileged");
        println!("  MAE_AGENTS_AUTO_MCP=0 Disable auto .mcp.json on terminal spawn");
        println!("  MAE_SKIP_WIZARD=1   Skip the first-run wizard");
        println!("  MAE_LOG / RUST_LOG  tracing filter (e.g. mae=debug)");
        return Ok(());
    }
    if args.get(1).is_some_and(|a| a == "pkg") {
        let code = pkg::cli::run_pkg_cli(&args[2..]);
        std::process::exit(code);
    }
    // Flat top-level subcommands (Doom-style): mae sync, mae purge, etc.
    // (`upgrade` is handled earlier so it owns its own `--help`.)
    if let Some(subcmd) = args.get(1).map(|s| s.as_str()) {
        match subcmd {
            "sync" | "purge" | "prune-shadows" | "list" | "info" | "create" => {
                let rest: Vec<String> = args[2..].to_vec();
                let code = pkg::cli::dispatch_subcmd(subcmd, &rest);
                std::process::exit(code);
            }
            _ => {}
        }
    }
    if args.iter().any(|a| a == "doctor" || a == "--doctor") {
        let code = doctor::run_doctor();
        std::process::exit(code);
    }
    if args.iter().any(|a| a == "--print-config-path") {
        println!("{}", config::config_path().display());
        return Ok(());
    }
    if args.iter().any(|a| a == "--print-config-template") {
        print!("{}", config::default_config_template());
        return Ok(());
    }
    // --collab-identity: print this editor's Ed25519 peer identity (generating
    // it on first use) so an admin can authorize it on the daemon.
    if args.iter().any(|a| a == "--collab-identity") {
        let label = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "mae-editor".to_string());
        match mae_mcp::identity::default_collab_dir() {
            Some(dir) => match mae_mcp::identity::Identity::load_or_generate(&dir, &label) {
                Ok(id) => {
                    println!(
                        "MAE collab peer identity ({}):",
                        dir.join("id_ed25519").display()
                    );
                    println!("  fingerprint: {}", id.fingerprint());
                    println!("  public key:  {}", id.public().to_line());
                    println!();
                    println!("Authorize on the daemon host with:");
                    println!("  mae-daemon authorize {}", id.public().to_line());
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("error: failed to load/generate identity: {e}");
                    std::process::exit(1);
                }
            },
            None => {
                eprintln!("error: cannot resolve collab dir (set XDG_DATA_HOME or HOME)");
                std::process::exit(1);
            }
        }
    }
    // `mae kb-share-p2p [KB-ID] [--policy P] [--transport T] [--socket PATH]`:
    // ESTABLISH the P2P mesh share (create/expose `kbc:{kb_id}` on the mesh) AND
    // mint a join ticket, printing the ticket to stdout (pipe-friendly). Share
    // first, mint second: a ticket is only joinable once the KB is actually shared
    // (ADR-025 §"Driving surfaces"). The CLI surface of the `kb-share-p2p` command /
    // `(kb-share-p2p)` Scheme primitive / `kb_share_p2p` MCP tool.
    if args.get(1).is_some_and(|a| a == "kb-share-p2p") {
        let kb_id = args
            .get(2)
            .filter(|a| !a.starts_with("--"))
            .cloned()
            .unwrap_or_else(|| "default".to_string());
        let flag = |name: &str| -> Option<String> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let policy = flag("--policy");
        let transport = flag("--transport");
        let socket = flag("--socket").unwrap_or_else(|| {
            mae_mcp::daemon_client::default_daemon_socket()
                .to_string_lossy()
                .into_owned()
        });
        let mut client = mae_mcp::daemon_client::DaemonClient::new(&socket);
        if let Err(e) = client.connect() {
            eprintln!(
                "error: cannot reach mae-daemon at {socket}: {e}\n\
                 start it with `mae-daemon` and enable P2P with `mae setup-collab --p2p`."
            );
            std::process::exit(1);
        }
        // 1. Establish the share so there's something to pull.
        match client.share_kb_p2p(&kb_id, transport.as_deref(), policy.as_deref()) {
            Ok(msg) => eprintln!("{msg}"),
            Err(e) => {
                eprintln!("error: kb-share-p2p '{kb_id}' (share step): {e}");
                std::process::exit(1);
            }
        }
        // 2. Mint the join ticket.
        match client.mint_p2p_ticket(&kb_id) {
            Ok(ticket) => {
                // Just the ticket on stdout, so it pipes cleanly (e.g. | qrencode).
                println!("{ticket}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("error: kb-share-p2p '{kb_id}' (mint step): {e}");
                std::process::exit(1);
            }
        }
    }

    // `mae kb-join <ticket> [--socket PATH]`: queue a P2P join from a "magnet link"
    // via the running daemon. The CLI surface of the `kb-join-p2p` command /
    // `(kb-join-ticket)` Scheme primitive / `kb_join_p2p` MCP tool — same
    // `DaemonClient::join_p2p_ticket` backend (ADR-025 §"Driving surfaces"). The
    // background dialer then connects + pulls the KB once the owner approves.
    if args.get(1).is_some_and(|a| a == "kb-join") {
        let ticket = match args.get(2).filter(|a| !a.starts_with("--")) {
            Some(t) => t.clone(),
            None => {
                eprintln!("usage: mae kb-join <mae://join/…ticket> [--socket PATH]");
                std::process::exit(2);
            }
        };
        let socket = args
            .iter()
            .position(|a| a == "--socket")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| {
                mae_mcp::daemon_client::default_daemon_socket()
                    .to_string_lossy()
                    .into_owned()
            });
        let mut client = mae_mcp::daemon_client::DaemonClient::new(&socket);
        if let Err(e) = client.connect() {
            eprintln!(
                "error: cannot reach mae-daemon at {socket}: {e}\n\
                 start it with `mae-daemon` and enable P2P with `mae setup-collab --p2p`."
            );
            std::process::exit(1);
        }
        match client.join_p2p_ticket(&ticket) {
            Ok(msg) => {
                println!("{msg}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("error: kb-join: {e}");
                std::process::exit(1);
            }
        }
    }

    // `mae setup-collab [--server <addr>]`: idempotent one-command key-mode setup.
    // Generates the peer identity (if absent), persists collab key-mode options to
    // init.scm, and prints the `mae-daemon authorize` line for the admin.
    if args.get(1).is_some_and(|a| a == "setup-collab") {
        let server = args
            .iter()
            .position(|a| a == "--server")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| "127.0.0.1:9473".to_string());
        // `--server` is the address this editor CONNECTS to (the daemon's
        // reachable IP). `0.0.0.0` is a *bind* address (the daemon's), never a
        // connect target — catch the common mix-up early.
        if server.starts_with("0.0.0.0") {
            eprintln!(
                "error: --server is the daemon's reachable address to connect TO, not a bind address.\n\
                 '0.0.0.0' is what the DAEMON binds (in daemon.toml) to listen on all interfaces.\n\
                 Use the daemon host's LAN IP (e.g. 192.168.1.10:9473), or 127.0.0.1:9473 on the same machine."
            );
            std::process::exit(2);
        }
        let mut editor = Editor::new();
        for (opt, val) in [
            ("collab_auth_mode", "key"),
            ("collab_server_address", server.as_str()),
            ("collab_auto_connect", "true"),
        ] {
            if let Err(e) = editor.set_option(opt, val) {
                eprintln!("error: set {opt}: {e}");
                std::process::exit(1);
            }
            if let Err(e) = editor.save_option_to_init(opt) {
                eprintln!("error: persist {opt}: {e}");
                std::process::exit(1);
            }
        }
        let label = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "mae-editor".to_string());
        // --ssh-key <path>: reuse an existing OpenSSH Ed25519 key as the identity
        // (opt-in). The matching .pub is authorized on the daemon via
        // `mae-daemon authorize --from-ssh-pub`.
        let ssh_key = args
            .iter()
            .position(|a| a == "--ssh-key")
            .and_then(|i| args.get(i + 1));
        let id = if let Some(ssh_path) = ssh_key {
            match mae_mcp::identity::Identity::import_ssh_private_key(
                std::path::Path::new(ssh_path),
                &label,
            ) {
                Ok(id) => {
                    if let Some(dir) = mae_mcp::identity::default_collab_dir() {
                        if let Err(e) = id.save(&dir) {
                            eprintln!("error: persist identity: {e}");
                            std::process::exit(1);
                        }
                    }
                    println!("✓ imported SSH identity from {ssh_path}");
                    Some(id)
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            mae_mcp::identity::default_collab_dir()
                .and_then(|dir| mae_mcp::identity::Identity::load_or_generate(&dir, &label).ok())
        };
        match id {
            Some(id) => {
                println!("✓ collab key mode configured (init.scm updated):");
                println!("    collab-auth-mode = key");
                println!("    collab-server-address = {server}");
                println!("    collab-auto-connect = true");
                println!();
                println!("Your peer identity:");
                println!("  fingerprint: {}", id.fingerprint());
                println!("  public key:  {}", id.public().to_line());
                println!();
                println!("On the daemon host, authorize this peer:");
                println!("  mae-daemon authorize {}", id.public().to_line());
                println!();
                // --p2p: also flip on the iroh daemon mesh (ADR-025) in the local
                // daemon.toml, so this host's daemon joins the global P2P mesh.
                if args.iter().any(|a| a == "--p2p") {
                    match enable_daemon_p2p("default") {
                        Ok(path) => {
                            println!("✓ P2P mesh enabled in {}:", path.display());
                            println!("    [collab.p2p] enabled = true, relay = \"default\"");
                            println!("    (ensured [collab.auth] mode = \"key\")");
                            println!("  Restart the daemon to apply: `mae-daemon`");
                            println!(
                                "  For a REMOTE daemon, set [collab.p2p] in its daemon.toml instead."
                            );
                            println!();
                            println!("  Share a KB over the mesh:  mae kb-share-p2p <kb-id>");
                            println!(
                                "  Join a shared KB:          mae kb-join <mae://join/…ticket>"
                            );
                            println!();
                        }
                        Err(e) => {
                            eprintln!("error: enabling P2P in daemon.toml: {e}");
                            std::process::exit(1);
                        }
                    }
                }
                println!("Then launch `mae` — it auto-connects; accept the daemon's");
                println!("key on first connect (verify the fingerprint, then press y).");
                return Ok(());
            }
            None => {
                eprintln!("error: cannot resolve collab dir (set XDG_DATA_HOME or HOME)");
                std::process::exit(1);
            }
        }
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
        // Also write init.scm template if it doesn't exist.
        match config::write_init_template(force) {
            Ok(path) => println!("Wrote init.scm to {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {} // fine
            Err(e) => eprintln!("Warning: could not write init.scm: {}", e),
        }
        config::run_wizard()?;
        return Ok(());
    }

    // --check-config: bootstrap editor + Scheme, load init.scm, exit with status.
    // Useful in CI to validate that init.scm parses and evaluates cleanly.
    // --check-config --report: also print a configuration health report.
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
        let _module_registry = load_init_file(&mut scheme, &mut editor);
        // Check if init.scm set an error status
        let status = &editor.status_msg;
        let has_error = status.starts_with("Error in");
        if has_error {
            eprintln!("mae: {}", status);
        }

        if args.iter().any(|a| a == "--report") {
            // Print configuration health report to stdout
            match mae_ai::execute_audit_configuration(&editor) {
                Ok(json) => println!("{}", json),
                Err(e) => eprintln!("mae: report generation failed: {}", e),
            }
        }

        if has_error {
            std::process::exit(1);
        }
        println!("mae: config OK");
        return Ok(());
    }

    // --test PATH: headless Scheme test runner.
    if let Some(test_pos) = args.iter().position(|a| a == "--test") {
        let test_path = args
            .get(test_pos + 1)
            .filter(|a| !a.starts_with('-'))
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("mae: --test requires a PATH argument (file or directory)");
                std::process::exit(2);
            });

        let test_filter = args
            .iter()
            .position(|a| a == "--test-filter")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str());

        let test_output = args
            .iter()
            .position(|a| a == "--test-output")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("tap");

        // Boot editor headless with Scheme runtime.
        let mut editor = Editor::new();
        let (app_config, _) = config::load_config();
        if let Some(ref theme) = app_config.editor.theme {
            editor.set_theme_by_name(theme);
        }
        let mut scheme = match SchemeRuntime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("mae: scheme runtime init failed: {}", e.message);
                std::process::exit(2);
            }
        };

        let _module_registry = load_init_file(&mut scheme, &mut editor);

        // Per-launch collab overrides (env vars) win over init.scm — applied AFTER
        // it, with value parsing (so MAE_COLLAB_AUTO_CONNECT=false disables).
        // `--test` has no `--connect`. See apply_collab_launch_overrides.
        apply_collab_launch_overrides(&mut editor, None);

        // Build a minimal tokio runtime for the collab bridge.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| io::Error::other(e.to_string()))?;

        let (mut collab_event_rx, collab_command_tx, collab_spawn) =
            collab_bridge::setup_collab_channels(&editor);

        let exit_code = rt.block_on(async {
            collab_bridge::spawn_collab_task(collab_spawn);

            // Give the collab bridge a moment to connect if auto-connect is set.
            if editor.collab.auto_connect {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                // Drain initial connection events.
                while let Ok(event) = collab_event_rx.try_recv() {
                    collab_bridge::handle_collab_event(&mut editor, event);
                }
            }

            test_runner::run_scheme_tests(
                &mut editor,
                &mut scheme,
                &mut collab_event_rx,
                &collab_command_tx,
                &test_path,
                test_filter,
                test_output,
            )
            .await
        });

        std::process::exit(exit_code);
    }

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

    // --connect [ADDR]: connect to collab server on startup (emacsclient -c equivalent)
    let connect_addr: Option<String> = {
        let pos = args.iter().position(|a| a == "--connect");
        if let Some(i) = pos {
            let addr = args
                .get(i + 1)
                .filter(|a| !a.starts_with('-'))
                .cloned()
                .unwrap_or_else(|| mae_core::DEFAULT_COLLAB_ADDRESS.to_string());
            Some(addr)
        } else {
            None
        }
    };

    // Find the first positional argument (not a flag), skipping --connect's address arg.
    let connect_pos = args.iter().position(|a| a == "--connect");
    let file_arg = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, a)| !a.starts_with('-') && connect_pos.is_none_or(|ci| *i != ci + 1))
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
            editor.project_list = mae_core::ProjectList::load(&data_dir);

            // Prune stale entries (nonexistent dirs, temp dirs) and notify.
            let pruned = editor.project_list.prune_stale();
            if !pruned.is_empty() {
                let _ = editor.project_list.save(&data_dir);
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
    if let Some(ref theme) = app_config.editor.theme {
        editor.set_theme_by_name(theme);
    }
    if let Some(ref art) = app_config.editor.splash_art {
        editor.splash_art = Some(art.clone());
    }
    if let Some(ref cmd) = app_config.ai.editor {
        editor.ai.editor_name = cmd.clone();
    }
    if let Some(restore) = app_config.editor.restore_session {
        editor.restore_session = restore;
    }
    if let Some(interval) = app_config.editor.autosave_interval {
        editor.autosave_interval = interval;
    }

    // Apply org agenda files from config.
    if !app_config.org.agenda_files.is_empty() {
        editor.org_agenda_files = app_config.org.agenda_files.clone();
        editor.ingest_agenda_files();
    }

    // Apply font settings from config early (init.scm can override).
    if let Some(size) = app_config.editor.font_size {
        editor.gui_font_size = size;
        editor.gui_font_size_default = size;
    }
    if let Some(ref family) = app_config.editor.font_family {
        editor.gui_font_family = family.clone();
    }
    if let Some(ref icon_family) = app_config.editor.icon_font_family {
        editor.gui_icon_font_family = icon_family.clone();
    }

    // Apply collaboration settings from config → OptionRegistry.
    if let Some(ref addr) = app_config.collaboration.server_address {
        let _ = editor.set_option("collab_server_address", addr);
    }
    if let Some(auto) = app_config.collaboration.auto_connect {
        let _ = editor.set_option("collab_auto_connect", &auto.to_string());
    }
    if let Some(auto) = app_config.collaboration.auto_share {
        let _ = editor.set_option("collab_auto_share", &auto.to_string());
    }
    if let Some(secs) = app_config.collaboration.reconnect_interval_secs {
        let _ = editor.set_option("collab_reconnect_interval", &secs.to_string());
    }
    if let Some(ref name) = app_config.collaboration.user_name {
        let _ = editor.set_option("collab_user_name", name);
    }
    if let Some(secs) = app_config.collaboration.heartbeat_interval_secs {
        let _ = editor.set_option("collab_heartbeat_interval", &secs.to_string());
    }
    if let Some(ref cmd) = app_config.collaboration.psk_command {
        let _ = editor.set_option("collab_psk_command", cmd);
    }
    if let Some(ref key) = app_config.collaboration.psk {
        let _ = editor.set_option("collab_psk", key);
    }
    if let Some(ref mode) = app_config.collaboration.kb_sync_mode {
        let _ = editor.set_option("collab_kb_sync_mode", mode);
    }

    // Auto-derive collab user name if not set via config.
    if editor.collab.user_name.is_empty() {
        let (resolved, source) = resolve_collab_user_name();
        info!(name = %resolved, source = %source, "collab identity resolved");
        let _ = editor.set_option("collab_user_name", &resolved);
    }

    // ADR-020 B-16: derive this peer's STABLE, UNIQUE yrs client_id for KB CRDT
    // edits from the durable collab identity fingerprint. Two peers sharing a
    // client_id collide in yrs' clock space and their concurrent edits diverge;
    // seeding from the per-install Ed25519 fingerprint makes every peer distinct
    // and stable across restarts (so a peer's edits chain on one lineage).
    if editor.collab.local_kb_client_id == 0 {
        if let Some(dir) = mae_mcp::identity::default_collab_dir() {
            let label = editor.collab.user_name.clone();
            if let Ok(id) = mae_mcp::identity::Identity::load_or_generate(&dir, &label) {
                let fp = id.fingerprint();
                let cid = mae_core::editor::derive_kb_client_id(&fp, 0);
                editor.collab.local_kb_client_id = cid;
                // ADR-023: remember our own principal so node edits can be re-derived
                // under a rotated per-KB authorization epoch (see kb_client_id_for).
                editor.collab.local_fingerprint = fp;
                info!(
                    client_id = cid,
                    "KB CRDT client_id derived from collab identity"
                );
            }
        }
    }

    // NB: per-launch collab overrides (env + CLI `--connect`) are applied AFTER
    // init.scm loads (below), so they win over config files — see
    // `apply_collab_launch_overrides`. (They used to be set here, before init.scm,
    // which let a `(set-option! "collab_auto_connect" …)` in init.scm clobber the
    // CLI flag and ignore the env var.)

    // Apply daemon settings from config → OptionRegistry.
    if let Some(enabled) = app_config.daemon.enabled {
        let _ = editor.set_option("daemon_enabled", &enabled.to_string());
    }
    if let Some(ref socket) = app_config.daemon.socket {
        let _ = editor.set_option("daemon_socket", socket);
    }
    if let Some(size) = app_config.daemon.cache_size {
        let _ = editor.set_option("daemon_cache_size", &size.to_string());
    }

    // Apply performance thresholds from config.
    if let Some(v) = app_config.performance.large_file_lines {
        editor.large_file_lines = v;
    }
    if let Some(v) = app_config.performance.degrade_threshold_chars {
        editor.degrade_threshold_chars = v;
    }
    if let Some(v) = app_config.performance.degrade_threshold_line_length {
        editor.degrade_threshold_line_length = v;
    }
    if let Some(v) = app_config.performance.display_region_debounce_ms {
        editor.display_region_debounce_ms = v;
    }
    if let Some(v) = app_config.performance.syntax_reparse_debounce_ms {
        editor.syntax_reparse_debounce_ms = v;
    }

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
    // defaults < config files < env < CLI. Must run AFTER init.scm.
    apply_collab_launch_overrides(&mut editor, connect_addr.as_deref());

    // Load KB federation registry and import enabled instances.
    if !clean_mode {
        // XDG-first (CLAUDE.md principle #13 / B-6): honor XDG_DATA_HOME, then
        // ~/.local/share — NOT dirs::data_dir() (macOS ~/Library). This MUST
        // match editor.mae_data_dir() (where ADR-019 persists the shared-KB
        // registry markers) or those markers would save + load to different
        // paths and silently fail to survive restart.
        let data_dir = editor.mae_data_dir().unwrap_or_else(|| {
            crate::pkg::paths::data_dir_candidate("mae")
                .unwrap_or_else(|| std::path::PathBuf::from("~/.local/share/mae"))
        });

        // Build an in-memory manual KB so the help system's cozo-backed
        // `KbQueryLayer` can resolve built-in nodes (`index`, command/option
        // help, etc.). It is sourced from the pre-built CozoDB file when found
        // (read-only — we never open the on-disk asset read-write, since sled
        // would write recovery snapshots and dirty a git-tracked asset or drift
        // an install's checksum), otherwise from the code-generated seed nodes
        // already in `editor.kb.primary`. Without a manual cozo, `SPC h h` fails
        // with "no such KB node: index".
        match mae_kb::CozoKbStore::open_mem() {
            Ok(mem_store) => {
                let mut sourced_from_prebuilt = false;
                if let Some(result) = manual_kb::locate_and_validate(&data_dir, None) {
                    match &result.validation {
                        manual_kb::ManualValidation::Valid => {
                            debug!(path = %result.path.display(), "manual KB checksum valid");
                        }
                        manual_kb::ManualValidation::Historical { matched_version } => {
                            warn!(
                                path = %result.path.display(),
                                matched = %matched_version,
                                current = env!("CARGO_PKG_VERSION"),
                                "manual KB is from an older mae version"
                            );
                        }
                        manual_kb::ManualValidation::Unknown => {
                            warn!(
                                path = %result.path.display(),
                                "manual KB checksum does not match any known release"
                            );
                        }
                        manual_kb::ManualValidation::Custom => {
                            info!(path = %result.path.display(), "using custom manual KB");
                        }
                    }
                    match manual_kb::load_nodes_readonly(&result.path) {
                        Ok(nodes) => {
                            let count = nodes.len();
                            for node in &nodes {
                                editor.kb.primary.insert(node.clone());
                                if let Err(e) = mem_store.insert_node(node) {
                                    warn!(error = %e, id = %node.id, "failed to load manual node");
                                }
                            }
                            info!(count, path = %result.path.display(), "loaded manual KB nodes (read-only)");
                            sourced_from_prebuilt = true;
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to read pre-built manual KB; falling back to seed");
                        }
                    }
                }

                if !sourced_from_prebuilt {
                    // No usable pre-built KB: seed the in-memory manual cozo from
                    // the code-generated nodes already present in `kb.primary`.
                    match mem_store.persist_nodes(&editor.kb.primary) {
                        Ok(count) => {
                            info!(
                                count,
                                "built in-memory manual KB from seed (no pre-built KB found)"
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to persist seed nodes to in-memory manual KB");
                        }
                    }
                }

                let _ = mem_store.seed_type_system();
                let _ = mem_store.seed_typed_relationships();
                let _ = mem_store.seed_views();
                editor.kb.manual_cozo = Some(std::sync::Arc::new(mem_store));
            }
            Err(e) => {
                warn!(error = %e, "failed to open in-memory manual KB store");
            }
        }

        // Initialize standardized KB data directory layout (XDG-compliant).
        match mae_kb::data_dir::KbDataDir::new(&data_dir) {
            Ok(kb_data_dir) => {
                // Migrate old scattered layout to new structure if needed.
                match mae_kb::data_dir::migrate_legacy_layout(&data_dir) {
                    Ok(0) => {}
                    Ok(n) => info!(
                        count = n,
                        "migrated legacy KB instances to new data directory layout"
                    ),
                    Err(e) => warn!(error = %e, "failed to migrate legacy KB layout"),
                }
                // Phase D3 (ADR-029): if opted in and a local daemon already hosts the
                // primary KB, take the THIN startup path — skip the O(n) `load_all`
                // mirror preload. Reads resolve via the daemon (LRU layer, wired below);
                // the store handle is still opened so the durable pending queue, the
                // lazy single-node load on edit, and the daemon-less fallback keep
                // working. We force `daemon_enabled` so the read LRU is wired. On any
                // probe failure we fall through to the full local init (unchanged).
                let daemon_hosts_primary = editor.kb.daemon_default
                    && probe_daemon_hosts_primary(&editor.kb.daemon_socket);
                if daemon_hosts_primary {
                    editor.kb.daemon_enabled = true;
                    info!(
                        "Phase D3: local daemon hosts the primary KB — thin startup \
                         (skipping the mirror preload; reads via daemon, lazy load on edit)"
                    );
                }

                // Initialize primary KB store (CozoDB) for user data.
                let kb_root = kb_data_dir.root();
                let cozo_path = kb_root.join("primary.cozo");
                match mae_kb::CozoKbStore::open(&cozo_path) {
                    Ok(store) => {
                        if let Err(e) = store.seed_type_system() {
                            warn!(error = %e, "failed to seed KB type system");
                        }
                        match store.seed_typed_relationships() {
                            Ok(n) => debug!(count = n, "seeded typed KB relationships"),
                            Err(e) => {
                                warn!(error = %e, "failed to seed typed relationships")
                            }
                        }
                        if let Err(e) = store.seed_views() {
                            warn!(error = %e, "failed to seed KB views");
                        }

                        // Load user nodes into the in-memory mirror — UNLESS the daemon
                        // hosts the primary (Phase D3): skip the bulk preload; nodes load
                        // lazily from this open store on edit (`kb_ensure_node_loaded`).
                        if daemon_hosts_primary {
                            info!("Phase D3: mirror preload skipped (daemon-hosted primary)");
                        } else {
                            match store.load_all() {
                                Ok(user_nodes) if !user_nodes.is_empty() => {
                                    let count = user_nodes.len();
                                    for node in user_nodes {
                                        editor.kb.primary.insert(node);
                                    }
                                    debug!(count, "loaded user KB nodes from primary store");
                                }
                                Ok(_) => {} // empty store, nothing to load
                                Err(e) => {
                                    warn!(error = %e, "failed to load user nodes from primary store");
                                }
                            }
                        }

                        info!(path = %cozo_path.display(), "primary KB store opened (CozoDB)");
                        let arc_store = std::sync::Arc::new(store);
                        editor.kb.primary_cozo = Some(arc_store.clone());
                        editor.kb.store = Some(arc_store);
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to open CozoDB KB store");
                    }
                }
                editor.kb.data_dir = Some(kb_data_dir);
            }
            Err(e) => {
                warn!(error = %e, "failed to initialize KB data directory");
            }
        }

        // Migrate kb-registry.toml from config → data (v0.9.0)
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
            .join("mae");
        let old_registry = config_dir.join("kb-registry.toml");
        let new_registry = data_dir.join("kb-registry.toml");
        if old_registry.exists() && !new_registry.exists() {
            let _ = std::fs::create_dir_all(&data_dir);
            if std::fs::rename(&old_registry, &new_registry).is_ok() {
                info!("migrated kb-registry.toml from config to data directory");
            }
        }
        let registry = mae_kb::federation::KbRegistry::load(&data_dir);
        for inst in &registry.instances {
            if !inst.enabled {
                continue;
            }
            // ADR-020: load from the durable CozoDB store FIRST when present — this
            // works for collab-JOINED instances whose `org_dir` is empty (they carry
            // a real `db_path`). Previously gated on `org_dir.exists()`, so joined
            // instances were skipped ("dir missing") and lost their nodes (B-10).
            let loaded_via_cozo = if inst.db_path.exists() {
                match mae_kb::CozoKbStore::open(&inst.db_path) {
                    Ok(store) => match store.load_all() {
                        Ok(nodes) => {
                            let count = nodes.len();
                            let mut kb = mae_kb::KnowledgeBase::new();
                            for node in nodes {
                                kb.insert(node);
                            }
                            info!(name = %inst.name, nodes = count, shared = inst.shared, "KB instance loaded from CozoDB");
                            editor.kb.instances.insert(inst.uuid.clone(), kb);
                            editor
                                .kb
                                .instance_stores
                                .insert(inst.uuid.clone(), std::sync::Arc::new(store));
                            true
                        }
                        Err(e) => {
                            warn!(error = %e, name = %inst.name, "CozoDB load_all failed, falling back to org import");
                            false
                        }
                    },
                    Err(e) => {
                        warn!(error = %e, name = %inst.name, "CozoDB open failed, falling back to org import");
                        false
                    }
                }
            } else {
                false
            };
            if loaded_via_cozo {
                // done
            } else if inst.org_dir.exists() {
                let (kb, report, _health) = mae_kb::federation::import_org_dir(&inst.org_dir);
                info!(
                    name = %inst.name,
                    nodes = report.nodes_imported,
                    skipped = report.nodes_skipped,
                    errors = report.errors.len(),
                    "KB instance loaded from org files"
                );
                editor.kb.instances.insert(inst.uuid.clone(), kb);
            } else {
                warn!(name = %inst.name, db = %inst.db_path.display(), "KB instance has no loadable store or org dir, skipping");
            }
        }
        editor.kb.registry = registry;

        // ADR-020 recovery: reconstruct shared-KB instances present on disk but
        // MISSING from the registry (e.g. a clobbered registry — the exact failure
        // that lost a joined KB mid-session). Collect candidates first (immutable
        // borrow of data_dir), then reconstruct (mutable). Idempotent.
        let recoveries: Vec<(String, String, std::path::PathBuf, Option<String>)> =
            if let Some(dd) = editor.kb.data_dir.as_ref() {
                dd.list_shared_kbs()
                    .into_iter()
                    .filter_map(|slug| {
                        let meta = dd.read_shared_meta(&slug)?;
                        Some((
                            meta.collab_id,
                            meta.name,
                            dd.shared_kb_db(&slug),
                            meta.last_sync,
                        ))
                    })
                    .collect()
            } else {
                Vec::new()
            };
        let mut recovered_any = false;
        for (collab_id, name, db_path, last_sync) in recoveries {
            if collab_id.is_empty()
                || editor.kb.registry.find_by_collab_id(&collab_id).is_some()
                || !db_path.exists()
            {
                continue;
            }
            if let Ok(store) = mae_kb::CozoKbStore::open(&db_path) {
                if let Ok(nodes) = store.load_all() {
                    let uuid = mae_kb::federation::generate_uuid();
                    let mut kb = mae_kb::KnowledgeBase::new();
                    for node in nodes {
                        kb.insert(node);
                    }
                    let count = kb.list_ids(None).len();
                    editor.kb.instances.insert(uuid.clone(), kb);
                    editor
                        .kb
                        .instance_stores
                        .insert(uuid.clone(), std::sync::Arc::new(store));
                    editor
                        .kb
                        .registry
                        .instances
                        .push(mae_kb::federation::KbInstance {
                            uuid,
                            name: if name.is_empty() {
                                collab_id.clone()
                            } else {
                                name.clone()
                            },
                            org_dir: std::path::PathBuf::new(),
                            db_path,
                            primary: false,
                            enabled: true,
                            last_import: None,
                            collab_id: Some(collab_id.clone()),
                            shared: true,
                            remote_peers: Vec::new(),
                            last_sync,
                        });
                    recovered_any = true;
                    info!(kb = %collab_id, nodes = count, "recovered shared KB instance from disk (registry rescan)");
                }
            }
        }
        if recovered_any {
            if let Err(e) = editor.kb.registry.save(&data_dir) {
                warn!(error = %e, "failed to persist recovered shared-KB registry");
            }
        }

        // Build the CozoDB-first query layer AFTER all stores are loaded
        // (primary + manual + federated instances).
        editor.kb.rebuild_query_layer();

        // ADR-019: warm the shared-KB sync cache from durable markers at startup
        // so a restarted editor's broadcast gate + status reflect what syncs (the
        // re-subscribe to RECEIVE happens on the Connected event).
        editor.reconstruct_kb_sync_gate();
    }

    // Optionally connect to mae-daemon for LRU-cached KB access.
    // Falls back gracefully to local sled KB if daemon is unavailable.
    if editor.kb.daemon_enabled {
        let socket = editor.kb.daemon_socket.clone();
        let cache_size = editor.kb.daemon_cache_size;
        let mut client = mae_mcp::daemon_client::DaemonClient::new(&socket);
        match client.connect() {
            Ok(()) => {
                info!(socket = %socket.display(), cache_size, "connected to mae-daemon");
                let lru = mae_kb::lru_query::LruQueryLayer::new(client, cache_size);
                editor
                    .kb
                    .set_daemon_query_layer(Some(std::sync::Arc::new(lru)));

                // Wire a second client as the control channel for P2P lifecycle
                // ops (ticket mint/join) — the first was consumed by the LRU layer.
                let mut control = mae_mcp::daemon_client::DaemonClient::new(&socket);
                match control.connect() {
                    Ok(()) => editor.kb.set_daemon_control(Some(std::sync::Arc::new(
                        DaemonControlClient(std::sync::Mutex::new(control)),
                    ))),
                    Err(e) => {
                        warn!(error = %e, "daemon control channel unavailable (P2P share disabled)")
                    }
                }
            }
            Err(e) => {
                warn!(
                    socket = %socket.display(),
                    error = %e,
                    "daemon unavailable, using local KB"
                );
            }
        }
    }

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
                .map(|t| mae_mcp::protocol::ToolInfo {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: serde_json::to_value(&t.parameters).unwrap_or_default(),
                })
                .collect();
            let server = mae_mcp::McpServer::new(&mcp_socket_path, mcp_tool_tx, sync_broadcaster.clone());
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

// ---------------------------------------------------------------------------
// GUI event loop (Phase 8 M4: run_app + EventLoopProxy)
// ---------------------------------------------------------------------------
//
/// Resolve collaborative user name from available sources.
///
/// Resolution order:
/// 1. `git config user.name`
/// 2. `$USER` environment variable
/// 3. hostname
/// 4. "anonymous"
///
/// Returns `(name, source)` for logging.
fn resolve_collab_user_name() -> (String, &'static str) {
    // 1. git config user.name
    if let Ok(output) = std::process::Command::new("git")
        .args(["config", "user.name"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return (name, "git config");
            }
        }
    }
    // 2. $USER env var
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return (user, "$USER");
        }
    }
    // 3. hostname
    if let Ok(output) = std::process::Command::new("hostname")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return (name, "hostname");
            }
        }
    }
    // 4. fallback
    ("anonymous".to_string(), "fallback")
}

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
    mcp_client_mgr: ai_event_handler::McpClientMgrRef,
    sync_broadcaster: mae_mcp::broadcast::SharedBroadcaster,
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
    renderer.set_window_title(editor.window_title.clone());
    editor.renderer_name = "gui".to_string();
    editor.org_hide_emphasis_markers = app_config.editor.org_hide_emphasis_markers.unwrap_or(false);
    editor.clipboard = "unnamedplus".to_string();

    // Create typed event loop with user events — must happen on main thread
    // before the tokio runtime moves to the background.
    let event_loop = EventLoop::<MaeEvent>::with_user_event()
        .build()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let proxy = event_loop.create_proxy();

    // Set up collab bridge channels (no runtime needed yet — task spawned in bridge_task).
    let (collab_event_rx, collab_command_tx, collab_spawn) =
        collab_bridge::setup_collab_channels(&editor);

    // Shared atomics so the bridge task only sends ticks when relevant.
    let shell_active = Arc::new(AtomicBool::new(false));
    let mcp_active = Arc::new(AtomicBool::new(false));

    // Move the tokio runtime + bridge task to a background thread.
    let shell_active_bg = shell_active.clone();
    let mcp_active_bg = mcp_active.clone();
    std::thread::spawn(move || {
        rt.block_on(async {
            // Spawn collab task inside the tokio runtime.
            collab_bridge::spawn_collab_task(collab_spawn);
            bridge_task(
                proxy,
                ai_event_rx,
                lsp_event_rx,
                dap_event_rx,
                mcp_tool_rx,
                collab_event_rx,
                shell_active_bg,
                mcp_active_bg,
            )
            .await;
        });
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
        collab_command_tx,
        mcp_socket_path,
        app_config,
        mcp_client_mgr,
        sync_broadcaster,
        ctrl_held: false,
        alt_held: false,
        shift_held: false,
        dirty: true,
        cursor_x: 0.0,
        cursor_y: 0.0,
        scroll_accumulator_x: 0.0,
        last_scroll_window: None,
        last_scroll_time: None,
        mouse_pressed: false,
        shell_generations: std::collections::HashMap::new(),
        last_render: std::time::Instant::now(),
        input_dirty: false,
        last_input_time: std::time::Instant::now(),
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
#[allow(clippy::too_many_arguments)]
async fn bridge_task(
    proxy: winit::event_loop::EventLoopProxy<gui_event::MaeEvent>,
    mut ai_rx: tokio::sync::mpsc::Receiver<AiEvent>,
    mut lsp_rx: tokio::sync::mpsc::Receiver<mae_lsp::LspTaskEvent>,
    mut dap_rx: tokio::sync::mpsc::Receiver<mae_dap::DapTaskEvent>,
    mut mcp_rx: tokio::sync::mpsc::Receiver<mae_mcp::McpToolRequest>,
    mut collab_rx: tokio::sync::mpsc::Receiver<collab_bridge::CollabEvent>,
    shell_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mcp_active: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use gui_event::MaeEvent;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    let mut shell_interval = tokio::time::interval(Duration::from_millis(33));
    let mut mcp_interval = tokio::time::interval(Duration::from_millis(500));
    let mut health_interval = tokio::time::interval(Duration::from_secs(30));
    let mut idle_interval = tokio::time::interval(Duration::from_millis(100));

    // Skip the initial immediate tick from each interval.
    shell_interval.tick().await;
    mcp_interval.tick().await;
    health_interval.tick().await;
    idle_interval.tick().await;

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
            Some(ev) = collab_rx.recv() => {
                if proxy.send_event(MaeEvent::CollabEvent(ev)).is_err() { break; }
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
            _ = idle_interval.tick() => {
                let _ = proxy.send_event(MaeEvent::IdleTick);
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
    collab_command_tx: tokio::sync::mpsc::Sender<collab_bridge::CollabCommand>,

    // Config
    mcp_socket_path: String,
    app_config: config::Config,
    mcp_client_mgr: ai_event_handler::McpClientMgrRef,
    sync_broadcaster: mae_mcp::broadcast::SharedBroadcaster,

    // Input state
    ctrl_held: bool,
    alt_held: bool,
    shift_held: bool,
    dirty: bool,
    cursor_x: f64,
    cursor_y: f64,
    scroll_accumulator_x: f64,
    // Per-window inertial scrolling: tracks which window last scrolled
    // and when, so inertia activates in the correct pane.
    last_scroll_window: Option<mae_core::WindowId>,
    last_scroll_time: Option<std::time::Instant>,
    mouse_pressed: bool,

    // Shell generation tracking (dirty-check optimisation — TUI parity)
    shell_generations: std::collections::HashMap<usize, u64>,

    // Frame cap (60fps) + input-pending bypass (Emacs dispnew.c:3254 pattern)
    last_render: std::time::Instant,
    /// Keyboard/mouse input needs immediate visual feedback.
    /// Bypasses the 60fps frame cap so scroll/movement is never delayed.
    input_dirty: bool,
    /// Timestamp of last keyboard/mouse input. Used for idle tick scheduling.
    last_input_time: std::time::Instant,

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
        scheme_lsp_bridge::drain_scheme_lsp_intents(&mut self.editor, &self.scheme);
        lsp_bridge::drain_lsp_intents(&mut self.editor, &self.lsp_command_tx);
        scheme_dap_bridge::drain_scheme_dap_intents(&mut self.editor, &mut self.scheme);
        dap_bridge::drain_dap_intents(&mut self.editor, &self.dap_command_tx);
        collab_bridge::drain_collab_intents(&mut self.editor, &self.collab_command_tx);
        collab_bridge::queue_awareness_update(&mut self.editor);
        collab_bridge::cleanup_stale_awareness(&mut self.editor);

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

        // Rekey binary-owned shell maps after any buffer removals this tick.
        for removed_idx in std::mem::take(&mut self.editor.pending_buffer_removals) {
            mae_core::editor::rekey_after_remove(&mut self.shell_terminals, removed_idx);
            mae_core::editor::rekey_after_remove(&mut self.shell_last_dims, removed_idx);
            mae_core::editor::rekey_after_remove(&mut self.shell_generations, removed_idx);
        }

        // Detect theme changes and update shell terminal colors.
        if self.editor.theme.name != self.last_theme_name {
            self.last_theme_name = self.editor.theme.name.clone();
            shell_lifecycle::update_shell_theme_colors(&self.editor, &self.shell_terminals);
        }

        // Clean up generation tracking for removed shells.
        self.shell_generations
            .retain(|idx, _| self.shell_terminals.contains_key(idx));

        // Process module reload requests.
        let reloads = std::mem::take(&mut self.editor.pending_module_reloads);
        for module_name in reloads {
            if module_name == "__all__" {
                // Full reload pipeline (init → modules → config.scm → default_mode),
                // not modules-only, so `:reload-modules` matches startup (C1/H2).
                bootstrap::reload_everything(&mut self.scheme, &mut self.editor, None);
            } else if let Some(flavor) = module_name.strip_prefix("__flavor:") {
                bootstrap::switch_keymap_flavor(&mut self.scheme, &mut self.editor, flavor);
            } else {
                bootstrap::reload_module(&module_name, &mut self.scheme, &mut self.editor);
            }
        }
    }

    /// Send shutdown commands to AI/LSP/DAP tasks.
    fn shutdown(&mut self) {
        info!("editor shutting down (GUI)");

        // Fire app-exit hook.
        self.editor.fire_hook("app-exit");

        // Phase D3b: snapshot the daemon-hosted primary mirror back to the local
        // store so the per-edit-retired edits land in the daemon-less fallback.
        if self.editor.kb.daemon_hosts_primary() {
            self.editor.kb_snapshot_primary_to_store();
        }

        // Persist history (skipped in clean mode)
        if !self.editor.clean_mode {
            if let Err(e) = bootstrap::save_history(&self.editor) {
                error!(error = %e, "failed to save history");
            }
            // Save persistent project list
            if let Some(data_dir) = self.editor.mae_data_dir() {
                let _ = self.editor.project_list.save(&data_dir);
            }
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
                    scheme: &mut self.scheme,
                    mcp_client_mgr: &self.mcp_client_mgr,
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
                self.editor.ai.input_lock = mae_core::InputLock::McpBusy;
                self.last_mcp_activity = Some(tokio::time::Instant::now());
                let immediate = ai_event_handler::handle_mcp_request(
                    &mut self.editor,
                    mcp_req,
                    &self.all_tools,
                    &self.permission_policy,
                    &self.lsp_command_tx,
                    &mut self.deferred_mcp_reply,
                    &mut self.scheme,
                );
                if immediate && self.deferred_mcp_reply.is_empty() {
                    self.editor.ai.input_lock = mae_core::InputLock::None;
                    self.last_mcp_activity = None;
                }
                // Drain hooks queued by MCP-driven commands (e.g. mode-change).
                key_handling::drain_hook_evals(&mut self.editor, &mut self.scheme);
                // Drain sync updates immediately after MCP-driven edits.
                sync_broadcast::drain_and_broadcast(
                    &mut self.editor,
                    &self.sync_broadcaster,
                    Some(&self.collab_command_tx),
                );
                self.dirty = true;
            }
            MaeEvent::ShellTick => {
                // Only check generations if we're not already waiting to render.
                // This prevents redraw stacking when shell output streams faster
                // than the frame budget allows.
                if !self.dirty {
                    for (idx, term) in &self.shell_terminals {
                        let gen = term.generation();
                        if self.shell_generations.get(idx) != Some(&gen) {
                            self.shell_generations.insert(*idx, gen);
                            self.dirty = true;
                            break; // One dirty is enough
                        }
                    }
                }
            }
            MaeEvent::McpIdleTick => {
                if let Some(ts) = self.last_mcp_activity {
                    if ts.elapsed() > std::time::Duration::from_millis(500)
                        && self.deferred_mcp_reply.is_empty()
                    {
                        if self.editor.ai.input_lock == mae_core::InputLock::McpBusy {
                            self.editor.set_status("MCP: input unlocked");
                        }
                        self.editor.ai.input_lock = mae_core::InputLock::None;
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
                // Rekey after health_check zombie cleanup.
                for removed_idx in std::mem::take(&mut self.editor.pending_buffer_removals) {
                    mae_core::editor::rekey_after_remove(&mut self.shell_terminals, removed_idx);
                    mae_core::editor::rekey_after_remove(&mut self.shell_last_dims, removed_idx);
                    mae_core::editor::rekey_after_remove(&mut self.shell_generations, removed_idx);
                }
                // Autosave check (piggybacks on 30s health tick).
                self.editor.try_autosave();
            }
            MaeEvent::CollabEvent(collab_event) => {
                collab_bridge::handle_collab_event(&mut self.editor, collab_event);
                self.dirty = true;
            }
            MaeEvent::IdleTick => {
                if self.last_input_time.elapsed() > std::time::Duration::from_millis(100) {
                    self.editor.idle_work();
                    // Don't set dirty — idle work shouldn't trigger redraws.
                }
                // Drain sync updates on idle tick (~100ms max latency for keyboard edits).
                sync_broadcast::drain_and_broadcast(
                    &mut self.editor,
                    &self.sync_broadcaster,
                    Some(&self.collab_command_tx),
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
            WindowEvent::Focused(true) => {
                // Check if current buffer's file changed on disk
                let idx = self.editor.active_buffer_idx();
                if self.editor.mini_dialog.is_none() {
                    self.editor.check_and_reload_buffer(idx);
                }
                self.dirty = true;
            }
            WindowEvent::Resized(size) => {
                self.renderer.handle_resize(size.width, size.height);
                if let Ok((w, h)) = self.renderer.size() {
                    self.editor.last_layout_area = mae_core::WinRect {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h.saturating_sub(2),
                    };
                }
                self.dirty = true;
            }
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                self.ctrl_held = state.control_key();
                self.alt_held = state.alt_key();
                self.shift_held = state.shift_key();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Track modifier keys directly from KeyboardInput events.
                // On some Wayland compositors (GNOME), ModifiersChanged may
                // arrive AFTER KeyboardInput, causing shift_held to be stale.
                // Tracking from physical key press/release fixes this.
                use winit::keyboard::{Key as WinitKey, NamedKey};
                match &event.logical_key {
                    WinitKey::Named(NamedKey::Shift) => {
                        self.shift_held = event.state == winit::event::ElementState::Pressed;
                    }
                    WinitKey::Named(NamedKey::Control) => {
                        self.ctrl_held = event.state == winit::event::ElementState::Pressed;
                    }
                    WinitKey::Named(NamedKey::Alt) => {
                        self.alt_held = event.state == winit::event::ElementState::Pressed;
                    }
                    _ => {}
                }

                // Bare modifier keys don't dispatch commands — skip dirty/frame.
                if matches!(
                    &event.logical_key,
                    WinitKey::Named(
                        NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super
                    )
                ) {
                    return;
                }

                // Only process non-release events for actual key dispatch.
                if event.state != winit::event::ElementState::Pressed {
                    return;
                }

                self.dirty = true;
                self.input_dirty = true;
                self.last_input_time = std::time::Instant::now();
                self.editor.last_edit_time = std::time::Instant::now();
                self.editor.clear_highlights();
                // Cancel inertial scrolling on any key input.
                self.last_scroll_window = None;
                self.last_scroll_time = None;
                for win in self.editor.window_mgr.iter_windows_mut() {
                    win.inertia_active = false;
                    win.scroll_velocity = 0.0;
                    win.scroll_samples.clear();
                }
                // Default to CursorOnly redraw for keyboard input. Commands that
                // modify text or change mode escalate via mark_full_redraw() or
                // mark_scrolled() internally. This avoids full syntax recomputation
                // on every keypress (scroll, cursor move).
                self.editor.mark_cursor_moved();
                if let Some(mae_core::InputEvent::Key(kp)) = mae_gui::winit_event_to_input(
                    &event,
                    self.ctrl_held,
                    self.alt_held,
                    self.shift_held,
                ) {
                    if self.editor.mini_dialog.is_some() {
                        // A blocking modal (e.g. host-key TOFU prompt) captures input
                        // before AI-input-lock / shell routing — otherwise Esc/Ctrl-C
                        // leak to AI-cancel and the modal is unanswerable (B-22).
                        key_handling::handle_key_from_keypress(
                            &mut self.editor,
                            &mut self.scheme,
                            kp,
                            &mut self.pending_keys,
                            &self.ai_command_tx,
                            &mut self.pending_interactive_event,
                        );
                    } else if self.editor.ai.input_lock != mae_core::InputLock::None {
                        if kp.key == mae_core::Key::Escape
                            || (kp.key == mae_core::Key::Char('c') && kp.ctrl)
                        {
                            self.editor.ai.input_lock = mae_core::InputLock::None;
                            self.editor.ai.streaming = false;
                            self.last_mcp_activity = None;
                            if let Some(ref tx) = self.ai_command_tx {
                                let _ = tx.try_send(AiCommand::Cancel);
                            }
                            if self.editor.cleanup_self_test() {
                                self.editor
                                    .set_status("[AI] Cancelled — self-test state restored");
                            } else {
                                self.editor.set_status("AI operation cancelled");
                            }
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

                        if self.editor.ai.cancel_requested {
                            self.editor.ai.cancel_requested = false;
                            if let Some(ref tx) = self.ai_command_tx {
                                let _ = tx.try_send(AiCommand::Cancel);
                            }
                            self.editor.ai.streaming = false;
                            self.editor.ai.input_lock = mae_core::InputLock::None;
                            self.pending_interactive_event = None;
                            if self.editor.cleanup_self_test() {
                                self.editor
                                    .set_status("[AI] Cancelled — self-test state restored");
                            }
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
                let (cell_w, cell_h) = self.renderer.cell_dimensions();
                if cell_w > 0.0 && cell_h > 0.0 {
                    if self.mouse_pressed {
                        let col = (self.cursor_x / cell_w as f64) as u16;
                        let row = (self.cursor_y / cell_h as f64) as u16;
                        // Drag across windows: switch focus so visual selection extends correctly.
                        self.editor.focus_window_at(col, row);
                        self.editor.handle_mouse_drag(row as usize, col as usize);
                        self.dirty = true;
                    } else if self.editor.mouse_autoselect_window {
                        let col = (self.cursor_x / cell_w as f64) as u16;
                        let row = (self.cursor_y / cell_h as f64) as u16;
                        if self.editor.focus_window_at(col, row) {
                            self.dirty = true;
                        }
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
                    self.last_input_time = std::time::Instant::now();
                    // Cancel inertial scrolling on mouse click.
                    self.last_scroll_window = None;
                    self.last_scroll_time = None;
                    for win in self.editor.window_mgr.iter_windows_mut() {
                        win.inertia_active = false;
                        win.scroll_velocity = 0.0;
                        win.scroll_samples.clear();
                    }
                    let (cell_w, cell_h) = self.renderer.cell_dimensions();
                    if cell_w > 0.0 && cell_h > 0.0 {
                        let col = (self.cursor_x / cell_w as f64) as u16;
                        let row = (self.cursor_y / cell_h as f64) as u16;

                        // Click-to-focus: switch window before dispatching the click.
                        self.editor.focus_window_at(col, row);

                        // Dismiss stale popups on any mouse click.
                        self.editor.lsp.hover_popup = None;
                        self.editor.lsp.code_action_menu = None;

                        // Try pixel-precise positioning via cached FrameLayout
                        // (handles scaled headings and folded lines correctly).
                        let px_x = self.cursor_x as f32;
                        let px_y = self.cursor_y as f32;
                        let focused_id = self.editor.window_mgr.focused_id();
                        let fl = self.renderer.window_layout(focused_id);
                        if let Some(fl) = fl {
                            if let Some((buf_row, char_col)) =
                                fl.pixel_to_buffer_position(px_x, px_y)
                            {
                                self.editor.set_cursor_position(buf_row, char_col);
                                self.dirty = true;
                            } else {
                                self.editor.handle_mouse_click_shift(
                                    row as usize,
                                    col as usize,
                                    mae_button,
                                    self.shift_held,
                                );
                                self.dirty = true;
                            }
                        } else {
                            self.editor.handle_mouse_click_shift(
                                row as usize,
                                col as usize,
                                mae_button,
                                self.shift_held,
                            );
                            self.dirty = true;
                        }
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
                let now = std::time::Instant::now();
                self.last_input_time = now;
                use tracing::debug;

                let cell_h = self.editor.gui_cell_height;
                let (h_px, v_px): (f32, f32) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        debug!(x, y, "MouseWheel: LineDelta");
                        // Convert line deltas to pixel amounts (3 lines per notch).
                        (x * cell_h * 3.0, y * cell_h * 3.0)
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        debug!(pos_x = pos.x, pos_y = pos.y, "MouseWheel: PixelDelta");
                        (pos.x as f32, pos.y as f32)
                    }
                };

                if v_px.abs() > 0.01 {
                    // Determine target window for scroll.
                    let target_win = if self.editor.mouse_wheel_follow_mouse {
                        let (cell_w, cell_h_dim) = self.renderer.cell_dimensions();
                        if cell_w > 0.0 && cell_h_dim > 0.0 {
                            let col = (self.cursor_x / cell_w as f64) as u16;
                            let row = (self.cursor_y / cell_h_dim as f64) as u16;
                            self.editor.window_mgr.window_at_cell(
                                col,
                                row,
                                self.editor.last_layout_area,
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let target_id =
                        target_win.unwrap_or_else(|| self.editor.window_mgr.focused_id());

                    // Push sample to target window and prune old samples (>100ms).
                    if let Some(win) = self.editor.window_mgr.window_mut(target_id) {
                        win.scroll_samples
                            .retain(|(t, _)| now.duration_since(*t).as_secs_f32() < 0.10);
                        win.scroll_samples.push((now, v_px));
                        // Real input overrides inertia in this window.
                        win.inertia_active = false;
                    }
                    self.last_scroll_window = Some(target_id);
                    self.last_scroll_time = Some(now);

                    // Apply pixel delta directly.
                    if target_win.is_some() {
                        self.editor
                            .handle_mouse_scroll_pixels_in_window(target_id, v_px);
                    } else {
                        self.editor.handle_mouse_scroll_pixels(v_px);
                    }
                    self.dirty = true;
                    self.input_dirty = true;
                }

                // Horizontal scroll: keep simple accumulator (no inertia).
                let h_delta = {
                    self.scroll_accumulator_x += h_px as f64;
                    let whole_cols = (self.scroll_accumulator_x / 20.0) as i16;
                    if whole_cols != 0 {
                        self.scroll_accumulator_x -= whole_cols as f64 * 20.0;
                    }
                    whole_cols
                };
                if h_delta != 0 {
                    if self.editor.mouse_wheel_follow_mouse {
                        let (cell_w, cell_h) = self.renderer.cell_dimensions();
                        if cell_w > 0.0 && cell_h > 0.0 {
                            let col = (self.cursor_x / cell_w as f64) as u16;
                            let row = (self.cursor_y / cell_h as f64) as u16;
                            if let Some(target) = self.editor.window_mgr.window_at_cell(
                                col,
                                row,
                                self.editor.last_layout_area,
                            ) {
                                self.editor
                                    .handle_mouse_scroll_horizontal_in_window(target, h_delta);
                            } else {
                                self.editor.handle_mouse_scroll_horizontal(h_delta);
                            }
                        } else {
                            self.editor.handle_mouse_scroll_horizontal(h_delta);
                        }
                    } else {
                        self.editor.handle_mouse_scroll_horizontal(h_delta);
                    }
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
                // Record frame snapshot for perf_profile tool.
                if self.editor.event_recorder.is_recording() {
                    let ps = &self.editor.perf_stats;
                    let snapshot = mae_core::event_record::FrameSnapshot {
                        offset_us: self.editor.event_recorder.duration_us(),
                        frame_time_us: frame_elapsed,
                        total_render_us: ps.total_render_us,
                        render_syntax_us: ps.render_syntax_us,
                        render_layout_us: ps.render_layout_us,
                        render_draw_us: ps.render_draw_us,
                        redraw_level: format!("{:?}", self.editor.redraw_level),
                        scroll_offset: self.editor.window_mgr.focused_window().scroll_offset,
                        syntax_cache_hit: ps.syntax_cache_hits > 0 && ps.syntax_cache_misses == 0,
                        visual_rows_cache_hit: ps.visual_rows_cache_hits > 0
                            && ps.visual_rows_cache_misses == 0,
                    };
                    self.editor.event_recorder.record_frame_snapshot(snapshot);
                }
                self.dirty = false;
                self.editor.clear_redraw();
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

        // Push real cell dimensions so image_extra_rows() matches GUI layout.
        let (cw, ch) = self.renderer.cell_dimensions();
        self.editor.gui_cell_width = cw;
        self.editor.gui_cell_height = ch;

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
                let gutter_w = if !mae_core::BufferMode::has_gutter(&buf.kind) {
                    0
                } else if self.editor.show_line_numbers {
                    mae_renderer::gutter_width(buf.display_line_count())
                } else {
                    2 // marker column + padding
                };
                let scrollbar_w: usize = if self.editor.scrollbar { 1 } else { 0 };
                let text_w = inner_w.saturating_sub(gutter_w).saturating_sub(scrollbar_w);
                self.editor.text_area_width = text_w;
                if !self.editor.word_wrap {
                    self.editor
                        .window_mgr
                        .focused_window_mut()
                        .ensure_scroll_horizontal(text_w);
                }
            }

            {
                // Pre-compute visual rows for the viewport range so the
                // ensure_scroll_wrapped closure doesn't need &self.editor.
                let buf_idx = self.editor.active_buffer_idx();
                let cursor_row = self.editor.window_mgr.focused_window().cursor_row;
                let scroll = self.editor.window_mgr.focused_window().scroll_offset;
                let so = self.editor.scrolloff;
                // Pass tight needed range — populate_visual_rows_cache adds padding internally.
                let cache_start = scroll.min(cursor_row).saturating_sub(1);
                let cache_end = (scroll.max(cursor_row) + vh + 2)
                    .min(self.editor.buffers[buf_idx].display_line_count());
                self.editor
                    .populate_visual_rows_cache(buf_idx, cache_start, cache_end);

                // Snapshot cache Vec<u8> to avoid borrow conflict with window_mgr.
                let (cache_rows, cache_line_start) = {
                    let buf = &self.editor.buffers[buf_idx];
                    match &buf.visual_rows_cache {
                        Some(c) => (c.rows.clone(), c.line_start),
                        None => (Vec::new(), 0),
                    }
                };

                let line_count = self.editor.buffers[buf_idx].display_line_count();
                let win = self.editor.window_mgr.focused_window_mut();
                if win.scroll_locked && win.cursor_row == win.scroll_locked_cursor {
                    // Cursor hasn't moved since scroll command; keep lock active
                } else {
                    win.scroll_locked = false;
                    win.ensure_scroll_wrapped_with_margin(vh, so, line_count, |line| {
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

        // Debounced syntax reparse: drain pending reparses after configured ms idle.
        let reparse_debounce =
            std::time::Duration::from_millis(self.editor.syntax_reparse_debounce_ms);
        if !self.editor.syntax_reparse_pending.is_empty()
            && self.editor.last_edit_time.elapsed() >= reparse_debounce
        {
            mae_core::syntax::drain_pending_reparses(&mut self.editor);
            self.dirty = true;
        }

        // Debounced document highlight: request after 300ms cursor idle.
        if self.editor.lsp.highlight_ranges.is_empty()
            && self.editor.last_edit_time.elapsed() >= std::time::Duration::from_millis(300)
        {
            self.editor.lsp_request_document_highlight();
        }

        // Breadcrumbs: request/refresh on cursor idle.
        if self.editor.show_breadcrumbs {
            self.editor.request_breadcrumb_symbols();
        }

        // Per-window inertial scrolling.
        // Phase 1: Activate inertia after 50ms gap since last real scroll event.
        const MAX_INERTIA_VELOCITY: f32 = 3000.0;
        const MIN_INERTIA_VELOCITY: f32 = 100.0;
        const INERTIA_KILL_THRESHOLD: f32 = 20.0;
        const INERTIA_DECAY: f32 = 0.92;

        if let Some(last) = self.last_scroll_time {
            if last.elapsed().as_secs_f32() > 0.05 {
                if let Some(target_id) = self.last_scroll_window.take() {
                    self.last_scroll_time = None;
                    // Compute velocity from samples: total displacement / total time.
                    if let Some(win) = self.editor.window_mgr.window_mut(target_id) {
                        if win.scroll_samples.len() >= 2 {
                            let first_t = win.scroll_samples.first().unwrap().0;
                            let last_t = win.scroll_samples.last().unwrap().0;
                            let dt = last_t.duration_since(first_t).as_secs_f32();
                            let total_disp: f32 = win.scroll_samples.iter().map(|(_, d)| d).sum();
                            if dt > 0.001 {
                                let velocity = (total_disp / dt)
                                    .clamp(-MAX_INERTIA_VELOCITY, MAX_INERTIA_VELOCITY);
                                if velocity.abs() >= MIN_INERTIA_VELOCITY {
                                    win.inertia_active = true;
                                    win.scroll_velocity = velocity;
                                }
                            }
                        }
                        win.scroll_samples.clear();
                    }
                }
            }
        }

        // Phase 2: Process active inertia windows.
        let any_inertia = {
            // Collect active windows to avoid borrow conflict.
            let active: Vec<(mae_core::WindowId, f32)> = self
                .editor
                .window_mgr
                .iter_windows()
                .filter(|w| w.inertia_active)
                .map(|w| (w.id, w.scroll_velocity))
                .collect();
            let mut any = false;
            for (win_id, velocity) in active {
                let dt = 1.0 / 60.0_f32;
                let delta_px = velocity * dt;
                let moved = self
                    .editor
                    .handle_mouse_scroll_pixels_in_window(win_id, delta_px);
                if let Some(win) = self.editor.window_mgr.window_mut(win_id) {
                    win.scroll_velocity *= INERTIA_DECAY;
                    if win.scroll_velocity.abs() < INERTIA_KILL_THRESHOLD || !moved {
                        win.scroll_velocity = 0.0;
                        win.inertia_active = false;
                    } else {
                        any = true;
                    }
                }
            }
            if any {
                self.dirty = true;
            }
            any
        };

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
        } else if any_inertia || self.last_scroll_time.is_some() {
            // Inertia pending or about to activate — keep 60fps cadence.
            let frame_budget = std::time::Duration::from_micros(16_667);
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                std::time::Instant::now() + frame_budget,
            ));
        } else if !self.editor.syntax_reparse_pending.is_empty() {
            // Pending reparses but not otherwise dirty — wake up when debounce expires.
            let wake_at = self.editor.last_edit_time + reparse_debounce;
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(wake_at));
        } else {
            // Not dirty — sleep until next event (no busy-loop).
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{display_available_from_env, parse_truthy, should_use_gui};

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
