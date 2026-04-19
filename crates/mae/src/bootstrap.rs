use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::Mutex;

use mae_ai::{
    ai_specific_tools, tools_from_registry, AgentSession, AiCommand, AiEvent, ClaudeProvider,
    OpenAiProvider, ProviderConfig,
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
    let dir = state_home.join("mae");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("mae.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    Some((path, Mutex::new(file)))
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
    Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AiEvent>(32);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<AiCommand>(8);

    let config = load_ai_config();

    if let Some(config) = config {
        let provider_name = config.provider_type.clone();
        let model = config.model.clone();
        let budget = config.budget.clone();
        info!(provider = %provider_name, model = %model, "initializing AI provider");
        let provider: Box<dyn mae_ai::AgentProvider> = match provider_name.as_str() {
            "openai" => Box::new(OpenAiProvider::new(config)),
            _ => Box::new(ClaudeProvider::new(config)), // default to Claude
        };

        let tools = {
            let mut t = tools_from_registry(&editor.commands);
            t.extend(ai_specific_tools(&editor.option_registry));
            t
        };

        let session = AgentSession::new(provider, tools, build_system_prompt(), event_tx, cmd_rx)
            .with_budget(model, budget);

        tokio::spawn(session.run());

        (event_rx, Some(cmd_tx))
    } else {
        // No AI configured — event channel exists but nothing sends to it
        (event_rx, None)
    }
}

/// Load the AI provider configuration by layering env vars over the TOML
/// config file (if any) over built-in defaults. See `config.rs` for the
/// precedence details.
pub fn load_ai_config() -> Option<ProviderConfig> {
    let file = crate::config::load_config();
    crate::config::resolve_ai_config(&file)
}

fn build_system_prompt() -> String {
    let base = include_str!("system_prompt.md");
    let mut prompt = base.to_string();

    // Add working directory context
    if let Ok(cwd) = std::env::current_dir() {
        prompt.push_str(&format!("\n\n## Working Directory\n`{}`\n", cwd.display()));
    }

    // Add git status context if in a git repo
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

    prompt
}

/// Load init.scm from standard locations.
pub fn load_init_file(scheme: &mut SchemeRuntime, editor: &mut Editor) {
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
            // Inject editor state so init.scm can reference *buffer-name* etc.
            scheme.inject_editor_state(editor);
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
/// Thin forwarder to `Editor::conversation_mut`; kept as a free function
/// because `main.rs` uses it ergonomically alongside other `bootstrap::`
/// helpers.
pub fn find_conversation_buffer_mut(editor: &mut Editor) -> Option<&mut mae_core::Conversation> {
    editor.conversation_mut()
}

/// Spawn the LSP coordinator task and return (event_rx, command_tx).
///
/// Configures a small set of well-known language servers. Servers are only
/// *launched* lazily on the first `DidOpen` for their language — if the
/// configured binary isn't installed, opening a file of that language will
/// produce a `ServerStartFailed` event but won't block startup.
///
/// Override via environment variables:
///   MAE_LSP_RUST=rust-analyzer
///   MAE_LSP_PYTHON=pylsp
///   MAE_LSP_TYPESCRIPT="typescript-language-server --stdio"
pub fn setup_lsp() -> (
    tokio::sync::mpsc::Receiver<LspTaskEvent>,
    tokio::sync::mpsc::Sender<LspCommand>,
) {
    let mut configs: HashMap<String, LspServerConfig> = HashMap::new();

    insert_if_set(&mut configs, "rust", "MAE_LSP_RUST", "rust-analyzer", &[]);
    insert_if_set(&mut configs, "python", "MAE_LSP_PYTHON", "pylsp", &[]);
    insert_if_set(
        &mut configs,
        "typescript",
        "MAE_LSP_TYPESCRIPT",
        "typescript-language-server",
        &["--stdio"],
    );
    insert_if_set(
        &mut configs,
        "javascript",
        "MAE_LSP_TYPESCRIPT",
        "typescript-language-server",
        &["--stdio"],
    );
    insert_if_set(&mut configs, "go", "MAE_LSP_GO", "gopls", &[]);

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<LspCommand>(64);
    let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<LspTaskEvent>(64);

    info!(languages = configs.len(), "starting LSP task");
    tokio::spawn(run_lsp_task(configs, cmd_rx, evt_tx));

    (evt_rx, cmd_tx)
}

/// Populate `configs[language_id]` using an override env var or a default
/// command, allowing users to point at a custom binary (or a wrapper with
/// additional flags) without rebuilding.
fn insert_if_set(
    configs: &mut HashMap<String, LspServerConfig>,
    language_id: &str,
    env_var: &str,
    default_cmd: &str,
    default_args: &[&str],
) {
    let (command, args) = match std::env::var(env_var) {
        Ok(v) => {
            let mut parts = v.split_whitespace();
            let Some(cmd) = parts.next() else {
                return; // empty value disables the server
            };
            (
                cmd.to_string(),
                parts.map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        }
        Err(_) => (
            default_cmd.to_string(),
            default_args.iter().map(|s| s.to_string()).collect(),
        ),
    };
    configs.insert(
        language_id.to_string(),
        LspServerConfig {
            command,
            args,
            root_uri: None,
        },
    );
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
