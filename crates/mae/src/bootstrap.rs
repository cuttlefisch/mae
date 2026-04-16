use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use mae_ai::{
    ai_specific_tools, tools_from_registry, AgentSession, AiCommand, AiEvent, ClaudeProvider,
    OpenAiProvider, ProviderConfig,
};
use mae_core::Editor;
use mae_lsp::{run_lsp_task, LspCommand, LspServerConfig, LspTaskEvent};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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
pub fn init_logging(log_handle: mae_core::MessageLogHandle) {
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

pub fn load_ai_config() -> Option<ProviderConfig> {
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
pub fn find_conversation_buffer_mut(editor: &mut Editor) -> Option<&mut mae_core::Conversation> {
    editor
        .buffers
        .iter_mut()
        .find_map(|b| b.conversation.as_mut())
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
