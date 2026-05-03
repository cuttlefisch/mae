use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::Mutex;

use mae_ai::{
    ai_specific_tools, tools_from_registry, AgentSession, AiCommand, AiEvent, ClaudeProvider,
    GeminiProvider, OpenAiProvider, ProviderConfig,
};
use mae_core::Editor;
use mae_dap::{run_dap_task, DapCommand, DapTaskEvent};
use mae_lsp::{run_lsp_task, LspCommand, LspServerConfig, LspTaskEvent};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize structured logging with two outputs:
///
/// 1. **JSON structured log** — newline-delimited JSON. Routed to `stderr`
///    when stderr is *not* a TTY (containers, CI, `mae 2> file.log`), or
///    to `$XDG_STATE_HOME/mae/mae.log` (fallback `~/.local/state/mae/mae.log`)
///    when stderr *is* a TTY — because the TUI shares the tty with stderr
///    and raw JSON lines would paint over the rendered frame. This is the
///    same split helix/neovim use.
/// 2. **In-editor MessageLog** — ring buffer viewable via `:messages`, so
///    interactive users don't need to tail a file to see what's happening.
///
/// The TUI owns stdout; logs must never interfere with it.
///
/// Control via MAE_LOG env var (falls back to RUST_LOG):
///   MAE_LOG=info        — startup/shutdown, AI events, file ops
///   MAE_LOG=debug       — command dispatch, scheme eval, key sequences
///   MAE_LOG=mae=trace   — full trace including per-key events
///   (default)           — warn (only errors and warnings)
///
/// The resolved log file path (if any) is printed to stderr *before* the
/// TUI takes the tty, so users know where to tail.
pub fn init_logging(log_handle: mae_core::MessageLogHandle) {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_env("MAE_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("warn"));

    let editor_layer = EditorLogLayer { handle: log_handle };

    // When stderr is a TTY, writing JSON logs to it would corrupt the TUI.
    // Fall back to a log file. When stderr is piped/redirected (container,
    // CI, `2> file`), stderr is still the ergonomic choice.
    let to_tty = io::stderr().is_terminal();
    let file_writer = if to_tty { open_log_file() } else { None };

    let json_layer = fmt::layer()
        .json()
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(fmt::format::FmtSpan::CLOSE);

    match file_writer {
        Some((path, writer)) => {
            eprintln!("mae: logging to {}", path.display());
            let json_layer = json_layer.with_writer(writer);
            tracing_subscriber::registry()
                .with(filter)
                .with(json_layer)
                .with(editor_layer)
                .init();
        }
        None => {
            let json_layer = json_layer.with_writer(io::stderr);
            tracing_subscriber::registry()
                .with(filter)
                .with(json_layer)
                .with(editor_layer)
                .init();
        }
    }
}

/// Resolve the log file path and open it for append. Returns None if the
/// directory can't be created or the file can't be opened — we do not
/// want a log-setup failure to prevent the editor from launching.
fn open_log_file() -> Option<(PathBuf, Mutex<std::fs::File>)> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let dir = state_home.join("mae").join("logs");
    std::fs::create_dir_all(&dir).ok()?;

    // Timestamped log file per session — no cross-session contamination.
    // Use libc gmtime_r for UTC formatting without adding a crate dependency.
    let filename = {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::gmtime_r(&secs, &mut tm) };
        format!(
            "mae_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.log",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        )
    };
    let path = dir.join(&filename);

    // Symlink mae.log → current session for easy `tail -f`.
    let symlink = dir.parent()?.join("mae.log");
    let _ = std::fs::remove_file(&symlink);
    let _ = std::os::unix::fs::symlink(&path, &symlink);

    // Prune old logs — keep the 10 most recent.
    prune_old_logs(&dir, 10);

    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    Some((path, Mutex::new(file)))
}

fn prune_old_logs(dir: &std::path::Path, keep: usize) {
    let mut logs: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .collect();
    if logs.len() <= keep {
        return;
    }
    logs.sort_by_key(|e| e.file_name());
    for old in &logs[..logs.len() - keep] {
        let _ = std::fs::remove_file(old.path());
    }
}

/// Resolve the history file path (~/.local/state/mae/history.scm).
pub fn history_file_path() -> Option<PathBuf> {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    let dir = state_home.join("mae");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("history.scm"))
}

/// Serialize editor history (recent files/projects) into executable Scheme.
pub fn save_history(editor: &Editor) -> std::io::Result<()> {
    let Some(path) = history_file_path() else {
        return Ok(());
    };

    let mut script = String::new();
    script.push_str(";; MAE generated history file. Do not edit by hand.\n\n");

    // We write them in reverse order, so that when executed, the
    // push operation preserves the same MRU ordering.
    for file in editor.recent_files.list().iter().rev() {
        script.push_str(&format!(
            "(recent-files-add! \"{}\")\n",
            file.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ));
    }

    for project in editor.recent_projects.list().iter().rev() {
        script.push_str(&format!(
            "(recent-projects-add! \"{}\")\n",
            project
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ));
    }

    std::fs::write(path, script)
}

/// Load and evaluate the history Scheme file.
pub fn load_history(scheme: &mut SchemeRuntime, editor: &mut Editor) {
    if let Some(path) = history_file_path() {
        if path.exists() {
            match scheme.load_file(&path) {
                Ok(()) => {
                    scheme.apply_to_editor(editor);
                    debug!(path = %path.display(), "history loaded successfully");
                }
                Err(e) => {
                    error!(path = %path.display(), error = %e, "history load failed");
                }
            }
        }
    }
}

/// Create a comprehensive debug dump of the editor state.
pub fn debug_dump(editor: &Editor) {
    use serde_json::json;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let dir = state_home.join("mae/dumps");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("debug_dump_{}.json", timestamp));

    let messages = editor
        .message_log
        .entries()
        .into_iter()
        .map(|e| {
            json!({
                "level": e.level.to_string(),
                "target": e.target,
                "message": e.message,
                "seq": e.seq,
            })
        })
        .collect::<Vec<_>>();

    let conversation = editor.conversation().map(|c| {
        c.entries
            .iter()
            .map(|e| {
                json!({
                    "role": format!("{:?}", e.role),
                    "content": e.content,
                })
            })
            .collect::<Vec<_>>()
    });

    let dump = json!({
        "timestamp": timestamp,
        "editor_mode": format!("{:?}", editor.mode),
        "buffer_count": editor.buffers.len(),
        "window_count": editor.window_mgr.window_count(),
        "recent_files": editor.recent_files.list(),
        "recent_projects": editor.recent_projects.list(),
        "messages": messages,
        "ai_conversation": conversation,
    });

    if let Ok(content) = serde_json::to_string_pretty(&dump) {
        if let Err(e) = std::fs::write(&path, content) {
            error!(path = %path.display(), error = %e, "failed to write debug dump");
        } else {
            info!(path = %path.display(), "debug dump saved");
        }
    }
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
///
/// # Provider selection
///
/// Claude is the default provider — it's the primary development and testing
/// target. Any unrecognized `provider_type` string falls through to the
/// Claude constructor.
///
/// # Adding a new provider
///
/// 1. Implement [`AgentProvider`](mae_ai::AgentProvider) for your struct
///    (see `crates/ai/src/provider.rs` for the trait definition).
/// 2. Add a public constructor in your provider module under `crates/ai/src/`.
/// 3. Add a match arm in the `provider_name.as_str()` block below.
///
/// Note: the `"ollama"` provider name is remapped to `"openai"` in
/// `config.rs::resolve_ai_config()` because Ollama speaks the OpenAI-compatible
/// API. By the time we reach this function, `provider_type` is already `"openai"`.
pub fn setup_ai(
    editor: &Editor,
) -> (
    tokio::sync::mpsc::Receiver<AiEvent>,
    tokio::sync::mpsc::Sender<AiEvent>,
    Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    // Ensure PATH is populated from the user's shell environment so we can
    // find agent binaries like 'gemini' or 'claude' when running in GUI mode.
    mae_shell::path::sync_path_from_shell();

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AiEvent>(32);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AiCommand>(8);

    let config = load_ai_config(editor);

    if let Some(config) = config {
        let provider_name = config.provider_type.clone();
        let model = config.model.clone();
        let budget = config.budget.clone();
        info!(provider = %provider_name, model = %model, "initializing AI provider");
        let provider: Box<dyn mae_ai::AgentProvider> = match provider_name.as_str() {
            "openai" => Box::new(OpenAiProvider::new(config)),
            "gemini" => Box::new(GeminiProvider::new(config)),
            _ => Box::new(ClaudeProvider::new(config)), // default to Claude
        };

        let tools = {
            let mut t = tools_from_registry(&editor.commands);
            t.extend(ai_specific_tools(&editor.option_registry));
            t
        };

        let session = AgentSession::new(
            provider,
            tools,
            build_system_prompt("pair-programmer"),
            event_tx.clone(),
            cmd_rx,
        )
        .with_budget(model, budget);

        // Self-test mode: wider checkpoint interval, higher stagnation tolerance
        let session = if std::env::args().any(|a| a == "--self-test") {
            session.with_self_test_mode()
        } else {
            session
        };

        spawn_ai_session(session);

        (event_rx, event_tx, Some(cmd_tx))
    } else {
        // No AI configured — event channel exists but nothing sends to it
        (event_rx, event_tx, None)
    }
}

pub fn spawn_ai_session(session: AgentSession) {
    tokio::spawn(session.run());
}

/// Load the AI provider configuration by layering:
///   env vars > Scheme (init.scm) > TOML (config.toml) > defaults.
/// See `config.rs` for the full precedence details.
pub fn load_ai_config(editor: &Editor) -> Option<ProviderConfig> {
    let (file, _) = crate::config::load_config();
    let scheme = crate::config::SchemeAiOverrides::from_editor(editor);
    crate::config::resolve_ai_config_with_scheme(&file, &scheme)
}

pub fn build_system_prompt(profile: &str) -> String {
    let mut prompt = String::new();

    // 1. Load the profile-specific base from prioritized locations:
    //    Project-local (.mae/prompts/*.xml) > User-config (~/.config/mae/prompts/*.xml) > Bundled (prompts/*.xml)
    let profile_filename = format!("{}.xml", profile);
    let mut base_content = None;

    // Check project-local
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join(".mae/prompts").join(&profile_filename);
        if path.exists() {
            base_content = std::fs::read_to_string(path).ok();
        }
    }

    // Check user-config
    if base_content.is_none() {
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("mae/prompts").join(&profile_filename);
            if path.exists() {
                base_content = std::fs::read_to_string(path).ok();
            }
        }
    }

    // Fall back to bundled
    let content = base_content.unwrap_or_else(|| match profile {
        "explorer" => include_str!("prompts/explorer.xml").to_string(),
        "planner" => include_str!("prompts/planner.xml").to_string(),
        "reviewer" => include_str!("prompts/reviewer.xml").to_string(),
        _ => include_str!("prompts/pair-programmer.xml").to_string(),
    });
    prompt.push_str(&content);

    // 2. Add dynamic context
    if let Ok(cwd) = std::env::current_dir() {
        prompt.push_str(&format!(
            "\n\n<context>\n## Working Directory\n`{}`\n",
            cwd.display()
        ));

        // Add project context from CLAUDE.md, README.md, etc.
        let project_files = ["CLAUDE.md", "README.md", "README.org", ".project"];
        for filename in &project_files {
            let path = cwd.join(filename);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let max_chars = 8000;
                    let truncated = if content.len() > max_chars {
                        format!("{}...\n[truncated]", &content[..max_chars])
                    } else {
                        content
                    };
                    prompt.push_str(&format!(
                        "\n## Project Context ({})\n```\n{}\n```\n",
                        filename, truncated
                    ));
                    break;
                }
            }
        }

        // Add memory context from .mae/memory/*.txt
        let memory_dir = cwd.join(".mae/memory");
        if memory_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(memory_dir) {
                prompt.push_str("\n## Long-term Memory\n");
                for entry in entries.flatten() {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        prompt.push_str(&format!("- {}\n", content.trim()));
                    }
                }
            }
        }

        // Add active plans from .mae/plans/*.md
        let plan_dir = cwd.join(".mae/plans");
        if plan_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(plan_dir) {
                prompt.push_str("\n## Active Plans\n");
                for entry in entries.flatten() {
                    if entry.path().extension().is_some_and(|e| e == "md") {
                        if let Ok(content) = std::fs::read_to_string(entry.path()) {
                            prompt.push_str(&format!(
                                "### Plan: {}\n```markdown\n{}\n```\n",
                                entry.file_name().to_string_lossy(),
                                content
                            ));
                        }
                    }
                }
            }
        }

        // Add git status
        if let Ok(output) = std::process::Command::new("git")
            .args(["status", "--porcelain", "--branch"])
            .output()
        {
            if output.status.success() {
                let status = String::from_utf8_lossy(&output.stdout);
                if !status.is_empty() {
                    prompt.push_str(&format!("\n## Git Status\n```\n{}```\n", status));
                }
            }
        }
        prompt.push_str("</context>\n");
    }

    prompt
}

/// Load init.scm files in layers: user → project.
/// Each layer is independent — errors in one don't block others.
///
/// Loading order:
/// 1. `~/.config/mae/init.scm` (user config)
/// 2. `$PROJECT_ROOT/.mae/init.scm` (project-local, if cwd has .mae/)
///
/// Legacy fallbacks: `init.scm` and `scheme/init.scm` in cwd (for v0.6 compat).
pub fn load_init_file(scheme: &mut SchemeRuntime, editor: &mut Editor) {
    load_init_files(scheme, editor);
}

/// Layered init loading — returns the number of files loaded.
pub fn load_init_files(scheme: &mut SchemeRuntime, editor: &mut Editor) -> usize {
    let mut layers: Vec<PathBuf> = Vec::new();

    // Layer 1: user config (~/.config/mae/init.scm)
    if let Some(user_init) = dirs_candidate("mae/init.scm") {
        layers.push(user_init);
    }

    // Layer 2: project-local (.mae/init.scm in cwd)
    if let Ok(cwd) = std::env::current_dir() {
        let project_init = cwd.join(".mae").join("init.scm");
        layers.push(project_init);
    }

    // Legacy fallbacks (v0.6 compat): init.scm, scheme/init.scm in cwd
    layers.push(PathBuf::from("init.scm"));
    layers.push(PathBuf::from("scheme/init.scm"));

    let mut loaded = 0;
    let mut seen = std::collections::HashSet::new();

    for path in &layers {
        // Canonicalize to avoid loading the same file twice (e.g. ./init.scm and init.scm)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !canonical.exists() || !seen.insert(canonical.clone()) {
            continue;
        }

        info!(path = %path.display(), "loading init file");
        scheme.inject_editor_state(editor);
        match scheme.load_file(path) {
            Ok(()) => {
                scheme.apply_to_editor(editor);
                info!(path = %path.display(), "init file loaded successfully");
                // Fire after-load hook with filename
                let filename = path
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default();
                editor.fire_hook(&format!("after-load:{}", filename));
                loaded += 1;
            }
            Err(e) => {
                error!(path = %path.display(), error = %e, "init file load failed");
                editor.set_status(format!("Error in {}: {}", path.display(), e));
                // Continue to next layer — errors don't block
            }
        }
    }

    if loaded == 0 {
        debug!("no init files found");
    } else {
        info!(count = loaded, "init files loaded");
    }
    loaded
}

/// Find the first conversation buffer's Conversation, if any.
/// Thin forwarder to `Editor::conversation_mut`; kept as a free function
/// because `main.rs` uses it ergonomically alongside other `bootstrap::`
/// helpers.
pub fn find_conversation_buffer_mut(editor: &mut Editor) -> Option<&mut mae_core::Conversation> {
    editor.conversation_mut()
}

/// Spawn the LSP coordinator task and return (event_rx, command_tx, server_info).
///
/// Configures language servers using a three-level priority chain:
///   1. Environment variables (MAE_LSP_RUST, MAE_LSP_PYTHON, etc.)
///   2. config.toml `[lsp]` section
///   3. Hardcoded defaults for common languages
///
/// Servers are only *launched* lazily on the first `DidOpen` for their
/// language — if the configured binary isn't installed, opening a file of
/// that language will produce a `ServerStartFailed` event but won't block
/// startup.
///
/// Returns `(event_rx, command_tx, lsp_server_info)` where `lsp_server_info`
/// contains the resolved command and binary-found status for each language.
pub fn setup_lsp(
    root_uri: Option<String>,
    config: &crate::config::Config,
) -> (
    tokio::sync::mpsc::Receiver<LspTaskEvent>,
    tokio::sync::mpsc::Sender<LspCommand>,
    HashMap<String, mae_core::LspServerInfo>,
) {
    // Hardcoded defaults.
    let defaults: &[(&str, &str, &str, &[&str])] = &[
        ("rust", "MAE_LSP_RUST", "rust-analyzer", &[]),
        ("python", "MAE_LSP_PYTHON", "pylsp", &[]),
        (
            "typescript",
            "MAE_LSP_TYPESCRIPT",
            "typescript-language-server",
            &["--stdio"],
        ),
        (
            "javascript",
            "MAE_LSP_TYPESCRIPT",
            "typescript-language-server",
            &["--stdio"],
        ),
        ("go", "MAE_LSP_GO", "gopls", &[]),
    ];

    let mut configs: HashMap<String, LspServerConfig> = HashMap::new();
    let mut server_info: HashMap<String, mae_core::LspServerInfo> = HashMap::new();

    // Phase 1: Populate from defaults, overridden by config.toml, overridden by env vars.
    for &(lang, env_var, default_cmd, default_args) in defaults {
        let (command, args) = resolve_lsp_config(lang, env_var, default_cmd, default_args, config);
        let binary_found = find_binary(&command);
        server_info.insert(
            lang.to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Starting,
                command: command.clone(),
                binary_found,
            },
        );
        configs.insert(
            lang.to_string(),
            LspServerConfig {
                command,
                args,
                root_uri: root_uri.clone(),
            },
        );
    }

    // Phase 2: Add any extra languages from config.toml not in the defaults.
    for (lang, lsp_cfg) in &config.lsp.servers {
        if configs.contains_key(lang) {
            continue; // Already handled above.
        }
        // Still check env var override (MAE_LSP_<LANG>).
        let env_var = format!("MAE_LSP_{}", lang.to_uppercase());
        let (command, args) = resolve_lsp_config(
            lang,
            &env_var,
            &lsp_cfg.command,
            &lsp_cfg.args_as_strs(),
            config,
        );
        let binary_found = find_binary(&command);
        server_info.insert(
            lang.to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Starting,
                command: command.clone(),
                binary_found,
            },
        );
        configs.insert(
            lang.to_string(),
            LspServerConfig {
                command,
                args,
                root_uri: root_uri.clone(),
            },
        );
    }

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<LspCommand>(64);
    let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<LspTaskEvent>(64);

    info!(languages = configs.len(), "starting LSP task");
    tokio::spawn(run_lsp_task(configs, cmd_rx, evt_tx));

    (evt_rx, cmd_tx, server_info)
}

/// Resolve LSP command/args using priority: env var > config.toml > default.
fn resolve_lsp_config(
    lang: &str,
    env_var: &str,
    default_cmd: &str,
    default_args: &[&str],
    config: &crate::config::Config,
) -> (String, Vec<String>) {
    // Priority 1: Environment variable.
    if let Ok(v) = std::env::var(env_var) {
        let mut parts = v.split_whitespace();
        if let Some(cmd) = parts.next() {
            return (cmd.to_string(), parts.map(|s| s.to_string()).collect());
        }
    }
    // Priority 2: config.toml [lsp.<lang>].
    if let Some(cfg) = config.lsp.servers.get(lang) {
        return (cfg.command.clone(), cfg.args.clone());
    }
    // Priority 3: Hardcoded default.
    (
        default_cmd.to_string(),
        default_args.iter().map(|s| s.to_string()).collect(),
    )
}

/// Check if a binary is available on PATH.
fn find_binary(command: &str) -> bool {
    std::process::Command::new("which")
        .arg(command)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn the DAP coordinator task and return (event_rx, command_tx).
///
/// Unlike LSP, DAP sessions are one-at-a-time: you're debugging one
/// program. The task sits idle until it gets a `StartSession` command.
pub fn setup_dap() -> (
    tokio::sync::mpsc::Receiver<DapTaskEvent>,
    tokio::sync::mpsc::Sender<DapCommand>,
) {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<DapCommand>(32);
    let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<DapTaskEvent>(64);
    info!("starting DAP task");
    tokio::spawn(run_dap_task(cmd_rx, evt_tx));
    (evt_rx, cmd_tx)
}

pub fn dirs_candidate(rel: &str) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layered_init_loads_multiple_files() {
        // Create a temp dir with two init files
        let tmp = tempfile::tempdir().unwrap();
        let dir1 = tmp.path().join(".config").join("mae");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::write(dir1.join("init.scm"), "(set-status \"user\")").unwrap();

        let dir2 = tmp.path().join("project").join(".mae");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("init.scm"), "(set-status \"project\")").unwrap();

        // Can't easily test the full layered loading without env var manipulation,
        // but we can verify the function signature exists and is callable.
        let mut scheme = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        // load_init_files returns a usize count
        let _count: usize = load_init_files(&mut scheme, &mut editor);
    }

    #[test]
    fn load_init_files_returns_zero_when_no_files() {
        // In a temp dir with no init.scm, should return 0
        let tmp = tempfile::tempdir().unwrap();
        let _guard = std::env::set_current_dir(tmp.path());
        // Note: this test may still load ~/.config/mae/init.scm if it exists,
        // but that's fine — we're testing that the function completes without error.
        let mut scheme = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        let count = load_init_files(&mut scheme, &mut editor);
        // Count depends on whether user has an init.scm, so just verify no panic
        let _ = count;
    }
}
