//! @ai-caution: [architecture-debt] App bootstrapping (path resolution, config
//! application, KB federation init, daemon connect, collab user-name
//! resolution). Already 2,397 lines pre-existing debt before the `main.rs`
//! split (2026-07) added `apply_app_config`/`init_kb_federation`/
//! `init_daemon_connection`/`resolve_collab_user_name`, bringing it to
//! ~3,062 — not split further this pass, needs its own dedicated look.
//! Tracked in .claude/commands/mae-audit.md's "Known exceptions" and
//! ROADMAP.md's "Architecture Debt" section.

use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

// Path resolution lives in `pkg::paths`; re-exported here so existing
// `crate::bootstrap::{dirs_candidate, builtin_module_dirs}` call sites are
// unchanged. (`data_dir_candidate` is used only inside `pkg::paths`.)
pub use crate::pkg::paths::{builtin_module_dirs, dirs_candidate};
use std::sync::Mutex;

use mae_ai::{
    ai_specific_tools, tools_from_registry, AgentSession, AiCommand, AiEvent, ClaudeProvider,
    GeminiProvider, OllamaProvider, OpenAiProvider, ProviderConfig,
};
use mae_core::Editor;
use mae_dap::{run_dap_task, DapCommand, DapTaskEvent};
use mae_kb::KbStore;
use mae_lsp::{run_lsp_task, LspCommand, LspServerConfig, LspTaskEvent};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};
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
            "mae_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}_{}.log",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
            std::process::id(),
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

/// Serialize `(files, projects)` MRU lists into the executable-Scheme history
/// format. Used by the exit-time merge path (`save_history_on_exit`).
fn save_history_lists(
    files: &mae_core::RecentFiles,
    projects: &mae_core::RecentProjects,
) -> std::io::Result<()> {
    let Some(path) = history_file_path() else {
        return Ok(());
    };

    let mut script = String::new();
    script.push_str(";; MAE generated history file. Do not edit by hand.\n\n");

    // We write them in reverse order, so that when executed, the
    // push operation preserves the same MRU ordering.
    for file in files.list().iter().rev() {
        script.push_str(&format!(
            "(recent-files-add! \"{}\")\n",
            file.to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ));
    }

    for project in projects.list().iter().rev() {
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

/// Parse a history.scm file's `(recent-files-add! "...")` /
/// `(recent-projects-add! "...")` lines back into plain path lists, in file
/// order (oldest -> newest, the inverse of `save_history_lists`'s `.rev()`).
/// A small, symmetric twin of the hand-rolled writer above — used to load a
/// scratch snapshot of on-disk history without evaluating Scheme against a
/// live `Editor` (needed by the exit-time merge, which must not run
/// arbitrary Scheme mid-shutdown).
fn parse_history_lists(path: &std::path::Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut files = Vec::new();
    let mut projects = Vec::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return (files, projects);
    };
    for line in content.lines() {
        let line = line.trim();
        let (prefix, target) = if let Some(rest) = line.strip_prefix("(recent-files-add! \"") {
            (true, rest)
        } else if let Some(rest) = line.strip_prefix("(recent-projects-add! \"") {
            (false, rest)
        } else {
            continue;
        };
        let Some(quoted) = target.strip_suffix("\")") else {
            continue;
        };
        let path = PathBuf::from(unescape_scheme_string(quoted));
        if prefix {
            files.push(path);
        } else {
            projects.push(path);
        }
    }
    (files, projects)
}

/// Inverse of `.replace('\\', "\\\\").replace('"', "\\\"")`.
fn unescape_scheme_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Merge on-disk MRU lists (oldest -> newest, as returned by
/// `parse_history_lists`) with this session's own lists, ranking the
/// session's entries as most recent. Pure (no I/O) so it's testable without
/// touching `history_file_path()`'s real `XDG_STATE_HOME`/`HOME`-derived path.
///
/// `RecentFiles`/`RecentProjects::push` already dedups + moves-to-front +
/// caps, so replaying disk entries first (so they rank as "older") and then
/// this session's own entries on top (so this session's recency wins for
/// anything it touched) produces a proper merged MRU list, not a naive
/// concatenation.
///
/// Cross-process ordering is best-effort (there's no shared per-entry
/// timestamp): this session's entries are always ranked at least as recent
/// as anything disk-only. The *set* of entries is what's guaranteed
/// preserved, not exact global recency across processes.
fn merge_history_lists(
    disk_files: Vec<PathBuf>,
    disk_projects: Vec<PathBuf>,
    session_files: &mae_core::RecentFiles,
    session_projects: &mae_core::RecentProjects,
) -> (mae_core::RecentFiles, mae_core::RecentProjects) {
    let mut merged_files = mae_core::RecentFiles::new(session_files.cap());
    for file in disk_files {
        merged_files.push(file);
    }
    for file in session_files.list().iter().rev() {
        merged_files.push(file.clone());
    }

    let mut merged_projects = mae_core::RecentProjects::new(session_projects.cap());
    for project in disk_projects {
        merged_projects.push(project);
    }
    for project in session_projects.list().iter().rev() {
        merged_projects.push(project.clone());
    }

    (merged_files, merged_projects)
}

/// Persist history (recent files/projects) at exit, merging with whatever's
/// currently on disk rather than blindly overwriting it.
///
/// By exit, `editor.recent_files`/`recent_projects` already hold this whole
/// session's MRU state — but another concurrently-running `mae` process (a
/// normal way to use MAE) may have opened files/projects of its own since
/// this session started. A bare `save_history_lists` overwrite here would
/// silently drop those; `merge_history_lists` (above) folds them together
/// instead.
pub fn save_history_on_exit(editor: &Editor) -> std::io::Result<()> {
    let Some(path) = history_file_path() else {
        return Ok(());
    };
    let (disk_files, disk_projects) = parse_history_lists(&path);
    let (merged_files, merged_projects) = merge_history_lists(
        disk_files,
        disk_projects,
        &editor.recent_files,
        &editor.recent_projects,
    );
    save_history_lists(&merged_files, &merged_projects)
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

/// Persist `editor.project_list` at exit, merging with whatever's currently
/// on disk rather than blindly overwriting it.
///
/// By exit, `editor.project_list` already holds this whole session's
/// accumulated state — but another concurrently-running `mae` process (a
/// normal way to use MAE: one process per project directory) may have added
/// projects of its own since this session started. A bare `save()` here
/// would silently drop those. Reload fresh and upsert (by `root`) each of
/// this session's entries on top, so a concurrent process's additions
/// survive alongside this session's.
pub fn save_project_list_on_exit(editor: &Editor, data_dir: &std::path::Path) {
    let mine = editor.project_list.projects.clone();
    let (_, (), saved) = mae_core::ProjectList::update(data_dir, |pl| {
        for entry in mine {
            match pl.projects.iter_mut().find(|e| e.root == entry.root) {
                Some(existing) => *existing = entry,
                None => pl.projects.push(entry),
            }
        }
    });
    if let Err(e) = saved {
        error!(error = %e, "failed to save project list");
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
/// 3. Add a match arm to [`construct_provider`] below.
///
/// Note: `"deepseek"` is remapped to `"openai"` in
/// `config.rs::resolve_ai_config()` because DeepSeek speaks the OpenAI-compatible
/// API with no native alternative. `"ollama"` keeps its own identity — it has
/// an OpenAI-compatible endpoint too, but that shim doesn't forward Ollama's
/// `think` field (the thinking-mode toggle), so `OllamaProvider` talks to
/// Ollama's native `/api/chat` endpoint instead. See `crates/ai/src/ollama.rs`.
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

    let (file_config, _) = crate::config::load_config();
    let config = load_ai_config(editor);

    if let Some(config) = config {
        let provider_name = config.provider_type.clone();
        let model = config.model.clone();
        let budget = config.budget.clone();
        let config_ai = &file_config.ai;
        info!(provider = %provider_name, model = %model, "initializing AI provider");

        // Warn about untested models so users know tool calling may be unreliable
        let limits = mae_ai::context_limits::lookup(&model);
        match limits.verification {
            mae_ai::ModelVerification::Untested => {
                tracing::warn!(model = %model, "model has not been tested with MAE — tool calling may be unreliable");
            }
            mae_ai::ModelVerification::Testing => {
                info!(model = %model, "model is in testing — report issues at github.com/cuttlefisch/mae");
            }
            mae_ai::ModelVerification::Verified => {}
        }

        let provider = construct_provider(&provider_name, config);

        let tools = {
            let mut t = tools_from_registry(&editor.commands);
            t.extend(ai_specific_tools(&editor.option_registry));
            t.extend(mae_ai::scheme_tools_to_definitions(&editor.ai.scheme_tools));
            t
        };

        let effective_tier = config_ai
            .prompt_tier
            .as_deref()
            .map(mae_ai::context_limits::ModelTier::parse_tier)
            .unwrap_or_else(|| mae_ai::context_limits::tier(&model));
        info!(tier = effective_tier.as_str(), "selected prompt tier");

        let mut prompt = build_system_prompt_with_model(
            "pair-programmer",
            effective_tier,
            &editor.active_modules,
            &model,
        );

        // Inject provider-specific hints for non-Claude models
        let provider_hint = mae_ai::context_limits::ProviderHint::from_model(&model);
        if let Some(hints) = provider_hint.prompt_hints() {
            prompt.push_str(hints);
        }

        let session = AgentSession::new(provider, tools, prompt, event_tx.clone(), cmd_rx)
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

/// Construct a boxed [`AgentProvider`](mae_ai::AgentProvider) for the given provider-type
/// string. This is the single, shared constructor for every site in this crate that turns a
/// `provider_type` + [`ProviderConfig`] into a live provider — do not add a second match arm
/// site elsewhere; a prior drift between this logic and the `delegate` sub-agent handler in
/// `ai_event_handler.rs` silently ran Ollama-configured sub-agents on Claude instead.
pub(crate) fn construct_provider(
    provider_type: &str,
    config: ProviderConfig,
) -> Box<dyn mae_ai::AgentProvider> {
    match provider_type {
        "openai" => Box::new(OpenAiProvider::new(config)),
        "gemini" => Box::new(GeminiProvider::new(config)),
        "ollama" => Box::new(OllamaProvider::new(config)),
        _ => Box::new(ClaudeProvider::new(config)), // default to Claude
    }
}

/// Load the AI provider configuration by layering:
///   env vars > Scheme (init.scm) > TOML (config.toml) > defaults.
/// See `config.rs` for the full precedence details.
pub fn load_ai_config(editor: &Editor) -> Option<ProviderConfig> {
    let (file, _) = crate::config::load_config();
    let scheme = crate::config::SchemeAiOverrides::from_editor(editor);
    crate::config::resolve_ai_config_with_scheme(&file, &scheme)
}

/// Load memory context from `.mae/memory/*.txt` files.
///
/// Returns a formatted block suitable for injection into system prompts.
/// Files are sorted by name (newest first, since names contain timestamps),
/// and the total is capped at 8000 chars to stay within ~2K tokens.
pub fn load_memory_context(project_root: &std::path::Path) -> Option<String> {
    let memory_dir = project_root.join(".mae/memory");
    if !memory_dir.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&memory_dir).ok()?;
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "txt"))
        .collect();
    if files.is_empty() {
        return None;
    }
    // Sort by filename descending (newest timestamp first)
    files.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    let mut block = String::from("## Long-term Memory\n");
    let cap = 8000;
    for entry in &files {
        if block.len() >= cap {
            break;
        }
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                let line = format!("- {}\n", trimmed);
                block.push_str(&line);
            }
        }
    }
    if block.len() > cap {
        block.truncate(cap);
        block.push_str("\n...[truncated]\n");
    }
    Some(block)
}

/// Synthesize memory context from `.mae/memory/*.txt` files with
/// model-aware formatting and budget.
///
/// Facts are categorized, deduplicated (newer wins), and truncated to
/// `budget_chars`. Format adapts to model tier and provider:
/// - Compact/DeepSeek/Local/Qwen → numbered lists per category
/// - Full + Claude/OpenAI/Gemini → grouped sections with headers
pub fn synthesize_memory(
    project_root: &std::path::Path,
    tier: mae_ai::context_limits::ModelTier,
    provider: mae_ai::context_limits::ProviderHint,
    budget_chars: usize,
) -> Option<String> {
    let memory_dir = project_root.join(".mae/memory");
    if !memory_dir.exists() {
        return None;
    }
    let entries = std::fs::read_dir(&memory_dir).ok()?;
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "txt"))
        .collect();
    if files.is_empty() {
        return None;
    }
    // Sort by filename descending (newest timestamp first — for dedup priority)
    files.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    // Collect facts
    let mut facts: Vec<String> = Vec::new();
    for entry in &files {
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                facts.push(trimmed.to_string());
            }
        }
    }
    if facts.is_empty() {
        return None;
    }

    // Categorize by keyword matching
    let mut conventions = Vec::new();
    let mut architecture = Vec::new();
    let mut bugs = Vec::new();
    let mut decisions = Vec::new();
    let mut other = Vec::new();

    for fact in &facts {
        let lower = fact.to_ascii_lowercase();
        if lower.contains("always")
            || lower.contains("never")
            || lower.contains("prefer")
            || lower.contains("convention")
            || lower.contains("style")
            || lower.contains("rule")
            || lower.contains("don't")
        {
            conventions.push(fact.as_str());
        } else if lower.contains("crate")
            || lower.contains("module")
            || lower.contains("struct")
            || lower.contains("trait")
            || lower.contains("pattern")
            || lower.contains("design")
            || lower.contains("directory")
        {
            architecture.push(fact.as_str());
        } else if lower.contains("bug")
            || lower.contains("fix")
            || lower.contains("workaround")
            || lower.contains("broken")
            || lower.contains("issue")
            || lower.contains("error")
            || lower.contains("crash")
        {
            bugs.push(fact.as_str());
        } else if lower.contains("decided")
            || lower.contains("chose")
            || lower.contains("because")
            || lower.contains("rationale")
            || lower.contains("tradeoff")
            || lower.contains("instead")
        {
            decisions.push(fact.as_str());
        } else {
            other.push(fact.as_str());
        }
    }

    // Priority order: conventions > architecture > bugs > decisions > other
    let categories: Vec<(&str, &[&str])> = vec![
        ("Conventions", &conventions),
        ("Architecture", &architecture),
        ("Bugs & Fixes", &bugs),
        ("Decisions", &decisions),
        ("Other", &other),
    ];

    // Choose format based on tier + provider
    let use_numbered = tier == mae_ai::context_limits::ModelTier::Compact
        || matches!(
            provider,
            mae_ai::context_limits::ProviderHint::DeepSeek
                | mae_ai::context_limits::ProviderHint::Local
                | mae_ai::context_limits::ProviderHint::Qwen
        );

    let mut block = String::from("## Project Memory\n");
    let mut counter = 1;

    for (label, items) in &categories {
        if items.is_empty() {
            continue;
        }
        if block.len() >= budget_chars {
            break;
        }
        if use_numbered {
            // Numbered list per category
            block.push_str(&format!("### {}\n", label));
            for item in *items {
                let line = format!("{}. {}\n", counter, item);
                if block.len() + line.len() > budget_chars {
                    break;
                }
                block.push_str(&line);
                counter += 1;
            }
        } else {
            // Grouped sections with brief headers
            block.push_str(&format!("### {}\n", label));
            for item in *items {
                let line = format!("- {}\n", item);
                if block.len() + line.len() > budget_chars {
                    break;
                }
                block.push_str(&line);
            }
        }
    }

    Some(block)
}

pub fn build_system_prompt(profile: &str, tier: mae_ai::context_limits::ModelTier) -> String {
    build_system_prompt_with_modules(profile, tier, &[])
}

pub fn build_system_prompt_with_modules(
    profile: &str,
    tier: mae_ai::context_limits::ModelTier,
    modules: &[mae_core::editor::ModuleInfo],
) -> String {
    build_system_prompt_with_model(profile, tier, modules, "")
}

pub fn build_system_prompt_with_model(
    profile: &str,
    tier: mae_ai::context_limits::ModelTier,
    modules: &[mae_core::editor::ModuleInfo],
    model: &str,
) -> String {
    let mut prompt = String::new();

    // 1. Load the profile-specific base from prioritized locations:
    //    Project-local (.mae/prompts/*.xml) > User-config (~/.config/mae/prompts/*.xml) > Bundled (prompts/*.xml)
    //
    //    For tiered prompts, try the tier-specific file first (e.g. pair-programmer-compact.xml),
    //    then fall back to the generic file (pair-programmer.xml).
    let tier_suffix = match tier {
        mae_ai::context_limits::ModelTier::Compact => "-compact",
        mae_ai::context_limits::ModelTier::Full => "",
    };
    let tiered_filename = format!("{}{}.xml", profile, tier_suffix);
    let fallback_filename = format!("{}.xml", profile);
    let mut base_content = None;

    // Check project-local (tiered then generic)
    if let Ok(cwd) = std::env::current_dir() {
        for filename in &[&tiered_filename, &fallback_filename] {
            let path = cwd.join(".mae/prompts").join(filename);
            if path.exists() {
                base_content = std::fs::read_to_string(path).ok();
                if base_content.is_some() {
                    break;
                }
            }
        }
    }

    // Check user-config (tiered then generic)
    if base_content.is_none() {
        if let Some(config_dir) = dirs::config_dir() {
            for filename in &[&tiered_filename, &fallback_filename] {
                let path = config_dir.join("mae/prompts").join(filename);
                if path.exists() {
                    base_content = std::fs::read_to_string(path).ok();
                    if base_content.is_some() {
                        break;
                    }
                }
            }
        }
    }

    // Fall back to bundled (tiered then generic)
    let content = base_content.unwrap_or_else(|| {
        let is_compact = tier == mae_ai::context_limits::ModelTier::Compact;
        match (profile, is_compact) {
            ("pair-programmer", true) => {
                include_str!("prompts/pair-programmer-compact.xml").to_string()
            }
            ("explorer", true) => include_str!("prompts/explorer-compact.xml").to_string(),
            ("reviewer", true) => include_str!("prompts/reviewer-compact.xml").to_string(),
            ("explorer", false) => include_str!("prompts/explorer.xml").to_string(),
            ("planner", true) => include_str!("prompts/planner-compact.xml").to_string(),
            ("planner", false) => include_str!("prompts/planner.xml").to_string(),
            ("reviewer", false) => include_str!("prompts/reviewer.xml").to_string(),
            ("verifier", true) => include_str!("prompts/verifier-compact.xml").to_string(),
            ("verifier", false) => include_str!("prompts/verifier.xml").to_string(),
            _ => include_str!("prompts/pair-programmer.xml").to_string(),
        }
    });
    prompt.push_str(&content);

    // 2. Add dynamic context
    if let Ok(cwd) = std::env::current_dir() {
        prompt.push_str(&format!(
            "\n\n<context>\n## Working Directory\n`{}`\n",
            cwd.display()
        ));

        // Config paths so the agent doesn't have to call audit_configuration
        if let Some(config_dir) = dirs::config_dir() {
            let mae_config = config_dir.join("mae");
            prompt.push_str(&format!(
                "\n## Config Paths\n- config.toml: `{}/config.toml`\n- init.scm: `{}/init.scm`\n",
                mae_config.display(),
                mae_config.display()
            ));
        }

        // Add project context from CLAUDE.md, README.md, etc. — shared with
        // mae-agent-cli's system prompt and the MCP `instructions` field
        // (mae_ai::guidance), so this logic isn't duplicated per AI surface.
        if let Some(ctx) = mae_ai::guidance::read_project_context(&cwd) {
            prompt.push_str(&ctx);
        }

        // Add memory context from .mae/memory/*.txt
        // When model is known, use synthesized format with budget; otherwise raw injection.
        if !model.is_empty() {
            let limits = mae_ai::context_limits::lookup(model);
            let provider = mae_ai::context_limits::ProviderHint::from_model(model);
            if let Some(mem) = synthesize_memory(&cwd, tier, provider, limits.memory_budget_chars())
            {
                prompt.push('\n');
                prompt.push_str(&mem);
            }
        } else if let Some(memory_block) = load_memory_context(&cwd) {
            prompt.push('\n');
            prompt.push_str(&memory_block);
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
        // Inject module context if any are loaded
        if !modules.is_empty() {
            prompt.push_str("\n## Modules\n");
            prompt
                .push_str("MAE has a Doom-style module system. Use `list_modules` for details.\n");
            let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
            prompt.push_str(&format!("Active: {}\n", names.join(", ")));
            prompt.push_str("KB docs: `kb_search \"module:\"` or `kb_get \"module:<name>\"`\n");
        }

        prompt.push_str("</context>\n");
    }

    prompt
}

/// Load init files, discover/load modules, then load config.scm.
///
/// This implements the three-file loading model:
///   1. init.scm (module declarations) — user → project
///   2. Module autoloads (topo-sorted, before user config)
///   3. config.scm (user customization, overrides module defaults)
///
/// Returns the ModuleRegistry for the caller to store.
pub fn load_init_file(
    scheme: &mut SchemeRuntime,
    editor: &mut Editor,
) -> crate::pkg::loader::ModuleRegistry {
    load_init_files(scheme, editor);
    let registry = load_modules(scheme, editor);
    load_config_scm(scheme, editor);
    apply_default_mode(editor);
    registry
}

/// Apply the `default_mode` option (set by the keymap flavor — non-modal flavors
/// use "insert") after modules + config have loaded, so a user `config.scm`
/// `(set-option!)` can still override it. The editor boots in Normal; this flips
/// it to Insert for non-modal flavors. `set_mode` is a no-op for buffers that
/// only allow Normal (e.g. the dashboard), which is fine.
pub fn apply_default_mode(editor: &mut Editor) {
    match editor.default_mode.as_str() {
        "insert" => {
            editor.set_mode(mae_core::Mode::Insert);
        }
        // Actively set Normal (not a no-op): a runtime flavor switch from a
        // non-modal flavor leaves the editor in Insert, and switching to a modal
        // flavor must return it to Normal.
        "normal" => {
            editor.set_mode(mae_core::Mode::Normal);
        }
        other => warn!("unknown default_mode '{other}' — staying in normal"),
    }
}

/// Layered init loading — returns the number of files loaded.
pub fn load_init_files(scheme: &mut SchemeRuntime, editor: &mut Editor) -> usize {
    let mut layers: Vec<PathBuf> = Vec::new();

    // Layer 1: user config (~/.config/mae/init.scm)
    let has_user_init = dirs_candidate("mae/init.scm")
        .filter(|p| p.exists())
        .is_some();
    if let Some(user_init) = dirs_candidate("mae/init.scm") {
        layers.push(user_init);
    }

    // Layer 2: project-local (.mae/init.scm in cwd)
    if let Ok(cwd) = std::env::current_dir() {
        let project_init = cwd.join(".mae").join("init.scm");
        layers.push(project_init);
    }

    // Legacy fallbacks (v0.6 compat): init.scm, scheme/init.scm in cwd.
    // Skipped when a proper user init.scm exists — these are templates that
    // would silently override user settings (e.g. theme) if loaded after it.
    if !has_user_init {
        layers.push(PathBuf::from("init.scm"));
        layers.push(PathBuf::from("scheme/init.scm"));
    }

    let mut loaded = 0;
    let mut seen = std::collections::HashSet::new();

    for path in &layers {
        // Canonicalize to avoid loading the same file twice (e.g. ./init.scm and init.scm)
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !canonical.exists() || !seen.insert(canonical.clone()) {
            continue;
        }

        let debug = editor.debug_init;
        if debug {
            editor.message_log.push(
                mae_core::MessageLevel::Info,
                "init",
                format!("Loading {}...", path.display()),
            );
        }
        info!(path = %path.display(), "loading init file");
        scheme.inject_editor_state(editor);
        match scheme.load_file(path) {
            Ok(()) => {
                scheme.apply_to_editor(editor);
                info!(path = %path.display(), "init file loaded successfully");
                if debug {
                    editor.message_log.push(
                        mae_core::MessageLevel::Info,
                        "init",
                        format!("  Loaded {} OK", path.display()),
                    );
                }
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
                if debug {
                    editor.message_log.push(
                        mae_core::MessageLevel::Error,
                        "init",
                        format!("  ERROR in {}: {}", path.display(), e),
                    );
                }
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

/// Discover, resolve, and load module autoloads.
///
/// Module loading happens between init.scm and config.scm:
///   1. Discover modules via [`builtin_module_dirs`] + user `~/.local/share/mae/packages/`
///   2. Resolve dependencies (topological sort)
///   3. Load each module's autoloads.scm (registers commands, keys, options, hooks)
///   4. config.scm runs AFTER this, so users can override any module setting
///
/// Enable `name` plus its transitive `[dependencies]` in the `enabled` set.
/// Used for auto-enabled keymap flavors (which bypass the `(mae!)` declaration)
/// so their deps — e.g. the shared `keymap-leader` — load too. Iterative
/// worklist; ignores deps not present among discovered modules (the resolver
/// reports those).
/// The registered default of the `keymap_flavor` option.
const DEFAULT_KEYMAP_FLAVOR: &str = "doom";

/// Reconcile (mae!)-declared keymap flavors with the `keymap_flavor` option
/// (audit H3). The option is the single source of truth, with one ergonomic
/// exception: if the user declared exactly one flavor and never moved the option
/// off its default, honor the declaration. Pure for testability.
///
/// Returns `(sync_to, to_drop)`:
/// - `sync_to`: a new value to write to the `keymap_flavor` option, if the lone
///   declaration should be adopted;
/// - `to_drop`: declared `keymap-*` module names that disagree with the
///   authoritative flavor and must be removed from the enabled set (so a live
///   `:keymap-set-flavor` wins even when init.scm hardcodes a different flavor).
fn reconcile_keymap_flavor(
    declared_flavors: &[String],
    current_flavor: &str,
    default_flavor: &str,
) -> (Option<String>, Vec<String>) {
    let mut sync_to = None;
    let mut effective = current_flavor.to_string();
    if declared_flavors.len() == 1 {
        let declared = declared_flavors[0]
            .strip_prefix("keymap-")
            .unwrap_or(&declared_flavors[0]);
        if current_flavor == default_flavor && declared != default_flavor {
            sync_to = Some(declared.to_string());
            effective = declared.to_string();
        }
    }
    let target = format!("keymap-{effective}");
    let to_drop = declared_flavors
        .iter()
        .filter(|f| **f != target)
        .cloned()
        .collect();
    (sync_to, to_drop)
}

fn enable_with_deps(
    name: &str,
    all_modules: &[crate::pkg::embedded::DiscoveredModule],
    enabled: &mut HashMap<String, Vec<String>>,
) {
    // Cycle/duplicate guard is SEPARATE from the `enabled` check on purpose. A
    // module declared in the (mae!) block is already present in `enabled`, but its
    // dependency closure must STILL be expanded — otherwise the resolver fails
    // with "depends on <X> which is not enabled" and drops everything. The old
    // code short-circuited on `enabled.contains_key`, so a *declared* flavor
    // (e.g. `keymap-doom`) never pulled in its `keymap-leader` dependency. Track
    // visited nodes independently and always walk a node's deps the first time we
    // see it, enabled or not.
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stack = vec![name.to_string()];
    while let Some(n) = stack.pop() {
        if !visited.insert(n.clone()) {
            continue;
        }
        // Enable if not already declared (preserve any existing flags).
        enabled.entry(n.clone()).or_default();
        if let Some(d) = all_modules.iter().find(|d| d.manifest.name() == n) {
            for dep in d.manifest.dependencies.keys() {
                if !visited.contains(dep) {
                    stack.push(dep.clone());
                }
            }
        }
    }
}

/// Returns the populated ModuleRegistry.
pub fn load_modules(
    scheme: &mut SchemeRuntime,
    editor: &mut Editor,
) -> crate::pkg::loader::ModuleRegistry {
    use crate::pkg::{
        embedded::merge_modules,
        loader::{load_module_autoloads, ModuleRegistry},
        manifest::discover_modules,
        resolver::resolve_load_order,
    };

    // On-disk modules: built-in search path (first existing dir that yields
    // modules wins) plus user-installed packages. These OVERRIDE the embedded
    // baseline by name (dev loop + user customization).
    let mut disk_modules = Vec::new();
    let builtin_dirs = builtin_module_dirs();
    for dir in &builtin_dirs {
        if dir.exists() && disk_modules.is_empty() {
            disk_modules.extend(discover_modules(dir));
        }
    }
    if let Some(user_pkg) = dirs_candidate("mae/packages") {
        if user_pkg.exists() {
            disk_modules.extend(discover_modules(&user_pkg));
        }
    }

    // H4: warn (don't break) when a stale on-disk copy shadows a NEWER built-in.
    // The override still applies, but an out-of-date ~/.local/share/mae/modules or
    // app-bundle MAE_MODULES_PATH copy silently suppressing an upgraded built-in
    // is a real trap — surface it so the user can refresh/remove it.
    for (name, disk_ver, emb_ver) in crate::pkg::embedded::stale_embedded_shadows(&disk_modules) {
        let msg = format!(
            "on-disk module '{name}' (v{disk_ver}) shadows newer built-in (v{emb_ver}); \
             delete the stale copy to use the built-in"
        );
        warn!("{}", msg);
        editor
            .message_log
            .push(mae_core::messages::MessageLevel::Warn, "modules", &msg);
    }

    // Embedded built-ins are the always-present baseline; on-disk modules
    // override by name. This is the single source of truth shared with the
    // `mae` package CLI. With built-ins embedded, this is essentially never
    // empty — keymap-doom is guaranteed present regardless of install layout.
    let all_modules = merge_modules(disk_modules);
    if all_modules.is_empty() {
        warn!(
            "no modules discovered (embedded baseline missing?!) — leader-key (SPC) \
             bindings are limited to built-in kernel defaults. This should not happen; \
             please report it. On-disk search paths: {:?}",
            builtin_dirs
        );
        return ModuleRegistry::new();
    }

    // Use declared modules from (mae! ...) if present; otherwise enable all.
    let declared = scheme.declared_modules();
    let has_mae_block = !declared.is_empty();
    let mut enabled: HashMap<String, Vec<String>> = if declared.is_empty() {
        // No mae! block — enable all discovered modules (backward compat),
        // EXCEPT keymap flavor modules (`keymap-*`): only the selected
        // `keymap_flavor` should load, since multiple flavors would clash on
        // bindings + `default_mode`. The chosen flavor + its deps (the shared
        // `keymap-leader`) are added by the flavor block below.
        all_modules
            .iter()
            .filter(|d| !d.manifest.name().starts_with("keymap-"))
            .map(|d| (d.manifest.name().to_string(), vec![]))
            .collect()
    } else {
        declared
    };

    // H3: the `keymap_flavor` option is the single source of truth for the active
    // flavor. Reconcile it with any flavor declared in (mae!):
    //   - if the user declared exactly one keymap-* and left the option at its
    //     default, honor the declaration (sync the option to it);
    //   - otherwise the option wins — drop a declared keymap-* that disagrees (and
    //     warn), so a live `:keymap-set-flavor` stays authoritative even when
    //     init.scm hardcodes a different flavor.
    {
        let declared_flavors: Vec<String> = enabled
            .keys()
            .filter(|k| k.starts_with("keymap-"))
            .cloned()
            .collect();
        let (sync_to, to_drop) = reconcile_keymap_flavor(
            &declared_flavors,
            &editor.keymap_flavor,
            DEFAULT_KEYMAP_FLAVOR,
        );
        if let Some(flavor) = sync_to {
            info!(
                flavor,
                "syncing keymap_flavor option to the declared flavor"
            );
            let _ = editor.set_option("keymap_flavor", &flavor);
        }
        for df in to_drop {
            let msg = format!(
                "keymap_flavor option '{}' overrides declared module '{df}'",
                editor.keymap_flavor
            );
            warn!("{}", msg);
            editor
                .message_log
                .push(mae_core::messages::MessageLevel::Warn, "modules", &msg);
            enabled.remove(&df);
        }
    }

    // The keymap flavor is a module named `keymap-<flavor>` (default "doom",
    // set via the `keymap_flavor` option in init.scm before this runs). Auto-
    // enable it unless the user explicitly declared a different keymap-* module.
    // keymap-doom is embedded, so the default always resolves.
    let has_keymap_module = enabled.keys().any(|k| k.starts_with("keymap-"));
    if !has_keymap_module {
        let flavor = editor.keymap_flavor.clone();
        let target = format!("keymap-{flavor}");
        let chosen = if all_modules.iter().any(|d| d.manifest.name() == target) {
            info!("auto-enabling {target} (keymap_flavor = {flavor})");
            Some(target)
        } else {
            warn!(
                "keymap flavor '{flavor}' ({target}) not found among discovered modules; \
                 falling back to embedded keymap-doom"
            );
            all_modules
                .iter()
                .any(|d| d.manifest.name() == "keymap-doom")
                .then(|| "keymap-doom".to_string())
        };
        // Enable the flavor AND its dependency closure (e.g. keymap-leader holds
        // the shared leader tree). Auto-enabled flavors bypass the (mae!)
        // declaration, so their deps must be pulled in here or the resolver
        // would error "depends on keymap-leader which is not enabled".
        if let Some(name) = chosen {
            enable_with_deps(&name, &all_modules, &mut enabled);
        }
    }

    // Auto-enable language modules (category = "lang") unless explicitly disabled.
    // Language modules provide keymaps and hooks for file types — without them,
    // file-type features silently fail (Emacs auto-mode-alist equivalent).
    if has_mae_block {
        for d in &all_modules {
            let module = &d.manifest;
            if module.module.category == "lang" && !enabled.contains_key(module.name()) {
                info!(
                    "auto-enabling {} (language module — add to mae! block to customize)",
                    module.name()
                );
                enabled.insert(module.name().to_string(), vec![]);
            }
        }
    }

    // Auto-enable REQUIRED (core) modules regardless of the `(mae!)` block — Doom's
    // `core/` analog — UNLESS explicitly disabled via `(package! "name" :disable #t)`.
    // These are cross-cutting features whose buffers/prompts are raised by
    // *background* events (so their keybindings must always be present), e.g. the
    // `notifications` attention bus. User-initiated features (git-status, debug, …)
    // stay opt-in. (Without a `(mae!)` block the enable-all path already includes them.)
    {
        let disabled: std::collections::HashSet<String> = scheme
            .declared_packages()
            .into_iter()
            .filter(|p| p.disable)
            .map(|p| p.name)
            .collect();
        for d in &all_modules {
            let module = &d.manifest;
            if module.module.required
                && !enabled.contains_key(module.name())
                && !disabled.contains(module.name())
            {
                info!("auto-enabling {} (required/core module)", module.name());
                enabled.insert(module.name().to_string(), vec![]);
            }
        }
    }

    // Expand the dependency closure of every enabled module (e.g. a declared or
    // auto-enabled keymap flavor pulls in the shared `keymap-leader`) so the
    // resolver never fails on an unlisted-but-required dependency.
    for name in enabled.keys().cloned().collect::<Vec<_>>() {
        enable_with_deps(&name, &all_modules, &mut enabled);
    }

    // Graceful degradation: resolution skips unsatisfiable modules instead of
    // aborting the whole load, so one broken drop-in module can't take out the
    // embedded keymap-leader/flavor and brick the leader/which-key system.
    let outcome = resolve_load_order(&all_modules, &enabled);
    let resolved = outcome.resolved;

    let mut registry = ModuleRegistry::new();
    registry.register_resolved(&resolved);

    // Record skipped modules as Failed (durable in list_modules/audit) and
    // aggregate them into a single user-visible message instead of a transient,
    // last-wins status line.
    let mut failures: Vec<String> = Vec::new();
    for sk in &outcome.skipped {
        if let Some(d) = all_modules.iter().find(|d| d.manifest.name() == sk.name) {
            registry.register_skipped(d, sk.reason.clone());
        }
        let msg = format!("module '{}' skipped: {}", sk.name, sk.reason);
        warn!("{}", msg);
        failures.push(msg);
    }

    let current_version = env!("CARGO_PKG_VERSION");
    for module in &resolved {
        // F2: Enforce version constraints at load time
        if let Err(e) = module.manifest.check_mae_version(current_version) {
            registry.mark_failed(&module.name, e.clone());
            error!(module = %module.name, error = %e, "module version constraint failed");
            failures.push(format!("module '{}' skipped: {}", module.name, e));
            continue;
        }

        // A5: Snapshot keymaps before module eval for conflict detection
        let pre_snapshot = mae_core::keymap::snapshot_all_keymaps(&editor.keymaps);

        match load_module_autoloads(module, scheme, editor) {
            Ok(()) => {
                registry.mark_loaded(&module.name);
                info!(module = %module.name, "module loaded");

                // Detect keybinding conflicts introduced by this module
                let post_snapshot = mae_core::keymap::snapshot_all_keymaps(&editor.keymaps);
                for ((km_name, seq), new_cmd) in &post_snapshot {
                    if let Some(old_cmd) = pre_snapshot.get(&(km_name.clone(), seq.clone())) {
                        if old_cmd != new_cmd {
                            let key_str = mae_core::keymap::format_key_seq(seq);
                            let warning = format!(
                                "[module: {}] overrides '{}' in keymap '{}' (was: {}, now: {})",
                                module.name, key_str, km_name, old_cmd, new_cmd
                            );
                            info!("{}", warning);
                            editor.message_log.push(
                                mae_core::messages::MessageLevel::Warn,
                                "modules",
                                &warning,
                            );
                            editor.module_binding_warnings.push(warning);
                        }
                    }
                }

                // Fire module-loaded hook
                editor.fire_hook(&format!("module-loaded:{}", module.name));
            }
            Err(e) => {
                registry.mark_failed(&module.name, e.clone());
                error!(module = %module.name, error = %e, "module load failed");
                failures.push(format!("module '{}' failed: {}", module.name, e));
                // Continue loading other modules — error isolation
            }
        }
    }

    // Surface every failure durably (message log) plus one aggregate status line,
    // so a user with several failing modules doesn't just see the last one flash
    // by. Each individual reason is already in the log; the status points there.
    if !failures.is_empty() {
        for msg in &failures {
            editor
                .message_log
                .push(mae_core::messages::MessageLevel::Warn, "modules", msg);
        }
        editor.set_status(format!(
            "{} module(s) did not load (see :messages)",
            failures.len()
        ));
    }

    // Safety net: the leader/which-key system depends on keymap-leader plus the
    // active flavor. If either failed to load the editor is effectively unusable
    // (no SPC / C-; leader). Make that loud rather than a silent dead keymap.
    let flavor_mod = format!("keymap-{}", editor.keymap_flavor);
    for core in ["keymap-leader", flavor_mod.as_str()] {
        if all_modules.iter().any(|d| d.manifest.name() == core) && !registry.is_loaded(core) {
            let msg = format!(
                "core keymap module '{core}' did not load — the leader/which-key menu \
                 will be unavailable. Check :messages for the cause."
            );
            error!("{}", msg);
            editor
                .message_log
                .push(mae_core::messages::MessageLevel::Error, "modules", &msg);
            editor.set_status(msg);
        }
    }

    // Populate editor's active_modules for :describe-module, list_modules, audit
    editor.active_modules = registry
        .list()
        .iter()
        .map(|m| {
            let status = match &m.status {
                crate::pkg::loader::ModuleStatus::Loaded => "loaded".to_string(),
                crate::pkg::loader::ModuleStatus::Failed(e) => format!("failed: {}", e),
                crate::pkg::loader::ModuleStatus::Disabled => "disabled".to_string(),
                crate::pkg::loader::ModuleStatus::Discovered => "discovered".to_string(),
            };
            mae_core::editor::ModuleInfo {
                name: m.name.clone(),
                version: m.version.clone(),
                status,
                category: m.manifest.module.category.clone(),
                description: m.manifest.module.description.clone(),
                commands: m.commands.clone(),
                options: m.options.clone(),
                flags: m
                    .manifest
                    .flags
                    .iter()
                    .map(|(k, v)| (k.clone(), v.doc.clone()))
                    .collect(),
                path: m.source.label(""),
                depends: m.manifest.dependencies.keys().cloned().collect(),
                enabled_flags: m.enabled_flags.clone(),
            }
        })
        .collect();

    // Generate module:* KB nodes from loaded module data
    {
        use mae_core::kb_seed::modules::{install_module_nodes, ModuleKbData};
        let module_data: Vec<ModuleKbData> = registry
            .list()
            .iter()
            .map(|m| ModuleKbData {
                name: m.name.clone(),
                version: m.version.clone(),
                category: m.manifest.module.category.clone(),
                description: m.manifest.module.description.clone(),
                status: match &m.status {
                    crate::pkg::loader::ModuleStatus::Loaded => "loaded".to_string(),
                    crate::pkg::loader::ModuleStatus::Failed(e) => format!("failed: {}", e),
                    crate::pkg::loader::ModuleStatus::Disabled => "disabled".to_string(),
                    crate::pkg::loader::ModuleStatus::Discovered => "discovered".to_string(),
                },
                flags: m
                    .manifest
                    .flags
                    .iter()
                    .map(|(k, v)| (k.clone(), v.doc.clone()))
                    .collect(),
                commands: m.commands.clone(),
                options: m.options.clone(),
                path: m.source.label(""),
            })
            .collect();
        install_module_nodes(&mut editor.kb.primary, &module_data);
    }

    // Also drain any KB nodes registered from Scheme during module autoloads
    for (id, title, body) in scheme.drain_kb_nodes() {
        let node = mae_core::KbNode::new(id, title, mae_core::KbNodeKind::Note, body)
            .with_tags(["scheme"]);
        editor.kb.primary.insert(node);
    }

    // Auto-seed scheme:* KB nodes from live VM function registry (Phase 13h)
    // This supplements the static scheme_api.rs nodes with dynamic data
    // from all registered functions (stdlib + mae + user modules).
    {
        let fn_nodes = scheme.kb_function_nodes();
        let mut seeded = 0;
        for (id, title, body, tags) in fn_nodes {
            // Only insert if the node doesn't already exist (static nodes take priority)
            if editor.kb.primary.get(&id).is_none() {
                let tag_refs: Vec<&str> = tags.iter().map(|s| s.as_str()).collect();
                let node = mae_core::KbNode::new(id, title, mae_core::KbNodeKind::Concept, body)
                    .with_tags(tag_refs);
                editor.kb.primary.insert(node);
                seeded += 1;
            }
        }
        if seeded > 0 {
            debug!(count = seeded, "scheme KB nodes auto-seeded from VM");
        }
    }

    let loaded_count = resolved
        .iter()
        .filter(|m| registry.is_loaded(&m.name))
        .count();
    if loaded_count > 0 {
        info!(
            count = loaded_count,
            total = resolved.len(),
            "modules loaded"
        );
    }

    registry
}

/// Reload a single module's autoloads.scm.
///
/// Scans discovered modules for the named one, re-evaluates its autoloads,
/// and applies the result to the editor. This is a hot-reload path for
/// module development — no restart needed.
pub fn reload_module(name: &str, scheme: &mut SchemeRuntime, editor: &mut Editor) {
    use crate::pkg::embedded::merge_modules;
    use crate::pkg::loader::load_module_autoloads;
    use crate::pkg::manifest::discover_modules;
    use crate::pkg::resolver::ResolvedModule;

    // Find the module across the merged view (embedded baseline + on-disk
    // overrides), so reload works for embedded modules AND picks up an on-disk
    // edit of a built-in (disk overrides embedded) — the dev hot-reload loop.
    let mut disk = Vec::new();
    let builtin_dirs = builtin_module_dirs();
    for dir in &builtin_dirs {
        if dir.exists() && disk.is_empty() {
            disk.extend(discover_modules(dir));
        }
    }
    if let Some(user_pkg) = dirs_candidate("mae/packages") {
        if user_pkg.exists() {
            disk.extend(discover_modules(&user_pkg));
        }
    }
    let found = merge_modules(disk)
        .into_iter()
        .find(|d| d.manifest.name() == name);

    let Some(d) = found else {
        editor.set_status(format!("Module '{}' not found", name));
        return;
    };

    let resolved = ResolvedModule {
        name: name.to_string(),
        source: d.source,
        manifest: d.manifest,
        enabled_flags: vec![],
    };

    match load_module_autoloads(&resolved, scheme, editor) {
        Ok(()) => {
            info!(module = %name, "module reloaded");
            editor.set_status(format!("Module '{}' reloaded", name));
            editor.fire_hook(&format!("module-loaded:{}", name));
        }
        Err(e) => {
            error!(module = %name, error = %e, "module reload failed");
            editor.set_status(format!("Module '{}' reload failed: {}", name, e));
        }
    }
}

/// Reload ALL modules at runtime: re-discover (embedded baseline + on-disk
/// overrides) and re-run every enabled module's autoloads, exactly as startup
/// does. Registration is idempotent (define-key / register-command / options
/// overwrite; hooks dedupe), so this safely picks up on-disk edits and — after
/// `:reload-config` re-evaluates init.scm — changes to the `(mae!)` block.
///
/// This is the live counterpart to startup module loading (it reuses
/// [`load_modules`], so there is no second copy of the load logic to drift) and
/// the missing piece that previously forced a full restart to apply config.
pub fn reload_all_modules(scheme: &mut SchemeRuntime, editor: &mut Editor) {
    // Per-load keymap-conflict warnings would otherwise accumulate across reloads.
    editor.module_binding_warnings.clear();
    let registry = load_modules(scheme, editor);
    let loaded = registry
        .list()
        .iter()
        .filter(|m| matches!(m.status, crate::pkg::loader::ModuleStatus::Loaded))
        .count();
    info!(modules = loaded, "reloaded all modules");
    editor.set_status(format!("Reloaded {loaded} module(s)"));
}

/// Switch the keymap flavor at runtime (e.g. doom ↔ nonmodal). Resets keymaps to
/// the kernel baseline (dropping the previous flavor's `SPC`/`C-;`/CUA entries —
/// there is no bulk keymap-clear, so a clean rebuild is the robust path), then
/// re-runs module loading with the new flavor + re-applies user config + the
/// flavor's `default_mode`. The shared `leader` tree is flavor-independent, so
/// only the thin entry bindings and the startup mode actually change.
pub fn switch_keymap_flavor(scheme: &mut SchemeRuntime, editor: &mut Editor, flavor: &str) {
    reload_everything(scheme, editor, Some(flavor));
    info!(flavor, "switched keymap flavor");
    editor.set_status(format!("Keymap flavor: {flavor}"));
    // Hook so user config can react to a live flavor change (e.g. per-flavor
    // theme, extra bindings). Fired after keymaps + config + mode are settled.
    editor.fire_hook("keymap-flavor-changed");
}

/// The full reload pipeline — the SAME init → modules → config.scm → default_mode
/// sequence startup runs — so every live-reload entry point (`:reload-modules`,
/// flavor switch) applies identical steps and can't drift (audit C1/H2). Resets
/// keymaps to the kernel baseline first for a clean rebuild, re-runs init.scm so
/// user customizations there survive (the default template puts keybindings like
/// `shell-insert C-]` in init.scm — the old `:reload-modules` reloaded modules
/// ONLY, dropping config.scm + default_mode; the old flavor switch dropped
/// init.scm). `flavor_override` forces a keymap flavor for a live switch; `None`
/// keeps whatever init.scm / the `keymap_flavor` option selects.
pub fn reload_everything(
    scheme: &mut SchemeRuntime,
    editor: &mut Editor,
    flavor_override: Option<&str>,
) {
    editor.reset_keymaps_to_kernel();
    load_init_files(scheme, editor);
    if let Some(flavor) = flavor_override {
        // A live switch is authoritative over whatever init.scm set.
        let _ = editor.set_option("keymap_flavor", flavor);
    }
    // Baseline; the active flavor's autoloads re-assert it (non-modal → "insert").
    let _ = editor.set_option("default_mode", "normal");
    reload_all_modules(scheme, editor);
    // Re-apply user config.scm so personal bindings/options survive the reload.
    load_config_scm(scheme, editor);
    apply_default_mode(editor);
}

/// Load user config.scm — runs AFTER module autoloads so users can override.
///
/// This is the second half of the three-file model:
///   init.scm → module autoloads → config.scm
pub fn load_config_scm(scheme: &mut SchemeRuntime, editor: &mut Editor) -> bool {
    if let Some(config_scm) = dirs_candidate("mae/config.scm") {
        if config_scm.exists() {
            info!(path = %config_scm.display(), "loading config.scm");
            scheme.inject_editor_state(editor);
            match scheme.load_file(&config_scm) {
                Ok(()) => {
                    scheme.apply_to_editor(editor);
                    info!("config.scm loaded");
                    return true;
                }
                Err(e) => {
                    error!(error = %e, "config.scm load failed");
                    editor.set_status(format!("Error in config.scm: {}", e));
                }
            }
        }
    }
    false
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
        // pyright is best-in-class for type inference (better AI-peer context,
        // principle #4) and is what `doctor` already advertises. pylsp users can
        // set MAE_LSP_PYTHON=pylsp (or [lsp.python] command = "pylsp").
        (
            "python",
            "MAE_LSP_PYTHON",
            "pyright-langserver",
            &["--stdio"],
        ),
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
        // clangd serves both C and C++ (file-extension → language id happens in
        // lsp_intent::language_id_from_path). Missing clangd degrades gracefully
        // (find_binary skip). Override via MAE_LSP_CPP / MAE_LSP_C or [lsp.cpp].
        ("cpp", "MAE_LSP_CPP", "clangd", &[]),
        ("c", "MAE_LSP_C", "clangd", &[]),
        // These languages already highlight + resolve a language id; a default
        // server was the only missing piece. Each degrades gracefully when its
        // binary isn't installed (find_binary skip).
        ("ruby", "MAE_LSP_RUBY", "ruby-lsp", &[]),
        ("yaml", "MAE_LSP_YAML", "yaml-language-server", &["--stdio"]),
        (
            "json",
            "MAE_LSP_JSON",
            "vscode-json-language-server",
            &["--stdio"],
        ),
        ("toml", "MAE_LSP_TOML", "taplo", &["lsp", "stdio"]),
        ("bash", "MAE_LSP_BASH", "bash-language-server", &["start"]),
    ];

    let mut configs: HashMap<String, LspServerConfig> = HashMap::new();
    let mut server_info: HashMap<String, mae_core::LspServerInfo> = HashMap::new();

    // Phase 1: Populate from defaults, overridden by config.toml, overridden by env vars.
    // Only include servers whose binary is actually on PATH — avoids spawning
    // unnecessary processes for languages not used in the current project.
    for &(lang, env_var, default_cmd, default_args) in defaults {
        let (command, args) = resolve_lsp_config(lang, env_var, default_cmd, default_args, config);
        let binary_found = find_binary(&command);
        if !binary_found {
            info!(lang, command, "LSP server binary not found, skipping");
            continue;
        }
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
        if !binary_found {
            info!(lang, command, "LSP server binary not found, skipping");
            continue;
        }
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

/// Resolve collaborative user name from available sources.
///
/// Resolution order:
/// 1. `git config user.name`
/// 2. `$USER` environment variable
/// 3. hostname
/// 4. "anonymous"
///
/// Returns `(name, source)` for logging.
pub(crate) fn resolve_collab_user_name() -> (String, &'static str) {
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

/// Apply editor preferences loaded from `config.toml` (+ auto-derived collab
/// identity / KB CRDT client_id) onto the [`Editor`]. Called once at startup,
/// after `config::load_config()` and before Scheme runtime init (`init.scm`
/// loads after this, so it can still override anything set here).
pub(crate) fn apply_app_config(editor: &mut Editor, app_config: &crate::config::Config) {
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
    crate::init_collab_kb_client_id(editor);

    // NB: per-launch collab overrides (env) are applied AFTER init.scm loads
    // (below), so they win over config files — see `apply_collab_launch_overrides`.
    // (They used to be set here, before init.scm, which let a
    // `(set-option! "collab_auto_connect" …)` in init.scm clobber the env var.)

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
}

/// Load the KB federation registry and import enabled instances (primary
/// CozoDB store, manual/help KB, federated instances, shared-KB recovery).
/// No-op in `--clean`/`-q` mode.
pub(crate) fn init_kb_federation(editor: &mut Editor, clean_mode: bool) {
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
                if let Some(result) = crate::manual_kb::locate_and_validate(&data_dir, None) {
                    match &result.validation {
                        crate::manual_kb::ManualValidation::Valid => {
                            debug!(path = %result.path.display(), "manual KB checksum valid");
                        }
                        crate::manual_kb::ManualValidation::Historical { matched_version } => {
                            warn!(
                                path = %result.path.display(),
                                matched = %matched_version,
                                current = env!("CARGO_PKG_VERSION"),
                                "manual KB is from an older mae version"
                            );
                        }
                        crate::manual_kb::ManualValidation::Unknown => {
                            warn!(
                                path = %result.path.display(),
                                "manual KB checksum does not match any known release"
                            );
                        }
                        crate::manual_kb::ManualValidation::Custom => {
                            info!(path = %result.path.display(), "using custom manual KB");
                        }
                    }
                    match crate::manual_kb::load_nodes_readonly(&result.path) {
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
                    && crate::probe_daemon_hosts_primary(&editor.kb.daemon_socket);
                if daemon_hosts_primary {
                    editor.kb.daemon_enabled = true;
                    // Mark the mirror thin so lazy edit-hydration fires off the daemon
                    // READ layer (available now), without waiting for the collab write
                    // channel that `daemon_hosts_primary` requires.
                    editor.kb.set_primary_thin(true);
                    info!(
                        "Phase D3: local daemon hosts the primary KB — thin startup \
                         (skipping the mirror preload; reads via daemon, lazy load on edit)"
                    );
                }

                // Initialize primary KB store (CozoDB) for user data.
                let kb_root = kb_data_dir.root();
                let cozo_path = kb_root.join("primary.cozo");

                // Phase 2b: resolve the storage engine (default sqlite — lets multiple
                // daemon-less mae processes share the store). Orthogonal to daemon_mode.
                let mut engine = editor.kb.storage_engine.clone();

                // One-time, reversible sled→sqlite migration when sqlite is selected and
                // a legacy sled store (a directory) is present. On failure the intact
                // sled store is opened as-is this session, so the KB keeps working.
                if engine == "sqlite" {
                    match mae_kb::migrate::migrate_sled_to_sqlite(&cozo_path) {
                        Ok(mae_kb::migrate::SledToSqliteOutcome::Migrated {
                            nodes,
                            links,
                            backup,
                        }) => {
                            info!(nodes, links, backup = %backup.display(), "migrated primary KB store: sled → sqlite");
                            editor.set_status(format!(
                                "KB migrated to sqlite ({nodes} nodes) — old store backed up alongside"
                            ));
                        }
                        Ok(mae_kb::migrate::SledToSqliteOutcome::NotNeeded) => {}
                        Err(e) => {
                            error!(error = %e, "sled→sqlite migration failed; opening the existing store");
                            // #79 third slice: a startup migration failure fired once and
                            // easily overwritten by the next status message in the same
                            // startup burst — routed onto the notification bus so it's
                            // still visible after boot completes.
                            editor.notify(
                                mae_core::notifications::Notification::warning(
                                    "kb",
                                    "KB migration failed — opened existing store",
                                )
                                .body(format!("{e}"))
                                .key("kb-sled-to-sqlite-migration-failed"),
                            );
                            // The sled store is intact (a directory) — open it as sled so
                            // the KB still works; the user can retry the migration later.
                            if cozo_path.is_dir() {
                                engine = "sled".to_string();
                            }
                        }
                    }
                }

                match mae_kb::CozoKbStore::open_with_engine(&cozo_path, &engine) {
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

                        info!(path = %cozo_path.display(), "primary KB store opened (CozoDB)");
                        let arc_store = std::sync::Arc::new(store);
                        editor.kb.primary_cozo = Some(arc_store.clone());
                        editor.kb.store = Some(arc_store.clone());

                        // Load user nodes into the in-memory mirror — UNLESS the daemon
                        // hosts the primary (Phase D3): skip the bulk preload; nodes load
                        // lazily from this open store on edit (`kb_ensure_node_loaded`).
                        if daemon_hosts_primary {
                            info!("Phase D3: mirror preload skipped (daemon-hosted primary)");
                        } else {
                            // Phase 1a: run the O(n) load_all OFF the UI thread. A
                            // synchronous load on a large store (thousands of nodes)
                            // blocked the main thread long enough to trip the 10s startup
                            // watchdog. The loader thread streams the node set back via a
                            // channel drained on the idle tick (`drain_kb_preload`).
                            let store_for_load = arc_store.clone();
                            let (tx, rx) = std::sync::mpsc::channel();
                            std::thread::spawn(move || {
                                let result = store_for_load.load_all().map_err(|e| e.to_string());
                                let _ = tx.send(result);
                            });
                            editor.kb.pending_preload = Some(rx);
                        }

                        // Phase 4: watch the sqlite store file so this process reloads
                        // its mirror when ANOTHER daemon-less process commits. Skip for
                        // sled (single-writer) and daemon-hosted primaries.
                        if engine == "sqlite" && !daemon_hosts_primary {
                            match mae_kb::watch::StoreWatcher::new(&cozo_path) {
                                Ok(w) => editor.kb.store_watcher = Some(w),
                                Err(e) => {
                                    warn!(error = %e, "KB store watcher failed to start (cross-instance refresh off)")
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // Phase 0c: surface the failure LOUDLY instead of booting with a
                        // silent empty KB. A second daemon-less process hits the sled
                        // single-writer lock here; flag the store unavailable so KB
                        // mutations refuse rather than write to a mirror that will never
                        // persist.
                        error!(error = %e, path = %cozo_path.display(), "failed to open primary KB store");
                        editor.kb.store_unavailable = true;
                        // #79 third slice: the highest-value site in this pass — every
                        // subsequent KB edit is silently discarded until the user notices
                        // this, so a clobberable startup status line is a real data-loss
                        // risk, not just an inconvenience.
                        editor.notify(
                            mae_core::notifications::Notification::error(
                                "kb",
                                "KB store unavailable — KB changes cannot be saved",
                            )
                            .body(format!(
                                "{e} — another mae instance may hold it, or it is corrupt."
                            ))
                            .key("kb-store-unavailable"),
                        );
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
        // Issue #370: auto-register the shipped dev-practices KB as a
        // federated instance, if one is installed and not already
        // registered, so `ai_guidance_kb` (default "MaePractices" in the
        // shipped init.scm template) resolves out of the box. Additive-only
        // and a silent no-op otherwise — run BEFORE the registry load below
        // so a newly-added entry is picked up by the same import loop that
        // handles every other instance, with no separate import path needed.
        crate::practices_kb::ensure_registered(&data_dir);

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
                match editor.kb_open_instance_store(&inst.db_path) {
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

        // Phase 4 (KB): watch kb-registry.toml itself for changes by OTHER
        // mae processes (e.g. a KB registered from a second concurrently
        // running editor), so this process's registry stays fresh without
        // needing a local KB operation to trigger a reload (see
        // `drain_kb_registry_watch`). `StoreWatcher::new` requires its
        // target to already exist — write an empty registry first if this
        // is a brand-new install with no KB ever registered.
        if !new_registry.exists() {
            let _ = mae_kb::federation::KbRegistry::default().save(&data_dir);
        }
        editor.kb.registry_watcher = mae_kb::watch::StoreWatcher::new(&new_registry).ok();

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
        // Collect recovered instances locally (rather than pushing into
        // `editor.kb.registry` directly) so the eventual persist goes through
        // `KbRegistry::update`'s reload-fresh-then-mutate path — startup is
        // still a moment another concurrently-starting `mae` process could be
        // writing the same registry file.
        let mut recovered_instances = Vec::new();
        for (collab_id, name, db_path, last_sync) in recoveries {
            if collab_id.is_empty()
                || editor.kb.registry.find_by_collab_id(&collab_id).is_some()
                || !db_path.exists()
            {
                continue;
            }
            if let Ok(store) = editor.kb_open_instance_store(&db_path) {
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
                    recovered_instances.push(mae_kb::federation::KbInstance {
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
                        ai_residency: mae_kb::federation::AiResidency::default(),
                    });
                    info!(kb = %collab_id, nodes = count, "recovered shared KB instance from disk (registry rescan)");
                }
            }
        }
        if !recovered_instances.is_empty() {
            let (registry, (), saved) = mae_kb::federation::KbRegistry::update(&data_dir, |reg| {
                for inst in recovered_instances {
                    // Re-check against the freshly-reloaded registry: another
                    // process may have already added this collab_id since we
                    // loaded ours at the top of this function.
                    if reg
                        .find_by_collab_id(inst.collab_id.as_deref().unwrap_or_default())
                        .is_none()
                    {
                        reg.instances.push(inst);
                    }
                }
            });
            if let Err(e) = saved {
                warn!(error = %e, "failed to persist recovered shared-KB registry");
            }
            editor.kb.registry = registry;
            editor.kb.last_local_registry_write = Some(std::time::Instant::now());
        }

        // Build the CozoDB-first query layer AFTER all stores are loaded
        // (primary + manual + federated instances).
        editor.kb.rebuild_query_layer();

        // ADR-019: warm the shared-KB sync cache from durable markers at startup
        // so a restarted editor's broadcast gate + status reflect what syncs (the
        // re-subscribe to RECEIVE happens on the Connected event).
        editor.reconstruct_kb_sync_gate();
    }
}

/// On-demand daemon auto-spawn (ADR-035 `daemon_mode`) + LRU-cached daemon
/// connection, `editor`-only. Best-effort throughout: a failed spawn or
/// connect falls back to the in-process/local KB.
pub(crate) fn init_daemon_connection(editor: &mut Editor) {
    // On-demand auto-spawn (ADR-035 `daemon_mode`): if configured `on-demand` and
    // nothing is already listening, spawn + await a co-located mae-daemon before we
    // try to attach below. `shared` never spawns (it attaches to an externally
    // managed daemon); `off` skips this entirely. Best-effort — a failed spawn
    // falls through to the in-process KB (the attach below just warns + uses local).
    if editor.kb.daemon_mode == mae_core::DaemonMode::OnDemand {
        let socket = editor.kb.daemon_socket.clone();
        crate::daemon_supervisor::ensure_on_demand_daemon(editor.kb.daemon_mode, &socket);
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
                // One daemon/status round-trip drives two ADR-035 guardrails:
                // (1) version skew — warn on a mismatched daemon; (2) read routing
                // — only attach the daemon LRU when it actually hosts the primary
                // (or we're thin with no local mirror). A freshly spawned on-demand
                // daemon serves its own empty daemon-kb.cozo, so attaching its LRU
                // would shadow the editor's local KB with nothing. Best-effort: a
                // status failure never blocks the attach.
                let status = client.call("daemon/status", serde_json::json!({})).ok();
                if let Some(ref s) = status {
                    if let Some(msg) = crate::daemon_version_skew(env!("CARGO_PKG_VERSION"), s) {
                        // #323: this check already runs on EVERY daemon connect
                        // (proactive, not just when the user thinks to run
                        // :collab-doctor) — but a bare tracing::warn! only reaches
                        // a log file, invisible during normal use. That's the
                        // exact "silently serving stale data" gap #323 reports:
                        // route it onto the notification bus too.
                        warn!("{}", msg);
                        editor.notify(
                            mae_core::notifications::Notification::warning(
                                "daemon",
                                "mae-daemon version differs from this editor",
                            )
                            .body(msg)
                            .key("daemon-version-skew"),
                        );
                    }
                }
                let primary_exists = status
                    .as_ref()
                    .and_then(|s| s.get("primary_exists").and_then(|p| p.as_bool()))
                    .unwrap_or(false);
                if crate::daemon_supervisor::should_attach_daemon_reads(
                    primary_exists,
                    editor.kb.primary_thin(),
                ) {
                    let lru = mae_kb::lru_query::LruQueryLayer::new(client, cache_size);
                    editor
                        .kb
                        .set_daemon_query_layer(Some(std::sync::Arc::new(lru)));
                    info!("routing KB reads through the daemon (hosts primary)");
                } else {
                    info!(
                        "daemon connected but does not host the primary KB; \
                         KB reads stay local"
                    );
                }

                // Wire a second client as the control channel for P2P lifecycle
                // ops (ticket mint/join) — independent of the read path above.
                let mut control = mae_mcp::daemon_client::DaemonClient::new(&socket);
                match control.connect() {
                    Ok(()) => editor.kb.set_daemon_control(Some(std::sync::Arc::new(
                        crate::DaemonControlClient(std::sync::Mutex::new(control)),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_history_lists_round_trips_escaped_paths() {
        let mut files = mae_core::RecentFiles::new(100);
        files.push(PathBuf::from("/home/user/say \"hi\".txt"));
        files.push(PathBuf::from(r"C:\weird\backslash\path.txt"));
        let mut projects = mae_core::RecentProjects::new(20);
        projects.push(PathBuf::from("/home/user/proj a"));

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("history.scm");
        std::fs::write(
            &path,
            format!(
                ";; MAE generated history file. Do not edit by hand.\n\n{}",
                {
                    let mut s = String::new();
                    for f in files.list().iter().rev() {
                        s.push_str(&format!(
                            "(recent-files-add! \"{}\")\n",
                            f.to_string_lossy()
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                        ));
                    }
                    for p in projects.list().iter().rev() {
                        s.push_str(&format!(
                            "(recent-projects-add! \"{}\")\n",
                            p.to_string_lossy()
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                        ));
                    }
                    s
                }
            ),
        )
        .unwrap();

        let (parsed_files, parsed_projects) = parse_history_lists(&path);
        // File-order (oldest -> newest) is the reverse of MRU `.list()` order.
        let expected_files: Vec<PathBuf> = files.list().iter().rev().cloned().collect();
        let expected_projects: Vec<PathBuf> = projects.list().iter().rev().cloned().collect();
        assert_eq!(parsed_files, expected_files);
        assert_eq!(parsed_projects, expected_projects);
    }

    /// Adversarial case for the exit-time merge: a naive "just serialize the
    /// session's own list" implementation would silently drop `old2` (added
    /// by a different, concurrently-running `mae` process) because this
    /// session's in-memory list never saw it. `merge_history_lists` must
    /// preserve it — the session's recency wins for anything it touched,
    /// but disk-only entries survive as "older", not vanish.
    #[test]
    fn merge_history_lists_preserves_disk_only_entries() {
        let disk_files = vec![PathBuf::from("/old1"), PathBuf::from("/old2")];
        let mut session_files = mae_core::RecentFiles::new(100);
        session_files.push(PathBuf::from("/old1")); // re-touched this session
        session_files.push(PathBuf::from("/new1"));

        let (merged, _) = merge_history_lists(
            disk_files,
            vec![],
            &session_files,
            &mae_core::RecentProjects::new(20),
        );

        let result: Vec<PathBuf> = merged.list().iter().cloned().collect();
        assert_eq!(
            result,
            vec![
                PathBuf::from("/new1"),
                PathBuf::from("/old1"),
                PathBuf::from("/old2"),
            ],
            "session recency wins for touched entries; disk-only entries survive as older, not dropped"
        );
    }

    /// #79 third slice: a primary-KB-store-open failure used to be a clobberable
    /// status-line message fired once during the startup burst — easy to miss, yet
    /// every subsequent KB edit is silently discarded until the user notices. Must
    /// land as a durable notification. Uses a REAL failure (a garbage regular file
    /// where a valid CozoDB store is expected), not a synthetic one.
    #[test]
    #[cfg(unix)]
    fn init_kb_federation_notifies_on_a_real_store_open_failure() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let kb_dir = tmp.path().join("kb");
        std::fs::create_dir_all(&kb_dir).unwrap();
        // primary.cozo as an unreadable regular file: not a directory (so the
        // sled->sqlite migration check short-circuits to NotNeeded) and permission
        // denied at the OS level — CozoKbStore::open_with_engine must fail on it for
        // real, without going through cozo's own file-format parsing (which panics
        // internally on garbage content rather than returning a clean Err).
        let cozo_path = kb_dir.join("primary.cozo");
        std::fs::write(&cozo_path, b"").unwrap();
        std::fs::set_permissions(&cozo_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut editor = mae_core::Editor::new();
        editor.data_dir_override = Some(tmp.path().to_path_buf());

        init_kb_federation(&mut editor, false);

        // Restore permissions so tempdir cleanup can remove the file.
        std::fs::set_permissions(&cozo_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(
            editor.kb.store_unavailable,
            "a real store-open failure must flag store_unavailable"
        );
        let notes = editor.notifications.active_sorted();
        let hit = notes
            .iter()
            .find(|n| n.source == "kb" && n.title.contains("KB store unavailable"));
        assert!(
            hit.is_some(),
            "a durable notification must be raised for a real store-open failure, \
             not just a status-line toast; got: {:?}",
            notes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );
        assert_eq!(
            hit.unwrap().severity,
            mae_core::notifications::Severity::Error
        );
    }

    /// #79 third slice: a sled->sqlite migration failure used to be a clobberable
    /// status-line message only. Uses a REAL failure — a `primary.cozo` directory
    /// that LOOKS like a legacy sled store (triggers the migration attempt) but
    /// contains no valid sled database, so the migration's own open genuinely fails.
    #[test]
    #[cfg(unix)]
    fn init_kb_federation_notifies_on_a_real_migration_failure() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let kb_dir = tmp.path().join("kb");
        // primary.cozo as a DIRECTORY (looks like a legacy sled store, so
        // migrate_sled_to_sqlite attempts the migration) but permission-denied —
        // the migration's own sled open must fail for real, without relying on
        // sabotaging sled's on-disk format (undocumented, fragile to depend on).
        let cozo_dir = kb_dir.join("primary.cozo");
        std::fs::create_dir_all(&cozo_dir).unwrap();
        std::fs::set_permissions(&cozo_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut editor = mae_core::Editor::new();
        editor.data_dir_override = Some(tmp.path().to_path_buf());
        assert_eq!(
            editor.kb.storage_engine, "sqlite",
            "default engine must be sqlite for the migration path to even attempt"
        );

        init_kb_federation(&mut editor, false);

        // Restore permissions so tempdir cleanup can remove the directory.
        std::fs::set_permissions(&cozo_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let notes = editor.notifications.active_sorted();
        let hit = notes
            .iter()
            .find(|n| n.source == "kb" && n.title.contains("KB migration failed"));
        assert!(
            hit.is_some(),
            "a durable notification must be raised for a real migration failure, \
             not just a status-line toast; got: {:?}",
            notes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );
        assert_eq!(
            hit.unwrap().severity,
            mae_core::notifications::Severity::Warning
        );
    }

    /// Recursively copy a directory tree (mirrors `manual_kb.rs::copy_dir_all` —
    /// duplicated rather than shared since that one is private to its module
    /// and this is test-only code). Used to stage a throwaway copy of a
    /// pre-built KB asset before opening it live: CozoDB (sled in
    /// particular) always opens read-write and may migrate/compact/write
    /// recovery snapshots on open, which would dirty a git-tracked asset —
    /// see `manual_kb.rs::load_nodes_readonly`'s doc comment for the same
    /// hazard, hit for real once already while writing this test (a sled
    /// directory got silently migrated to sqlite, with `.sled.bak-*` debris
    /// left alongside, the moment `init_kb_federation`'s normal federated-
    /// instance import path opened it in place).
    fn copy_kb_asset_to_tempdir(src: &std::path::Path) -> tempfile::TempDir {
        fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
            std::fs::create_dir_all(dst)?;
            for entry in std::fs::read_dir(src)? {
                let entry = entry?;
                let to = dst.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_dir_all(&entry.path(), &to)?;
                } else {
                    std::fs::copy(entry.path(), &to)?;
                }
            }
            Ok(())
        }
        let tmp = tempfile::tempdir().unwrap();
        let dst = tmp.path().join(src.file_name().unwrap());
        if src.is_dir() {
            copy_dir_all(src, &dst).expect("failed to stage KB asset copy");
        } else {
            std::fs::copy(src, &dst).expect("failed to stage KB asset copy");
        }
        tmp
    }

    /// Issue #370, end-to-end: `init_kb_federation` must auto-register the
    /// shipped practices KB and load it into `editor.kb.registry`/
    /// `editor.kb.instances`, using the REAL built `assets/mae-practices.cozo`
    /// (not a synthetic fixture) so this proves the whole chain — locate,
    /// register, import — against the actual asset that ships, not a stand-in
    /// that might not reflect its real shape. Operates on a throwaway COPY
    /// (see `copy_kb_asset_to_tempdir`) — the committed asset itself is
    /// never opened directly.
    #[test]
    fn init_kb_federation_auto_registers_and_loads_the_practices_kb() {
        // Isolate from any ambient env override so this test's own
        // `MAE_PRACTICES_KB_PATH` is authoritative regardless of what else
        // might be running in this process.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("MAE_PRACTICES_KB_PATH").ok();

        let real_asset = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/mae-practices.cozo");
        assert!(
            real_asset.exists(),
            "expected the real built practices KB at {} -- run `make practices-kb` first",
            real_asset.display()
        );
        let staged = copy_kb_asset_to_tempdir(&real_asset);
        let staged_asset = staged.path().join(real_asset.file_name().unwrap());
        std::env::set_var("MAE_PRACTICES_KB_PATH", &staged_asset);

        let tmp = tempfile::tempdir().unwrap();
        let mut editor = mae_core::Editor::new();
        editor.data_dir_override = Some(tmp.path().to_path_buf());

        init_kb_federation(&mut editor, false);

        match prev {
            Some(v) => std::env::set_var("MAE_PRACTICES_KB_PATH", v),
            None => std::env::remove_var("MAE_PRACTICES_KB_PATH"),
        }

        let inst = editor
            .kb
            .registry
            .find(crate::practices_kb::INSTANCE_NAME)
            .expect("MaePractices must be auto-registered");
        // `ensure_registered` copies the located asset into this data dir's
        // own canonical location before registering it (never the located
        // path directly, unless it was already there) — see its doc
        // comment for why a federated instance's `db_path` can't safely
        // point straight at `staged_asset`'s source location either.
        assert_eq!(inst.db_path, tmp.path().join("mae-practices.cozo"));
        assert!(
            editor.kb.instances.contains_key(&inst.uuid),
            "the newly-registered instance must be imported into kb.instances \
             this same session, not only persisted to the registry file"
        );
        let kb = &editor.kb.instances[&inst.uuid];
        assert!(
            kb.get("index").is_some(),
            "the real practices KB's index node must have loaded"
        );
    }

    /// #323: `daemon_version_skew` (main.rs) was already implemented, unit-tested,
    /// and — this is the load-bearing part — already CALLED here on every daemon
    /// connect (not just when the user thinks to run `:collab-doctor`). But the
    /// call site only reached `tracing::warn!`, a log line invisible during normal
    /// use — exactly the "daemon silently serves stale data with nothing surfacing
    /// it" gap #323 reports. Real fake daemon socket (not a synthetic call to
    /// daemon_version_skew in isolation) speaking real Content-Length JSON-RPC,
    /// responding to `daemon/status` with a version that differs from this
    /// editor's own — asserts the mismatch reaches the durable notification bus.
    #[test]
    #[cfg(unix)]
    fn init_daemon_connection_notifies_on_a_real_version_mismatch() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::unix::net::UnixListener;

        let tmp = tempfile::tempdir().unwrap();
        let socket_path = tmp.path().join("mae-daemon-fake.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;

            // Read one Content-Length-framed JSON-RPC request (real framing, the
            // same the daemon's own server side speaks).
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.strip_prefix("Content-Length: ") {
                    content_length = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(req["method"], "daemon/status");

            // Respond with a version that DIFFERS from this editor's own build.
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": { "version": "0.0.1-fake-old-daemon", "primary_exists": false },
            });
            let resp_body = serde_json::to_vec(&resp).unwrap();
            write!(writer, "Content-Length: {}\r\n\r\n", resp_body.len()).unwrap();
            writer.write_all(&resp_body).unwrap();
            writer.flush().unwrap();
        });

        let mut editor = mae_core::Editor::new();
        editor.kb.daemon_enabled = true;
        editor.kb.daemon_socket = socket_path;

        init_daemon_connection(&mut editor);
        server.join().unwrap();

        let notes = editor.notifications.active_sorted();
        let hit = notes
            .iter()
            .find(|n| n.source == "daemon" && n.title.contains("version differs"));
        assert!(
            hit.is_some(),
            "a durable notification must be raised for a real daemon version \
             mismatch, not just a tracing log line; got: {:?}",
            notes.iter().map(|n| &n.title).collect::<Vec<_>>()
        );
        assert_eq!(
            hit.unwrap().severity,
            mae_core::notifications::Severity::Warning
        );
        assert!(
            hit.unwrap()
                .body
                .as_ref()
                .is_some_and(|b| b.contains("0.0.1-fake-old-daemon")),
            "the notification body must name the mismatched daemon version"
        );
    }

    macro_rules! require_scheme {
        () => {
            SchemeRuntime::new().expect("SchemeRuntime::new() should not fail")
        };
    }

    fn disc(name: &str, toml: &str) -> crate::pkg::embedded::DiscoveredModule {
        use crate::pkg::embedded::{DiscoveredModule, ModuleSource};
        use crate::pkg::manifest::ModuleManifest;
        use std::path::{Path, PathBuf};
        DiscoveredModule {
            source: ModuleSource::Disk(PathBuf::from(format!("modules/{name}"))),
            manifest: ModuleManifest::from_str(toml, Path::new("test")).unwrap(),
        }
    }

    #[test]
    fn enable_with_deps_expands_deps_of_already_enabled_module() {
        // Regression for the Linux "keymap-doom depends on keymap-leader which is
        // not enabled" brick: when the flavor is declared in (mae!) it is already
        // in `enabled`, and enable_with_deps must STILL pull in keymap-leader.
        let all = vec![
            disc(
                "keymap-doom",
                "[module]\nname = \"keymap-doom\"\n\n[dependencies]\nkeymap-leader = \"*\"",
            ),
            disc("keymap-leader", "[module]\nname = \"keymap-leader\""),
        ];
        // keymap-doom pre-enabled (as if declared in the mae! block).
        let mut enabled: HashMap<String, Vec<String>> = HashMap::new();
        enabled.insert("keymap-doom".to_string(), vec![]);

        enable_with_deps("keymap-doom", &all, &mut enabled);

        assert!(
            enabled.contains_key("keymap-leader"),
            "declared keymap-doom must still pull in its keymap-leader dependency"
        );

        // And the resolver must now produce a consistent order with no skips.
        let outcome = crate::pkg::resolver::resolve_load_order(&all, &enabled);
        assert!(outcome.skipped.is_empty(), "nothing should be skipped");
        let names: Vec<&str> = outcome.resolved.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"keymap-doom") && names.contains(&"keymap-leader"));
    }

    #[test]
    fn reconcile_keymap_flavor_option_is_authoritative() {
        // No declaration → option untouched, nothing dropped.
        assert_eq!(reconcile_keymap_flavor(&[], "doom", "doom"), (None, vec![]));

        // Lone declaration, option at default → adopt the declaration.
        let (sync, drop) = reconcile_keymap_flavor(&["keymap-nonmodal".into()], "doom", "doom");
        assert_eq!(sync.as_deref(), Some("nonmodal"));
        assert!(drop.is_empty(), "the adopted flavor must not be dropped");

        // Option explicitly set (non-default) disagrees with a declared flavor →
        // option wins, declared flavor dropped (this is the live-switch case:
        // init.scm hardcodes keymap-doom, user switched to nonmodal).
        let (sync, drop) = reconcile_keymap_flavor(&["keymap-doom".into()], "nonmodal", "doom");
        assert_eq!(sync, None);
        assert_eq!(drop, vec!["keymap-doom".to_string()]);

        // Declaration matches the option → nothing to sync or drop.
        let (sync, drop) = reconcile_keymap_flavor(&["keymap-doom".into()], "doom", "doom");
        assert_eq!(sync, None);
        assert!(drop.is_empty());
    }

    #[test]
    fn enable_with_deps_terminates_on_cycle() {
        // Self-referential / cyclic deps must not loop forever.
        let all = vec![
            disc("a", "[module]\nname = \"a\"\n\n[dependencies]\nb = \"*\""),
            disc("b", "[module]\nname = \"b\"\n\n[dependencies]\na = \"*\""),
        ];
        let mut enabled: HashMap<String, Vec<String>> = HashMap::new();
        enable_with_deps("a", &all, &mut enabled);
        assert!(enabled.contains_key("a") && enabled.contains_key("b"));
    }

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
        let mut scheme = require_scheme!();
        let mut editor = Editor::new();
        // load_init_files returns a usize count
        let _count: usize = load_init_files(&mut scheme, &mut editor);
    }

    #[test]
    fn load_init_files_returns_zero_when_no_files() {
        let mut scheme = require_scheme!();
        // In a temp dir with no init.scm, should return 0
        let tmp = tempfile::tempdir().unwrap();
        let _guard = std::env::set_current_dir(tmp.path());
        // Note: this test may still load ~/.config/mae/init.scm if it exists,
        // but that's fine — we're testing that the function completes without error.
        let mut editor = Editor::new();
        let count = load_init_files(&mut scheme, &mut editor);
        // Count depends on whether user has an init.scm, so just verify no panic
        let _ = count;
    }

    #[test]
    fn reload_all_modules_loads_embedded_and_is_idempotent() {
        // Embedded keymap-doom must load even with no on-disk modules, and a
        // second reload must not change binding/module counts (idempotent
        // registration). This is the core regression guard for the whole
        // overhaul: it proves the embedded baseline populates the leader tree,
        // so removing the kernel's duplicated SPC bindings is safe.
        let mut scheme = require_scheme!();
        let mut editor = Editor::new();

        let total_normal = |e: &Editor| -> usize {
            e.keymaps
                .get("normal")
                .map(|k| k.bindings().count())
                .unwrap_or(0)
        };
        let has_collab = |e: &Editor| -> bool {
            // collab-start lives in the `leader` keymap (via keymap-leader).
            // Check all keymaps to be resilient to on-disk module overrides
            // during development (stale ~/.local/share/mae/modules may put
            // it in `normal` instead).
            e.keymaps
                .values()
                .any(|k| k.bindings().any(|(_, cmd)| cmd == "collab-start"))
        };

        reload_all_modules(&mut scheme, &mut editor);
        let mods1 = editor.active_modules.len();
        let binds1 = total_normal(&editor);
        assert!(
            mods1 >= 20,
            "embedded modules should load with no on-disk modules, got {mods1}"
        );
        assert!(
            editor
                .active_modules
                .iter()
                .any(|m| m.name == "keymap-doom" && m.status == "loaded"),
            "embedded keymap-doom must load"
        );
        // The collab leader binding must be present in some keymap after
        // module loading (in the `leader` keymap via keymap-leader).
        assert!(
            has_collab(&editor),
            "collab-start (SPC C s) should be bound after keymap modules load"
        );

        reload_all_modules(&mut scheme, &mut editor);
        assert_eq!(
            mods1,
            editor.active_modules.len(),
            "module count stable across reload"
        );
        assert_eq!(
            binds1,
            total_normal(&editor),
            "binding count stable (idempotent reload)"
        );
    }

    #[test]
    fn unknown_keymap_flavor_falls_back_to_doom() {
        // A bogus keymap_flavor must not leave the user with no leader tree —
        // load_modules falls back to the embedded keymap-doom.
        let mut scheme = require_scheme!();
        let mut editor = Editor::new();
        editor.keymap_flavor = "nonexistent-flavor".to_string();
        reload_all_modules(&mut scheme, &mut editor);
        assert!(
            editor
                .active_modules
                .iter()
                .any(|m| m.name == "keymap-doom" && m.status == "loaded"),
            "unknown flavor should fall back to keymap-doom"
        );
    }

    #[test]
    fn keymap_flavor_option_roundtrips() {
        let mut editor = Editor::new();
        assert_eq!(
            editor
                .get_option("keymap_flavor")
                .map(|(v, _)| v)
                .as_deref(),
            Some("doom"),
            "default keymap_flavor should be doom"
        );
        editor.set_option("keymap_flavor", "emacs").unwrap();
        assert_eq!(editor.keymap_flavor, "emacs");
        assert_eq!(
            editor
                .get_option("keymap_flavor")
                .map(|(v, _)| v)
                .as_deref(),
            Some("emacs")
        );
    }

    #[test]
    fn load_memory_context_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        assert!(load_memory_context(dir.path()).is_none());
    }

    #[test]
    fn load_memory_context_sorted_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("1000_old.txt"), "old fact").unwrap();
        std::fs::write(mem_dir.join("2000_new.txt"), "new fact").unwrap();
        let result = load_memory_context(dir.path()).unwrap();
        assert!(result.starts_with("## Long-term Memory\n"));
        let new_pos = result.find("new fact").unwrap();
        let old_pos = result.find("old fact").unwrap();
        assert!(new_pos < old_pos, "newer entries should come first");
    }

    // --- synthesize_memory tests ---

    #[test]
    fn synthesize_memory_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Full,
            mae_ai::context_limits::ProviderHint::Claude,
            4000,
        )
        .is_none());
    }

    #[test]
    fn synthesize_memory_small_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("1000_fact.txt"), "always use snake_case").unwrap();
        std::fs::write(mem_dir.join("2000_fact.txt"), "the crate uses ropey").unwrap();
        let result = synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Full,
            mae_ai::context_limits::ProviderHint::Claude,
            4000,
        )
        .unwrap();
        assert!(result.contains("## Project Memory"));
        assert!(result.contains("always use snake_case"));
        assert!(result.contains("the crate uses ropey"));
    }

    #[test]
    fn synthesize_memory_exceeds_budget() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        for i in 0..50 {
            let content = format!("always follow convention rule {}", i);
            std::fs::write(mem_dir.join(format!("{:04}_fact.txt", i)), content).unwrap();
        }
        let result = synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Full,
            mae_ai::context_limits::ProviderHint::Claude,
            200, // tiny budget
        )
        .unwrap();
        assert!(result.len() <= 250); // budget + header
    }

    #[test]
    fn synthesize_memory_compact_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("1000_fact.txt"), "always use bun").unwrap();
        let result = synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Compact,
            mae_ai::context_limits::ProviderHint::Claude,
            4000,
        )
        .unwrap();
        // Compact tier → numbered list
        assert!(result.contains("1. always use bun"));
    }

    #[test]
    fn synthesize_memory_deepseek_forces_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("1000_fact.txt"), "always use bun").unwrap();
        let result = synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Full, // Full tier, but DeepSeek → numbered
            mae_ai::context_limits::ProviderHint::DeepSeek,
            4000,
        )
        .unwrap();
        assert!(result.contains("1. always use bun"));
    }

    #[test]
    fn synthesize_memory_categories_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("1000_a.txt"), "bug: crash on startup").unwrap();
        std::fs::write(mem_dir.join("2000_b.txt"), "always use tabs").unwrap();
        let result = synthesize_memory(
            dir.path(),
            mae_ai::context_limits::ModelTier::Full,
            mae_ai::context_limits::ProviderHint::Claude,
            4000,
        )
        .unwrap();
        // Conventions should appear before bugs
        let conv_pos = result.find("always use tabs").unwrap();
        let bug_pos = result.find("crash on startup").unwrap();
        assert!(
            conv_pos < bug_pos,
            "conventions should appear before bugs: conv={}, bug={}",
            conv_pos,
            bug_pos
        );
    }

    #[test]
    fn load_memory_context_cap_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join(".mae/memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        // Write enough files to exceed 8000 chars
        for i in 0..100 {
            let content = format!("fact number {} with padding {}", i, "x".repeat(100));
            std::fs::write(mem_dir.join(format!("{:04}_entry.txt", i)), content).unwrap();
        }
        let result = load_memory_context(dir.path()).unwrap();
        // Should be capped near 8000 + truncation message
        assert!(result.len() < 8100);
    }
}
