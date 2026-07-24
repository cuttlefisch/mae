//! Early CLI flag / subcommand dispatch, extracted from `main()`.
//!
//! Each `handle_*` function checks its own trigger condition against `args`
//! and, if matched, performs the flag's/subcommand's full behavior (which
//! usually terminates the process one way or another — either via
//! `std::process::exit` or by returning `Some(..)` for `main()` to `return`
//! directly). If the trigger doesn't match, the function is a no-op and
//! returns control to the caller so the next flag can be checked. `main()`
//! calls these in the same order the original inline `if` chain checked them
//! — order matters (e.g. `upgrade` must be routed before the greedy global
//! `--help` check, so `mae upgrade --help` prints upgrade-specific usage).
//!
//! Pure code motion from `main.rs` (ADR none needed) — see
//! `.claude/commands/mae-audit.md` / ROADMAP.md "Architecture Debt".

use std::io;

use mae_core::Editor;
use mae_scheme::SchemeRuntime;

use crate::bootstrap::load_init_file;
use crate::{
    agents, apply_collab_launch_overrides, collab_bridge, config, doctor, init_collab_kb_client_id,
    pkg, test_runner, upgrade, DaemonControlClient, BUILD_SHA,
};

pub(crate) fn handle_version(args: &[String]) -> Option<io::Result<()>> {
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("mae {} ({})", env!("CARGO_PKG_VERSION"), BUILD_SHA);
        return Some(Ok(()));
    }
    None
}

/// `mae upgrade` owns its own flags (incl. `--help`), so it must be routed
/// before the greedy global `--help` check — otherwise `mae upgrade --help`
/// would print the global help instead of the upgrade-specific usage.
pub(crate) fn handle_upgrade(args: &[String]) {
    if args.get(1).is_some_and(|a| a == "upgrade") {
        std::process::exit(upgrade::run_upgrade_cli(&args[2..]));
    }
}

pub(crate) fn handle_help(args: &[String]) -> Option<io::Result<()>> {
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
        println!("  --debug                 Enable debug mode (RSS/CPU/frame time in status bar)");
        println!("  --setup-agents [DIR]    Write .mcp.json & agent settings for discovery");
        println!("  --check-config          Validate init.scm + config.toml and exit (for CI)");
        println!("  --check-config --report Print configuration health report and exit");
        println!("  --debug-init            Verbose init file loading (show errors in *Messages*)");
        println!("  -q, --clean             Skip config, init.scm, and history (like emacs -q)");
        println!("  --self-test [CATS]      Run AI self-test headless, exit with pass/fail code");
        println!("  --headless              Run the full engine (KB/AI/LSP/DAP/MCP), no UI, until SIGTERM/Ctrl-C (ADR-055)");
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
        return Some(Ok(()));
    }
    None
}

pub(crate) fn handle_pkg(args: &[String]) {
    if args.get(1).is_some_and(|a| a == "pkg") {
        let code = pkg::cli::run_pkg_cli(&args[2..]);
        std::process::exit(code);
    }
}

/// Flat top-level subcommands (Doom-style): `mae sync`, `mae purge`, etc.
/// (`upgrade` is handled earlier so it owns its own `--help`.)
pub(crate) fn handle_flat_subcommands(args: &[String]) {
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
}

pub(crate) fn handle_doctor(args: &[String]) {
    if args.iter().any(|a| a == "doctor" || a == "--doctor") {
        let code = doctor::run_doctor();
        std::process::exit(code);
    }
}

pub(crate) fn handle_print_config_path(args: &[String]) -> Option<io::Result<()>> {
    if args.iter().any(|a| a == "--print-config-path") {
        println!("{}", config::config_path().display());
        return Some(Ok(()));
    }
    None
}

pub(crate) fn handle_print_config_template(args: &[String]) -> Option<io::Result<()>> {
    if args.iter().any(|a| a == "--print-config-template") {
        print!("{}", config::default_config_template());
        return Some(Ok(()));
    }
    None
}

/// Pure resolution behind `--headless --print-socket-path`, split out for
/// testability without touching `std::env::current_dir()`/`process::exit` —
/// mirrors `headless_loop.rs`'s own `claim_stable_socket`/
/// `claim_stable_socket_at` split, same reason (CLAUDE.md per-test-fixture
/// isolation discipline: no real-process env/cwd dependence in the tested
/// unit). Reuses `headless_loop::stable_socket_path` verbatim (principle #8)
/// — this is guaranteed to resolve to exactly the same path `mae --headless`
/// itself would claim from the same working directory, never a
/// reimplementation that could silently drift from it.
fn resolve_print_socket_path(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let project_root = mae_core::detect_project_root(cwd);
    crate::headless_loop::stable_socket_path(project_root.as_deref())
}

/// `--headless --print-socket-path`: resolve and print ONLY the stable,
/// project-keyed headless socket path (ADR-055 decision 2) for the current
/// working directory's project, then exit — no bind, no editor bootstrap, no
/// other side effect. This is the single source of truth an external tool
/// (e.g. the "MAE for VS Code" extension, ADR-050 D1/Phase I/#384) can rely
/// on instead of reimplementing the project-hashing scheme itself. Requires
/// `--headless` to also be present, since only the stable-path convention
/// this resolves has any meaning there — a bare `--print-socket-path` alone
/// is not a recognized flag.
pub(crate) fn handle_print_socket_path(args: &[String]) -> Option<io::Result<()>> {
    if !(args.iter().any(|a| a == "--headless") && args.iter().any(|a| a == "--print-socket-path"))
    {
        return None;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("mae: --print-socket-path: failed to read current directory: {e}");
            std::process::exit(1);
        }
    };
    match resolve_print_socket_path(&cwd) {
        Some(path) => {
            println!("{}", path.display());
            Some(Ok(()))
        }
        None => {
            eprintln!(
                "mae: --print-socket-path: no project root detected for {}",
                cwd.display()
            );
            std::process::exit(1);
        }
    }
}

/// `--collab-identity`: print this editor's Ed25519 peer identity (generating
/// it on first use) so an admin can authorize it on the daemon.
pub(crate) fn handle_collab_identity(args: &[String]) -> Option<io::Result<()>> {
    if !args.iter().any(|a| a == "--collab-identity") {
        return None;
    }
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
                Some(Ok(()))
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

/// `mae kb-share-p2p [KB-ID] [--policy P] [--transport T] [--socket PATH]`:
/// ESTABLISH the P2P mesh share (create/expose `kbc:{kb_id}` on the mesh) AND
/// mint a join ticket, printing the ticket to stdout (pipe-friendly). Share
/// first, mint second: a ticket is only joinable once the KB is actually shared
/// (ADR-025 §"Driving surfaces"). The CLI surface of the `kb-share-p2p` command /
/// `(kb-share-p2p)` Scheme primitive / `kb_share_p2p` MCP tool.
pub(crate) fn handle_kb_share_p2p(args: &[String]) {
    if args.get(1).is_none_or(|a| a != "kb-share-p2p") {
        return;
    }
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

/// `mae kb-join <ticket> [--socket PATH]`: queue a P2P join from a "magnet link"
/// via the running daemon. The CLI surface of the `kb-join-p2p` command /
/// `(kb-join-ticket)` Scheme primitive / `kb_join_p2p` MCP tool — same
/// `DaemonClient::join_p2p_ticket` backend (ADR-025 §"Driving surfaces"). The
/// background dialer then connects + pulls the KB once the owner approves.
pub(crate) fn handle_kb_join(args: &[String]) {
    if args.get(1).is_none_or(|a| a != "kb-join") {
        return;
    }
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

/// `mae setup-collab [--server <addr>]`: idempotent one-command key-mode setup.
/// Generates the peer identity (if absent), persists collab key-mode options to
/// init.scm, and prints the `mae-daemon authorize` line for the admin.
pub(crate) fn handle_setup_collab(args: &[String]) -> Option<io::Result<()>> {
    if args.get(1).is_none_or(|a| a != "setup-collab") {
        return None;
    }
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
                match crate::enable_daemon_p2p("default") {
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
                        println!("  Join a shared KB:          mae kb-join <mae://join/…ticket>");
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
            Some(Ok(()))
        }
        None => {
            eprintln!("error: cannot resolve collab dir (set XDG_DATA_HOME or HOME)");
            std::process::exit(1);
        }
    }
}

pub(crate) fn handle_setup_agents(args: &[String]) -> Option<io::Result<()>> {
    if !args.iter().any(|a| a == "--setup-agents") {
        return None;
    }
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
    Some(Ok(()))
}

pub(crate) fn handle_init_config(args: &[String]) -> Option<io::Result<()>> {
    if !args.iter().any(|a| a == "--init-config") {
        return None;
    }
    let result = (|| -> io::Result<()> {
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
        Ok(())
    })();
    Some(result)
}

/// `--check-config`: bootstrap editor + Scheme, load init.scm, exit with status.
/// Useful in CI to validate that init.scm parses and evaluates cleanly.
/// `--check-config --report`: also print a configuration health report.
///
/// The theme is resolved directly via `Theme::load` (rather than through
/// `Editor::set_theme_by_name`, whose failure is only observable via
/// `editor.status_msg`) so a bad theme name is a structurally-detected fatal
/// error here — not a status-string sniff that a differently-worded failure
/// message could silently defeat (it did: `set_theme_by_name`'s failure
/// message doesn't start with "Error in", so `--check-config` used to exit 0
/// on a bad theme name in headless/CI mode; see ROADMAP.md "Known Bugs").
pub(crate) fn handle_check_config(args: &[String]) -> Option<io::Result<()>> {
    if !args.iter().any(|a| a == "--check-config") {
        return None;
    }
    let mut editor = Editor::new();
    let (app_config, _) = config::load_config();
    let mut theme_error: Option<String> = None;
    if let Some(ref theme) = app_config.editor.theme {
        match mae_core::Theme::load(theme, &mae_core::BundledResolver) {
            Ok(t) => editor.theme = t,
            Err(e) => theme_error = Some(format!("failed to load theme '{theme}': {e}")),
        }
    }
    let mut scheme = match SchemeRuntime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mae: scheme runtime init failed: {}", e.message);
            std::process::exit(1);
        }
    };
    let _module_registry = load_init_file(&mut scheme, &mut editor);
    // Check if init.scm set an error status.
    let status = &editor.status_msg;
    let init_error = status.starts_with("Error in");
    let has_error = init_error || theme_error.is_some();
    if init_error {
        eprintln!("mae: {}", status);
    }
    if let Some(ref err) = theme_error {
        eprintln!("mae: {}", err);
    }

    if args.iter().any(|a| a == "--report") {
        // Print configuration health report to stdout.
        match mae_ai::execute_audit_configuration(&editor) {
            Ok(json) => println!("{}", json),
            Err(e) => eprintln!("mae: report generation failed: {}", e),
        }
    }

    if has_error {
        std::process::exit(1);
    }
    println!("mae: config OK");
    Some(Ok(()))
}

/// `--ensure-guidance-config [--guidance-kb <name>]`: deterministic,
/// non-AI-dependent one-shot setup for the guidance-KB delivery mechanism
/// (K3, post-ship quality pass). Mirrors `--print-socket-path`'s shape —
/// a scriptable primitive an external tool (the "MAE for VS Code"
/// extension's first-activation hook) can call directly, rather than
/// depending on an LLM correctly guessing which of N MCP tools to call for
/// a one-shot setup step (principle #8: reuses the proven `set_option`/
/// `save_option_to_init` `:set-save` persistence path verbatim, never a
/// hand-rolled config writer).
///
/// Set-if-unset only, for both options independently:
/// - `ai_guidance_kb`: if already non-empty (e.g. the shipped init.scm
///   template's default `"MaePractices"`, or a user's own explicit choice),
///   left untouched. If empty and `--guidance-kb <name>` was given, set to
///   that name. If empty and no `--guidance-kb` given, left empty (nothing
///   to default to — printed as a no-op, not an error).
/// - `ai_guidance_export_live_sync`: if not already `true`, set to `true`
///   (ADR-050 D4) so a fresh workspace gets automatic per-session AGENTS.md
///   sync without a manual `kb_export_guidance` call.
///
/// Never overwrites an EXISTING explicit value for either option. Exits 0
/// in every case (nothing here is a hard error worth failing a caller's
/// script over) except genuine I/O failure persisting to init.scm.
pub(crate) fn handle_ensure_guidance_config(args: &[String]) -> Option<io::Result<()>> {
    if !args.iter().any(|a| a == "--ensure-guidance-config") {
        return None;
    }
    let guidance_kb_arg = args
        .iter()
        .position(|a| a == "--guidance-kb")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let mut editor = Editor::new();
    let mut scheme = match SchemeRuntime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("mae: scheme runtime init failed: {}", e.message);
            std::process::exit(1);
        }
    };
    let _module_registry = load_init_file(&mut scheme, &mut editor);

    let mut changes: Vec<String> = Vec::new();

    let current_guidance_kb = editor
        .get_option("ai_guidance_kb")
        .map(|(v, _)| v)
        .unwrap_or_default();
    if current_guidance_kb.is_empty() {
        match &guidance_kb_arg {
            Some(name) => {
                if let Err(e) = editor.set_option("ai_guidance_kb", name) {
                    eprintln!("mae: --ensure-guidance-config: failed to set ai_guidance_kb: {e}");
                    std::process::exit(1);
                }
                match editor.save_option_to_init("ai_guidance_kb") {
                    Ok(_) => changes.push(format!("ai_guidance_kb set to '{name}'")),
                    Err(e) => {
                        eprintln!(
                            "mae: --ensure-guidance-config: failed to persist ai_guidance_kb: {e}"
                        );
                        std::process::exit(1);
                    }
                }
            }
            None => println!(
                "mae: ai_guidance_kb is unset and no --guidance-kb given -- leaving unset \
                 (a fresh install's shipped init.scm template already defaults to \
                 \"MaePractices\"; nothing to do for an existing config with no KB chosen)"
            ),
        }
    } else {
        println!("mae: ai_guidance_kb already set to '{current_guidance_kb}' -- leaving unchanged");
    }

    let live_sync_already_on = editor
        .get_option("ai_guidance_export_live_sync")
        .map(|(v, _)| v == "true")
        .unwrap_or(false);
    if !live_sync_already_on {
        if let Err(e) = editor.set_option("ai_guidance_export_live_sync", "true") {
            eprintln!(
                "mae: --ensure-guidance-config: failed to set ai_guidance_export_live_sync: {e}"
            );
            std::process::exit(1);
        }
        match editor.save_option_to_init("ai_guidance_export_live_sync") {
            Ok(_) => changes.push("ai_guidance_export_live_sync set to true".to_string()),
            Err(e) => {
                eprintln!(
                    "mae: --ensure-guidance-config: failed to persist \
                     ai_guidance_export_live_sync: {e}"
                );
                std::process::exit(1);
            }
        }
    } else {
        println!("mae: ai_guidance_export_live_sync already true -- leaving unchanged");
    }

    if changes.is_empty() {
        println!("mae: guidance config already fully configured, no changes made");
    } else {
        println!("mae: guidance config updated: {}", changes.join(", "));
    }
    Some(Ok(()))
}

/// `--test PATH`: headless Scheme test runner. Always terminates the process
/// (`std::process::exit`) when triggered; returns `Ok(())` untouched when not.
pub(crate) fn handle_test_mode(args: &[String]) -> io::Result<()> {
    let Some(test_pos) = args.iter().position(|a| a == "--test") else {
        return Ok(());
    };
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
    apply_collab_launch_overrides(&mut editor);

    // #166: derive the KB CRDT client_id from the collab identity here too (the
    // interactive path does this) so scenario node edits author under the daemon's
    // expected `derive_kb_client_id(fp, 0)` and don't trip the epoch fence.
    init_collab_kb_client_id(&mut editor);

    // Joined KBs persist to a durable per-instance store under the KB data dir
    // (`kb_register_joined_instance` creates it on demand from `kb.data_dir`). The
    // interactive path sets this during full startup; the `--test` path must too,
    // else a scenario that JOINs a shared KB keeps the synced/decrypted nodes
    // in-memory only and they never reach disk (the collab-e2e oracle reads disk).
    if let Some(dd) = editor.mae_data_dir() {
        if let Ok(kb_dd) = mae_kb::data_dir::KbDataDir::new(&dd) {
            editor.kb.data_dir = Some(kb_dd);
        }
    }

    // P2P control channel: the interactive path wires this (the second DaemonClient at
    // the `set_daemon_control` site below); the `--test` path must too, else a scenario
    // that drives `kb-share-p2p` / `kb-join-p2p` (daemon control-socket RPCs, not the
    // collab TCP stream) fails with "not connected to a daemon". Best-effort — absent a
    // daemon it stays unset and the p2p primitives report their actionable error.
    {
        let socket = editor.kb.daemon_socket.clone();
        let mut control = mae_mcp::daemon_client::DaemonClient::new(&socket);
        if control.connect().is_ok() {
            editor
                .kb
                .set_daemon_control(Some(std::sync::Arc::new(DaemonControlClient(
                    std::sync::Mutex::new(control),
                ))));
        }
    }

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

    std::process::exit(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact property `--print-socket-path`'s whole design rests on
    /// (principle #8): resolving via `resolve_print_socket_path` must never
    /// drift from what `mae --headless` itself would claim for the same
    /// project root — it's a thin wrapper over `headless_loop::
    /// stable_socket_path`, not a parallel reimplementation.
    #[test]
    fn resolve_print_socket_path_matches_headless_loop_stable_path_for_a_real_project() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();

        let resolved = resolve_print_socket_path(tmp.path());
        let expected = crate::headless_loop::stable_socket_path(Some(tmp.path()));

        assert!(resolved.is_some());
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_print_socket_path_is_stable_across_repeated_calls() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();

        let first = resolve_print_socket_path(tmp.path());
        let second = resolve_print_socket_path(tmp.path());
        assert_eq!(first, second);
    }

    /// `handle_print_socket_path` must require BOTH flags together — a bare
    /// `--print-socket-path` (no `--headless`) is not a recognized flag and
    /// must fall through as a no-op (`None`), not attempt resolution.
    #[test]
    fn handle_print_socket_path_requires_both_flags() {
        let only_print = vec!["mae".to_string(), "--print-socket-path".to_string()];
        assert!(handle_print_socket_path(&only_print).is_none());

        let only_headless = vec!["mae".to_string(), "--headless".to_string()];
        assert!(handle_print_socket_path(&only_headless).is_none());

        let neither = vec!["mae".to_string()];
        assert!(handle_print_socket_path(&neither).is_none());
    }
}
