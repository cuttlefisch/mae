//! MAE configuration file loading, precedence resolution, and first-run wizard.
//!
//! Config precedence (highest → lowest):
//!   1. Environment variables (MAE_AI_PROVIDER, ANTHROPIC_API_KEY, MAE_AI_MODEL, …)
//!   2. `init.scm` — `(set-option!)` calls (primary user config surface)
//!   3. `config.toml` — legacy bootstrap (AI provider + theme for pre-Scheme loading)
//!   4. Built-in defaults (OptionDef structs in options.rs)
//!
//! The wizard writes both config.toml (bootstrap) and init.scm (all options).
//! `:set-save` persists to init.scm only. Going forward, init.scm is the
//! sole user config surface.
//!
//! Env var `MAE_SKIP_WIZARD=1` disables the wizard (useful for CI, containers,
//! and non-interactive launches).

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use mae_ai::{BudgetConfig, PermissionPolicy, PermissionTier, ProviderConfig};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Top-level configuration, serialized as `~/.config/mae/config.toml`.
/// Every field is optional so an empty or partial file is valid.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Schema version for forward-compatible config evolution.
    /// Absent or 0 in legacy files; current version is 1.
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    #[serde(default)]
    pub ai: AiSection,
    #[serde(default)]
    pub editor: EditorSection,
    #[serde(default)]
    pub agents: AgentsSection,
    #[serde(default)]
    pub lsp: LspSection,
    #[serde(default)]
    pub performance: PerformanceSection,
    #[serde(default)]
    pub org: OrgSection,
    #[serde(default)]
    pub collaboration: CollaborationSection,
    #[serde(default)]
    pub kb: KbSection,
    #[serde(default)]
    pub daemon: DaemonSection,
}

/// Current config schema version. Bump when config.toml format changes.
const CURRENT_CONFIG_VERSION: u32 = 1;

fn default_config_version() -> u32 {
    1
}

/// Per-language LSP server configuration.
/// Extensible: any language key is valid (e.g. `[lsp.zig]`, `[lsp.haskell]`).
///
/// ```toml
/// [lsp.rust]
/// command = "rust-analyzer"
///
/// [lsp.python]
/// command = "pylsp"
///
/// [lsp.typescript]
/// command = "typescript-language-server"
/// args = ["--stdio"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LspSection {
    /// Per-language server configs. Key is the language ID (e.g. "rust", "python").
    #[serde(flatten)]
    pub servers: std::collections::HashMap<String, LspLanguageConfig>,
}

/// Configuration for a single LSP language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLanguageConfig {
    /// The command to run (e.g. "rust-analyzer", "pylsp").
    pub command: String,
    /// Arguments to pass to the command.
    #[serde(default)]
    pub args: Vec<String>,
}

impl LspLanguageConfig {
    /// Convert args to `&[&str]` for compatibility with resolve functions.
    pub fn args_as_strs(&self) -> Vec<&str> {
        self.args.iter().map(|s| s.as_str()).collect()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSection {
    /// "claude" | "openai" | "gemini" | "ollama" | "deepseek"
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    /// Shell command whose stdout is used as the API key.
    /// Runs once at startup. Example: `"pass show deepseek/api-key"`
    /// Takes precedence over `api_key` but not over env vars.
    pub api_key_command: Option<String>,
    pub timeout_secs: Option<u64>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    /// Permission tier for AI/MCP tool execution:
    ///   "readonly"    — buffer reads only
    ///   "write"       — reads + edits
    ///   "shell"       — reads + edits + shell (default, container-first)
    ///   "privileged"  — everything including quit/force-quit
    /// Legacy aliases: "standard" → write, "trusted" → shell, "full" → privileged
    /// Env override: MAE_AI_PERMISSIONS (highest precedence).
    pub auto_approve_tier: Option<String>,
    /// Override the prompt tier for this model: "full" or "compact".
    /// If unset, auto-detected from the model name via the built-in table.
    /// Full tier: concise prompt for frontier models (Claude Opus/Sonnet, GPT-4o).
    /// Compact tier: explicit guardrails for smaller models (DeepSeek, Haiku).
    pub prompt_tier: Option<String>,
    /// Command to launch for AI agent shell sessions (SPC a a).
    /// Default: "claude"
    pub editor: Option<String>,
    /// Per-session spend guardrails. Both fields optional — setting
    /// neither disables budgeting, setting only the warn threshold
    /// keeps the session running with visibility but no hard limits.
    /// Shares the canonical `mae_ai::BudgetConfig` type so new fields
    /// added to the budget don't require a parallel shadow struct here.
    #[serde(default)]
    pub budget: BudgetConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditorSection {
    pub theme: Option<String>,
    pub splash_art: Option<String>,
    pub font_family: Option<String>,
    pub icon_font_family: Option<String>,
    pub font_size: Option<f32>,
    pub org_hide_emphasis_markers: Option<bool>,
    /// Restore previous session on startup (per-project).
    pub restore_session: Option<bool>,
    /// Autosave interval in seconds (0 = disabled). Requires 5s idle after last edit.
    pub autosave_interval: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgSection {
    /// Directories and files to include in the agenda view.
    /// Use `:agenda-add <path>` / `:agenda-remove <path>` to manage.
    #[serde(default)]
    pub agenda_files: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceSection {
    pub large_file_lines: Option<usize>,
    pub degrade_threshold_chars: Option<usize>,
    pub degrade_threshold_line_length: Option<usize>,
    pub display_region_debounce_ms: Option<u64>,
    pub syntax_reparse_debounce_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollaborationSection {
    /// State server address (e.g. "127.0.0.1:9473").
    pub server_address: Option<String>,
    /// Automatically connect to the state server on startup.
    pub auto_connect: Option<bool>,
    /// Automatically share new file buffers when connected.
    pub auto_share: Option<bool>,
    /// Seconds between reconnection attempts (default: 5).
    pub reconnect_interval_secs: Option<u64>,
    /// Display name for collaborative edits (shown to peers).
    pub user_name: Option<String>,
    /// Seconds between heartbeat pings to the state server (0 = disabled, default: 30).
    pub heartbeat_interval_secs: Option<u64>,
    /// Shell command to retrieve the PSK (preferred over `psk` for security).
    pub psk_command: Option<String>,
    /// Pre-shared key for mutual authentication (plaintext fallback).
    pub psk: Option<String>,
    /// KB sync mode: "on_save" (default) or "manual".
    pub kb_sync_mode: Option<String>,
}

/// KB configuration section.
///
/// CozoDB (with SQLite storage engine) is the sole backend since v0.12.1.
/// This struct is retained for forward compatibility with config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KbSection {}

/// Daemon configuration section.
///
/// Controls connection to `mae-daemon` for persistent KB (CozoDB+SQLite),
/// background maintenance, and services that outlive the editor session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonSection {
    /// Connect to mae-daemon for KB persistence and background services.
    pub enabled: Option<bool>,
    /// Unix socket path for daemon communication.
    pub socket: Option<String>,
    /// Maximum nodes in editor LRU cache (0 = unbounded).
    pub cache_size: Option<usize>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsSection {
    /// Automatically write `.mcp.json` to the project root on terminal spawn.
    /// Set to `false` or `MAE_AGENTS_AUTO_MCP=0` to disable.
    #[serde(default = "default_true")]
    pub auto_mcp_json: bool,
    /// Automatically configure spawned agents to trust MAE's MCP tools.
    /// Writes agent-specific settings (e.g. `.claude/settings.local.json`)
    /// so tools run without per-call approval prompts.
    /// Set to `false` or `MAE_AGENTS_AUTO_APPROVE=0` to disable.
    #[serde(default = "default_true")]
    pub auto_approve_tools: bool,
}

impl Default for AgentsSection {
    fn default() -> Self {
        Self {
            auto_mcp_json: true,
            auto_approve_tools: true,
        }
    }
}

impl AgentsSection {
    /// Resolve with env var override: `MAE_AGENTS_AUTO_MCP=0` disables.
    pub fn auto_mcp_json_effective(&self) -> bool {
        if let Ok(val) = std::env::var("MAE_AGENTS_AUTO_MCP") {
            return val != "0";
        }
        self.auto_mcp_json
    }

    /// Resolve with env var override: `MAE_AGENTS_AUTO_APPROVE=0` disables.
    pub fn auto_approve_tools_effective(&self) -> bool {
        if let Ok(val) = std::env::var("MAE_AGENTS_AUTO_APPROVE") {
            return val != "0";
        }
        self.auto_approve_tools
    }
}

/// Return the path to the user config file, honoring XDG_CONFIG_HOME.
pub fn config_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("mae")
        .join("config.toml")
}

// Load the config file, returning `Config::default()` if it doesn't exist or
// is unreadable. Parse errors are logged (not fatal).

fn validate_config_version(cfg: &Config) -> Option<String> {
    if cfg.config_version > CURRENT_CONFIG_VERSION {
        Some(format!(
            "Config version {} is newer than this MAE build (supports v{}). \
             Some settings may be ignored.",
            cfg.config_version, CURRENT_CONFIG_VERSION
        ))
    } else {
        None
    }
}

/// Returns `(Config, Option<String>)` where the second element is a
/// human-readable error message when the config was malformed or unreadable.
/// Callers can surface this in the status bar at startup.
pub fn load_config() -> (Config, Option<String>) {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(cfg) => {
                let warning = validate_config_version(&cfg);
                debug!(path = %path.display(), "loaded config");
                (cfg, warning)
            }
            Err(e) => {
                let msg = format!("Config parse error: {}; using defaults", e);
                warn!(path = %path.display(), error = %e, "config parse failed, using defaults");
                (Config::default(), Some(msg))
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            debug!(path = %path.display(), "no config file, using defaults");
            (Config::default(), None)
        }
        Err(e) => {
            let msg = format!("Config read error: {}; using defaults", e);
            warn!(path = %path.display(), error = %e, "config read failed, using defaults");
            (Config::default(), Some(msg))
        }
    }
}

/// Write a config to the user's config path, creating parent directories.
pub fn save_config(cfg: &Config) -> io::Result<PathBuf> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let header = default_config_header();
    let body = toml::to_string_pretty(cfg).map_err(io::Error::other)?;
    fs::write(&path, format!("{}{}", header, body))?;
    Ok(path)
}

/// Write a commented-out template config (no actual values) to the user's
/// config path if none exists. Used by `mae --init-config`.
pub fn write_template_config(force: bool) -> io::Result<PathBuf> {
    let path = config_path();
    if path.exists() && !force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already exists; pass --force to overwrite",
                path.display()
            ),
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, default_config_template())?;
    Ok(path)
}

/// Write a starter `init.scm` to `~/.config/mae/init.scm` if it doesn't exist.
/// Called by `mae --init-config`. Skip if file exists unless `force` is true.
pub fn write_init_template(force: bool) -> io::Result<PathBuf> {
    let dir = config_path().parent().unwrap().to_path_buf();
    let path = dir.join("init.scm");
    if path.exists() && !force {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "{} already exists; pass --force to overwrite",
                path.display()
            ),
        ));
    }
    fs::create_dir_all(&dir)?;
    fs::write(&path, default_init_template())?;
    Ok(path)
}

/// Starter init.scm template (Doom-style sections).
fn default_init_template() -> &'static str {
    r#";; MAE init.scm — Module declarations + configuration.
;; This is a real Scheme program, not a settings file.
;; Docs: :help guide:extension-authoring

;; ── Modules ──────────────────────────────────────────────
;; Uncomment modules to enable. Run `mae sync` after changes.
(mae!
  :editor
    "surround"          ; vim-surround (ds, cs, ys, S)
    "search"            ; /, ?, n, N, *, #
    "registers"         ; named registers (" in normal/visual)
    "macros"            ; macro recording (q, @)
    "marks-jumps"       ; marks, jump list, change list
    ;; (list "multicursor" "+align")  ; multi-cursor editing

  :ui
    "dashboard"         ; splash screen
    "file-tree"         ; project sidebar

  :lang
    "org"               ; org-mode keymap + hooks
    "tables"            ; table manipulation in org/markdown

  :app
    "dailies"           ; daily notes (SPC n d)
)

;; ── Third-party packages ─────────────────────────────────
;; Declare packages here, then run `mae sync` to install.
;; (package! "org-roam" :source "github:user/mae-org-roam")
;; (package! "my-theme" :source "github:user/mae-theme" :pin "abc123")
;; (package! "dashboard" :disable #t)  ; disable a built-in module

;; ── Appearance ──────────────────────────────────────────
(set-option! "theme" "default")
;; (set-option! "font-size" "14.0")

;; ── Editing ─────────────────────────────────────────────
;; (set-option! "relative-line-numbers" "true")
;; (set-option! "word-wrap" "true")

;; ── AI ──────────────────────────────────────────────────
;; (set-option! "ai-provider" "claude")

;; ── Daemon (KB persistence & hosting, ADR-035) ───────────
;; off (default) = in-process embedded KB only, no daemon needed.
;; on-demand = attach to / auto-spawn a daemon. shared = attach to an
;; existing daemon only, never spawn. Try `:eval (daemon-status)`.
;; (set-option! "daemon-mode" "off")

;; ── Keybindings ─────────────────────────────────────────
;; (define-key "normal" "SPC t t" "cycle-theme")

;; ── Hooks ───────────────────────────────────────────────
;; (add-hook! "before-save" "my-format-fn")
"#
}

/// Run `api_key_command` and return its trimmed stdout, or None on failure.
fn run_key_command(cmd: &Option<String>) -> Option<String> {
    let cmd = cmd.as_deref()?;
    if cmd.is_empty() {
        return None;
    }
    debug!(command = cmd, "running api_key_command");
    match std::process::Command::new("sh").args(["-c", cmd]).output() {
        Ok(output) if output.status.success() => {
            let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if key.is_empty() {
                warn!(command = cmd, "api_key_command produced empty output");
                None
            } else {
                Some(key)
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                command = cmd,
                status = %output.status,
                stderr = %stderr.trim(),
                "api_key_command failed — check the command in config.toml"
            );
            None
        }
        Err(e) => {
            warn!(command = cmd, error = %e, "api_key_command could not be executed");
            None
        }
    }
}

/// Overrides from Scheme init.scm (via `set-option!`).
/// Non-empty strings take precedence over TOML file values but not env vars.
pub struct SchemeAiOverrides {
    pub provider: String,
    pub model: String,
    pub api_key_command: String,
    pub base_url: String,
    pub thinking: String,
}

impl SchemeAiOverrides {
    /// Build from editor state. Empty strings mean "not set".
    pub fn from_editor(editor: &mae_core::Editor) -> Self {
        Self {
            provider: editor.ai.provider.clone(),
            model: editor.ai.model.clone(),
            api_key_command: editor.ai.api_key_command.clone(),
            base_url: editor.ai.base_url.clone(),
            thinking: editor.ai.thinking.clone(),
        }
    }

    fn opt(&self, field: &str) -> Option<String> {
        let val = match field {
            "provider" => &self.provider,
            "model" => &self.model,
            "api_key_command" => &self.api_key_command,
            "base_url" => &self.base_url,
            "thinking" => &self.thinking,
            _ => return None,
        };
        if val.is_empty() {
            None
        } else {
            Some(val.clone())
        }
    }
}

/// Resolve final AI `ProviderConfig` with precedence:
///   env > Scheme (init.scm) > TOML (config.toml) > defaults.
///
/// Returns `None` when no credentials or local endpoint are configured
/// anywhere — the AI is simply disabled in that case, not an error.
pub fn resolve_ai_config_with_scheme(
    file_config: &Config,
    scheme: &SchemeAiOverrides,
) -> Option<ProviderConfig> {
    let file = &file_config.ai;

    // Provider: env > scheme > file > "claude"
    let raw_provider = std::env::var("MAE_AI_PROVIDER")
        .ok()
        .or_else(|| scheme.opt("provider"))
        .or_else(|| file.provider.clone())
        .unwrap_or_else(|| "claude".into());

    // "deepseek" is syntactic sugar for an openai-compatible endpoint. "ollama"
    // keeps its own provider_type — it has a real native API (`/api/chat`)
    // that OllamaProvider talks to directly, distinct from its OpenAI-compatible
    // shim (which exists but doesn't forward the `think` field). See
    // bootstrap.rs::setup_ai for the dispatch and crates/ai/src/ollama.rs.
    let (provider_type, sugar_default_url) = match raw_provider.as_str() {
        "ollama" => (
            "ollama".to_string(),
            Some("http://localhost:11434".to_string()),
        ),
        "deepseek" => (
            "openai".to_string(),
            Some("https://api.deepseek.com/v1".to_string()),
        ),
        other => (other.to_string(), None),
    };

    // API key: env > scheme api_key_command > file api_key_command > file api_key
    // Check env var by raw_provider first (before sugar mapping).
    let effective_key_cmd = scheme
        .opt("api_key_command")
        .or_else(|| file.api_key_command.clone());
    let file_key = || run_key_command(&effective_key_cmd).or_else(|| file.api_key.clone());
    let api_key = match raw_provider.as_str() {
        "deepseek" => std::env::var("DEEPSEEK_API_KEY").ok().or_else(file_key),
        _ => match provider_type.as_str() {
            "openai" => std::env::var("OPENAI_API_KEY").ok().or_else(file_key),
            "gemini" => std::env::var("GEMINI_API_KEY").ok().or_else(file_key),
            // Local Ollama has no auth by default; still honor an explicit
            // key/command for authenticated deployments (reverse-proxied, etc).
            "ollama" => file_key(),
            _ => std::env::var("ANTHROPIC_API_KEY").ok().or_else(file_key),
        },
    };

    // Base URL: env > scheme > file > sugar-default
    let base_url = std::env::var("MAE_AI_BASE_URL")
        .ok()
        .or_else(|| scheme.opt("base_url"))
        .or_else(|| file.base_url.clone())
        .or(sugar_default_url);

    // If no auth path at all (no key, no local URL), AI is disabled.
    if api_key.is_none() && base_url.is_none() {
        return None;
    }

    // If a base URL is present but the provider is still "claude", switch
    // to openai-compatible (the Claude API doesn't accept arbitrary URLs).
    let provider_type = if base_url.is_some() && provider_type == "claude" {
        "openai".to_string()
    } else {
        provider_type
    };

    // Model: env > scheme > file > per-provider default
    let model = std::env::var("MAE_AI_MODEL")
        .ok()
        .or_else(|| scheme.opt("model"))
        .or_else(|| file.model.clone())
        .unwrap_or_else(|| match raw_provider.as_str() {
            "deepseek" => "deepseek-chat".to_string(),
            _ => match provider_type.as_str() {
                "openai" => "gpt-4o".to_string(),
                "gemini" => "gemini-2.5-flash".to_string(),
                // No universal default makes sense for locally-installed
                // models; this is a placeholder most users will override.
                "ollama" => "llama3.1".to_string(),
                _ => "claude-sonnet-4-20250514".to_string(),
            },
        });

    let timeout_secs = std::env::var("MAE_AI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or(file.timeout_secs)
        .unwrap_or(300);

    let max_tokens = file.max_tokens.unwrap_or(8192);

    // Thinking: env > scheme > unset (provider default). No TOML field —
    // this is a Scheme-first option (see options.rs `ai_thinking`), matching
    // ai_mode/ai_profile rather than the legacy config.toml-backed fields.
    let thinking = std::env::var("MAE_AI_THINKING")
        .ok()
        .or_else(|| scheme.opt("thinking"));

    Some(ProviderConfig {
        provider_type,
        api_key,
        model,
        base_url,
        max_tokens,
        temperature: file.temperature,
        timeout_secs,
        budget: file.budget.clone(),
        thinking,
    })
}

/// Backward-compatible wrapper: resolve without Scheme overrides.
/// Used by tests and `--check-config`.
#[cfg(test)]
pub fn resolve_ai_config(file_config: &Config) -> Option<ProviderConfig> {
    let empty = SchemeAiOverrides {
        provider: String::new(),
        model: String::new(),
        api_key_command: String::new(),
        base_url: String::new(),
        thinking: String::new(),
    };
    resolve_ai_config_with_scheme(file_config, &empty)
}

/// Resolve AI permission policy with precedence: env > file > default (trusted).
pub fn resolve_permission_policy(config: &Config) -> PermissionPolicy {
    let tier_str = std::env::var("MAE_AI_PERMISSIONS")
        .ok()
        .or_else(|| config.ai.auto_approve_tier.clone())
        .unwrap_or_else(|| "trusted".into());
    let tier = match tier_str.as_str() {
        "readonly" => PermissionTier::ReadOnly,
        "write" | "standard" => PermissionTier::Write,
        "shell" | "trusted" => PermissionTier::Shell,
        "privileged" | "full" => PermissionTier::Privileged,
        _ => {
            warn!(tier = %tier_str, "unknown AI permission tier, defaulting to 'shell'");
            PermissionTier::Shell
        }
    };
    PermissionPolicy {
        auto_approve_up_to: tier,
    }
}

/// Update a single editor preference in the config file (load → modify → save).
/// Silently logs on failure — preference persistence is best-effort.
pub fn persist_editor_preference(key: &str, value: &str) {
    let (mut cfg, _) = load_config();
    match key {
        "theme" => cfg.editor.theme = Some(value.to_string()),
        "splash_art" => cfg.editor.splash_art = Some(value.to_string()),
        "ai_editor" => cfg.ai.editor = Some(value.to_string()),
        "font_family" => cfg.editor.font_family = Some(value.to_string()),
        "icon_font_family" => cfg.editor.icon_font_family = Some(value.to_string()),
        "font_size" => cfg.editor.font_size = value.parse().ok(),
        "org_hide_emphasis_markers" => cfg.editor.org_hide_emphasis_markers = Some(value == "true"),
        "restore_session" => cfg.editor.restore_session = Some(value == "true"),
        "autosave_interval" => cfg.editor.autosave_interval = value.parse().ok(),
        _ => {
            warn!(key, value, "unknown editor preference key");
            return;
        }
    }
    if let Err(e) = save_config(&cfg) {
        warn!(key, value, error = %e, "failed to persist editor preference");
    } else {
        debug!(key, value, "persisted editor preference");
    }
}

/// Run the first-run configuration wizard. Returns `Ok(true)` if a config
/// was written, `Ok(false)` if the user skipped. Only runs when stdin is
/// a TTY and `MAE_SKIP_WIZARD` is not set.
///
/// pudb-inspired: a simple sequential prompt that writes a complete
/// `config.toml` the user can edit later.
pub fn maybe_run_first_run_wizard() -> io::Result<bool> {
    if std::env::var("MAE_SKIP_WIZARD").is_ok() {
        return Ok(false);
    }
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    if config_path().exists() {
        return Ok(false);
    }
    // Also skip if any AI env var is already set — the user clearly has
    // their own setup and we shouldn't interrupt them on every launch.
    if std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("OPENAI_API_KEY").is_ok()
        || std::env::var("GEMINI_API_KEY").is_ok()
        || std::env::var("DEEPSEEK_API_KEY").is_ok()
        || std::env::var("MAE_AI_BASE_URL").is_ok()
    {
        return Ok(false);
    }

    run_wizard().map(|_| true)
}

/// Interactive wizard body. Separated from the gating logic so it can be
/// invoked explicitly (`mae --init-config`) even when env vars are set.
///
/// Writes both config.toml (bootstrap: AI + theme) and init.scm (all options).
pub fn run_wizard() -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    writeln!(out)?;
    writeln!(out, "  MAE — first-run setup")?;
    writeln!(out, "  ---------------------")?;
    writeln!(out)?;
    writeln!(out, "  No config found at {}.", config_path().display())?;
    writeln!(
        out,
        "  Let's set one up. Press Enter to accept the default at each prompt."
    )?;
    writeln!(
        out,
        "  (You can re-run this any time with `mae --init-config --force`.)"
    )?;

    // Accumulate init.scm managed options
    let mut init_options: Vec<(String, String)> = Vec::new();
    let mut cfg = Config::default();

    // --- 1. AI Provider ---
    writeln!(out)?;
    writeln!(out, "  \x1b[1m1. AI Provider\x1b[0m")?;
    writeln!(
        out,
        "    1. claude   — Anthropic Claude (requires ANTHROPIC_API_KEY)"
    )?;
    writeln!(
        out,
        "    2. openai   — OpenAI API (requires OPENAI_API_KEY)"
    )?;
    writeln!(
        out,
        "    3. gemini   — Google Gemini (requires GEMINI_API_KEY)"
    )?;
    writeln!(
        out,
        "    4. ollama   — Local Ollama (no key, uses http://localhost:11434)"
    )?;
    writeln!(
        out,
        "    5. deepseek — DeepSeek API (requires DEEPSEEK_API_KEY)"
    )?;
    writeln!(out, "    6. skip     — Don't configure AI now")?;
    let choice = prompt(&mut out, "Choice [1-6, default=6]", "6")?;

    let (provider, ask_key, default_model, default_base) = match choice.as_str() {
        "1" | "claude" => ("claude", true, "claude-sonnet-4-20250514", None),
        "2" | "openai" => ("openai", true, "gpt-4o", None),
        "3" | "gemini" => ("gemini", true, "gemini-2.5-flash", None),
        "4" | "ollama" => ("ollama", false, "llama3", Some("http://localhost:11434")),
        "5" | "deepseek" => ("deepseek", true, "deepseek-chat", None),
        _ => ("", false, "", None),
    };

    if !provider.is_empty() {
        cfg.ai.provider = Some(provider.into());
        init_options.push(("ai_provider".into(), provider.into()));

        let model = prompt(
            &mut out,
            &format!("Model [{}]", default_model),
            default_model,
        )?;
        cfg.ai.model = Some(model.clone());
        init_options.push(("ai_model".into(), model));

        if let Some(base) = default_base {
            let base = prompt(&mut out, &format!("Base URL [{}]", base), base)?;
            cfg.ai.base_url = Some(base);
        } else {
            let base = prompt(&mut out, "Base URL (optional, leave blank for default)", "")?;
            if !base.is_empty() {
                cfg.ai.base_url = Some(base);
            }
        }

        if ask_key {
            writeln!(out)?;
            writeln!(out, "  API key storage:")?;
            writeln!(
                out,
                "    1. Environment variable (recommended) — reads ${}_API_KEY",
                match provider {
                    "openai" => "OPENAI",
                    "gemini" => "GEMINI",
                    "deepseek" => "DEEPSEEK",
                    _ => "ANTHROPIC",
                }
            )?;
            writeln!(out, "    2. Password manager — enter retrieval command")?;
            writeln!(
                out,
                "    3. Paste key directly (not recommended for production)"
            )?;
            let key_choice = prompt(&mut out, "Choice [1-3, default=1]", "1")?;
            match key_choice.as_str() {
                "2" => {
                    let suggestion = platform_key_storage_suggestion("mae-ai");
                    writeln!(out, "    Suggestion: {}", suggestion)?;
                    let cmd = prompt(&mut out, "Key command", suggestion)?;
                    if !cmd.is_empty() {
                        cfg.ai.api_key_command = Some(cmd.clone());
                        init_options.push(("ai_api_key_command".into(), cmd));
                    }
                }
                "3" => {
                    let key = prompt(&mut out, "API key", "")?;
                    if !key.is_empty() {
                        cfg.ai.api_key = Some(key);
                    }
                }
                _ => {
                    writeln!(out, "    Set the env var before launching MAE.")?;
                }
            }
        }
    }

    // --- 2. Theme ---
    writeln!(out)?;
    writeln!(out, "  \x1b[1m2. Theme\x1b[0m")?;
    let theme = prompt(&mut out, "Theme [default]", "default")?;
    cfg.editor.theme = Some(theme.clone());
    init_options.push(("theme".into(), theme));

    // --- 3. Collaboration ---
    writeln!(out)?;
    writeln!(out, "  \x1b[1m3. Collaboration\x1b[0m")?;
    writeln!(out, "    1. solo      — No collaboration (default)")?;
    writeln!(
        out,
        "    2. loopback  — Local daemon, multi-window (127.0.0.1:9473)"
    )?;
    writeln!(
        out,
        "    3. network   — Multi-machine (prompts for address + PSK)"
    )?;
    writeln!(out, "    4. skip")?;
    let collab_choice = prompt(&mut out, "Choice [1-4, default=1]", "1")?;

    match collab_choice.as_str() {
        "2" | "loopback" => {
            cfg.collaboration.server_address = Some("127.0.0.1:9473".into());
            cfg.collaboration.auto_connect = Some(true);
            init_options.push(("collab_server_address".into(), "127.0.0.1:9473".into()));
            init_options.push(("collab_auto_connect".into(), "true".into()));
            writeln!(out, "    Loopback mode configured.")?;
        }
        "3" | "network" => {
            let addr = prompt(&mut out, "Server address [0.0.0.0:9473]", "0.0.0.0:9473")?;
            cfg.collaboration.server_address = Some(addr.clone());
            cfg.collaboration.auto_connect = Some(true);
            init_options.push(("collab_server_address".into(), addr));
            init_options.push(("collab_auto_connect".into(), "true".into()));

            writeln!(out)?;
            writeln!(out, "  PSK authentication (recommended for network mode):")?;
            let psk_suggestion = platform_key_storage_suggestion("mae-collab-psk");
            writeln!(out, "    Suggestion: {}", psk_suggestion)?;
            let psk_cmd = prompt(&mut out, "PSK command (blank = no auth)", "")?;
            if !psk_cmd.is_empty() {
                cfg.collaboration.psk_command = Some(psk_cmd.clone());
                init_options.push(("collab_psk_command".into(), psk_cmd));
            }
        }
        _ => { /* solo or skip */ }
    }

    // --- 4. KB Notes Directory ---
    writeln!(out)?;
    writeln!(out, "  \x1b[1m4. KB Notes Directory\x1b[0m")?;
    let default_notes = platform_default_notes_dir();
    let notes_dir = prompt(
        &mut out,
        &format!("Notes directory [{}]", default_notes.display()),
        &default_notes.display().to_string(),
    )?;
    if !notes_dir.is_empty() {
        let expanded = shellexpand_tilde(&notes_dir);
        let _ = fs::create_dir_all(&expanded);
        init_options.push(("kb_notes_dir".into(), notes_dir));
        writeln!(out, "    Created {}", expanded)?;
    }

    // --- 5. Daemon ---
    writeln!(out)?;
    writeln!(out, "  \x1b[1m5. Daemon\x1b[0m")?;
    let enable_daemon = prompt(&mut out, "Enable background daemon? [Y/n]", "Y")?;
    let daemon_enabled = !matches!(enable_daemon.to_lowercase().as_str(), "n" | "no");
    if daemon_enabled {
        cfg.daemon.enabled = Some(true);
        init_options.push(("daemon_mode".into(), "on-demand".into()));
        writeln!(out, "    Daemon enabled (daemon_mode = on-demand).")?;
        let svc = detect_platform_service_manager();
        match svc {
            ServiceManager::Homebrew => {
                writeln!(out, "    Start with: brew services start mae")?;
            }
            ServiceManager::Launchd => {
                writeln!(
                    out,
                    "    Start with: launchctl load ~/Library/LaunchAgents/com.cuttlefisch.mae-daemon.plist"
                )?;
            }
            ServiceManager::Systemd => {
                writeln!(
                    out,
                    "    Start with: systemctl --user enable --now mae-daemon"
                )?;
            }
            ServiceManager::None => {
                writeln!(out, "    Start manually: mae-daemon &")?;
            }
        }
    }

    // --- Write config files ---
    // config.toml (bootstrap: AI + theme for pre-Scheme loading)
    let toml_path = save_config(&cfg)?;

    // init.scm (all managed options)
    let init_path = write_managed_init_options(&init_options)?;

    writeln!(out)?;
    writeln!(out, "  Wrote {}", toml_path.display())?;
    writeln!(out, "  Wrote {}", init_path.display())?;
    writeln!(out, "  Launching MAE...")?;
    writeln!(out)?;
    info!(path = %toml_path.display(), "first-run wizard complete");
    Ok(())
}

/// Write `(set-option!)` calls to init.scm between sentinel markers.
/// Preserves any existing content in the file.
fn write_managed_init_options(options: &[(String, String)]) -> io::Result<PathBuf> {
    let dir = config_path().parent().unwrap().to_path_buf();
    let init_path = dir.join("init.scm");
    fs::create_dir_all(&dir)?;

    let existing = if init_path.exists() {
        fs::read_to_string(&init_path)?
    } else {
        default_init_template().to_string()
    };

    const MARKER_START: &str = ";; --- MAE managed options ---";
    const MARKER_END: &str = ";; --- end managed options ---";

    let managed_block: String = options
        .iter()
        .map(|(k, v)| format!("(set-option! \"{}\" \"{}\")", k, v))
        .collect::<Vec<_>>()
        .join("\n");

    let new_content = if existing.contains(MARKER_START) {
        // Replace the entire managed section
        if let (Some(start), Some(end)) = (existing.find(MARKER_START), existing.find(MARKER_END)) {
            let before = &existing[..start];
            let after = &existing[end + MARKER_END.len()..];
            format!(
                "{}{}\n{}\n{}{}",
                before, MARKER_START, managed_block, MARKER_END, after
            )
        } else {
            format!(
                "{}\n\n{}\n{}\n{}\n",
                existing.trim_end(),
                MARKER_START,
                managed_block,
                MARKER_END
            )
        }
    } else {
        format!(
            "{}\n\n{}\n{}\n{}\n",
            existing.trim_end(),
            MARKER_START,
            managed_block,
            MARKER_END
        )
    };

    fs::write(&init_path, new_content)?;
    Ok(init_path)
}

/// Expand leading `~` to `$HOME`.
fn shellexpand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen('~', &home, 1);
        }
    }
    path.to_string()
}

/// Platform-appropriate default notes directory.
fn platform_default_notes_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Documents").join("mae-notes")
    } else {
        PathBuf::from(home).join("mae-notes")
    }
}

/// Suggest a platform-appropriate password manager command.
fn platform_key_storage_suggestion(service: &str) -> &'static str {
    if cfg!(target_os = "macos") {
        match service {
            "mae-ai" => "security find-generic-password -s mae-ai -w",
            "mae-collab-psk" => "security find-generic-password -s mae-collab-psk -a mae -w",
            _ => "security find-generic-password -s mae -w",
        }
    } else {
        match service {
            "mae-ai" => "pass show mae/api-key",
            "mae-collab-psk" => "pass show mae/collab-psk",
            _ => "pass show mae/secret",
        }
    }
}

/// Detected service manager for daemon start instructions.
enum ServiceManager {
    Homebrew,
    Launchd,
    Systemd,
    None,
}

/// Return Homebrew's prefix (`brew --prefix`) if brew is installed, else None.
/// Used both for service-manager detection and by the self-upgrade channel
/// classifier (`crate::upgrade`) to tell a brew-installed binary apart from a
/// tarball/source install.
pub(crate) fn brew_prefix() -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("brew")
        .args(["--prefix"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(path))
    }
}

/// Detect which service manager is available.
fn detect_platform_service_manager() -> ServiceManager {
    if cfg!(target_os = "macos") {
        // Check Homebrew first
        if brew_prefix().is_some() {
            return ServiceManager::Homebrew;
        }
        ServiceManager::Launchd
    } else if std::process::Command::new("systemctl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        ServiceManager::Systemd
    } else {
        ServiceManager::None
    }
}

/// Read a line from stdin with a prompt. Returns the default if the user
/// presses Enter without typing.
fn prompt(out: &mut impl Write, label: &str, default: &str) -> io::Result<String> {
    write!(out, "  {}: ", label)?;
    out.flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Comment header prepended to saved configs so users learn the file format
/// without having to read the source.
fn default_config_header() -> String {
    let path = config_path();
    format!(
        "# MAE configuration\n\
         # Location: {}\n\
         # Env vars always take precedence over values set here.\n\
         # Full docs: :help config   (inside the editor)\n\
         \n",
        path.display()
    )
}

/// Full commented template used by `mae --init-config` and
/// `mae --print-config-template`.
pub fn default_config_template() -> String {
    format!(
        "# MAE configuration\n\
# Location: {}\n\
# Env vars always take precedence over values set here.\n\
\n\
[ai]\n\
# Provider: \"claude\" | \"openai\" | \"gemini\" | \"ollama\" | \"deepseek\"\n\
# (\"ollama\" and \"deepseek\" are shortcuts for openai-compatible + provider URL)\n\
# provider = \"claude\"\n\
\n\
# Model identifier. Leave unset for the provider default.\n\
# Claude defaults:  claude-sonnet-4-20250514  (also: claude-opus-4-6, claude-haiku-4-5-20251001)\n\
# OpenAI defaults:  gpt-4o\n\
# Gemini defaults:  gemini-2.5-flash (also: gemini-3.1-pro, gemini-3.1-flash-lite)\n\
# Ollama examples:  llama3, codellama, qwen2.5-coder\n\
# model = \"claude-sonnet-4-20250514\"\n\
\n\
# Base URL for the API. Leave unset for provider defaults.\n\
# base_url = \"http://localhost:11434/v1\"\n\
\n\
# API key. If unset, env vars are read:\n\
#   ANTHROPIC_API_KEY — https://console.anthropic.com/settings/keys\n\
#   OPENAI_API_KEY    — https://platform.openai.com/api-keys\n\
#   GEMINI_API_KEY    — https://aistudio.google.com/apikey\n\
#   DEEPSEEK_API_KEY  — https://platform.deepseek.com/api_keys\n\
# Ollama doesn't need a key.\n\
# api_key = \"...\"\n\
\n\
# Shell command to retrieve API key (e.g. from pass, 1Password, etc.).\n\
# Stdout is trimmed and used as the key. Takes precedence over api_key but not env vars.\n\
# api_key_command = \"pass show deepseek/api-key\"\n\
\n\
# Permission tier for AI/MCP tool execution.\n\
# Tiers: \"readonly\", \"write\", \"shell\" (default), \"privileged\"\n\
# Env override: MAE_AI_PERMISSIONS=full\n\
# auto_approve_tier = \"shell\"\n\
\n\
# Override auto-detected prompt tier: \"full\" or \"compact\".\n\
# Full: concise prompts for frontier models (Claude Opus/Sonnet, GPT-4o).\n\
# Compact: explicit guardrails for smaller models (DeepSeek, Haiku).\n\
# prompt_tier = \"compact\"\n\
\n\
# HTTP timeout in seconds. Increase for slow local inference.\n\
# timeout_secs = 300\n\
\n\
# Response sampling.\n\
# max_tokens = 8192\n\
# temperature = 0.7\n\
\n\
# Per-session spend guardrails (USD). Both optional.\n\
# - session_warn_usd: one-shot *Messages* warning once this is crossed\n\
# - session_hard_cap_usd: refuse new requests once this is reached\n\
# Unknown-priced models (Ollama) are treated as free, so these only\n\
# apply to paid APIs. Restart the editor to reset the counter.\n\
[ai.budget]\n\
# session_warn_usd = 0.25\n\
# session_hard_cap_usd = 1.00\n\
\n\
[editor]\n\
# Bundled themes: default, dark-ansi, light-ansi, gruvbox-dark, gruvbox-light, dracula, catppuccin-mocha, solarized-dark, one-dark\n\
# theme = \"default\"\n\
\n\
# Splash screen art: \"bat\" (more variants coming)\n\
# splash_art = \"bat\"\n\
\n\
# Font family for GUI mode (--gui). Nerd Font variants recommended for icons.\n\
# font_family = \"JetBrainsMono Nerd Font Mono\"\n\
# font_size = 14.0\n\
\n\
[agents]\n\
# Automatically write .mcp.json to the project root on :terminal spawn.\n\
# Claude Code and other MCP clients will auto-discover MAE's tools.\n\
# Set to false to disable. Env override: MAE_AGENTS_AUTO_MCP=0\n\
# auto_mcp_json = true\n\
\n\
# Automatically configure spawned agents to trust MAE's MCP tools.\n\
# Writes agent-specific settings (e.g. .claude/settings.local.json)\n\
# so MCP tools run without per-call approval prompts.\n\
# Set to false to disable. Env override: MAE_AGENTS_AUTO_APPROVE=0\n\
# auto_approve_tools = true\n\
\n\
# [performance]\n\
# large_file_lines = 5000\n\
# degrade_threshold_chars = 500000\n\
# degrade_threshold_line_length = 10000\n\
# syntax_reparse_debounce_ms = 50\n\
\n\
# [lsp]\n\
# Per-language LSP server configuration. Any language key is valid.\n\
# Env var overrides: MAE_LSP_RUST, MAE_LSP_PYTHON, MAE_LSP_TYPESCRIPT, etc.\n\
# [lsp.rust]\n\
# command = \"rust-analyzer\"\n\
# [lsp.python]\n\
# command = \"pylsp\"\n\
# [lsp.typescript]\n\
# command = \"typescript-language-server\"\n\
# args = [\"--stdio\"]\n\
# [lsp.go]\n\
# command = \"gopls\"\n\
",
        config_path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that manipulate environment variables must hold this lock
    /// to avoid races when cargo runs tests in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_config_is_empty() {
        let cfg = Config::default();
        assert!(cfg.ai.provider.is_none());
        assert!(cfg.ai.model.is_none());
        assert!(cfg.editor.theme.is_none());
    }

    #[test]
    fn config_round_trips_toml() {
        let cfg = Config {
            ai: AiSection {
                provider: Some("ollama".into()),
                model: Some("llama3".into()),
                base_url: Some("http://localhost:11434/v1".into()),
                ..Default::default()
            },
            editor: EditorSection {
                theme: Some("gruvbox-dark".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.ai.provider.as_deref(), Some("ollama"));
        assert_eq!(back.ai.model.as_deref(), Some("llama3"));
        assert_eq!(back.editor.theme.as_deref(), Some("gruvbox-dark"));
    }

    #[test]
    fn partial_config_parses_with_defaults() {
        let s = r#"
            [ai]
            provider = "claude"
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(cfg.ai.provider.as_deref(), Some("claude"));
        assert!(cfg.ai.model.is_none());
        assert!(cfg.editor.theme.is_none());
    }

    #[test]
    fn resolve_ollama_sets_base_url() {
        let _lock = ENV_LOCK.lock().unwrap();
        let cfg = Config {
            ai: AiSection {
                provider: Some("ollama".into()),
                model: Some("llama3".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Ensure no env vars leak in
        std::env::remove_var("MAE_AI_PROVIDER");
        std::env::remove_var("MAE_AI_BASE_URL");
        std::env::remove_var("MAE_AI_MODEL");
        let resolved = resolve_ai_config(&cfg).expect("ollama without key should still work");
        // "ollama" keeps its own provider_type now — OllamaProvider talks to
        // the native /api/chat endpoint, not the OpenAI-compatible shim
        // (which doesn't forward the `think` field). See ollama.rs.
        assert_eq!(resolved.provider_type, "ollama");
        assert_eq!(resolved.model, "llama3");
        let base_url = resolved.base_url.as_deref().unwrap();
        assert!(base_url.contains("localhost"));
        assert!(
            !base_url.ends_with("/v1"),
            "native Ollama endpoint is unversioned, unlike the OpenAI-compat shim"
        );
    }

    #[test]
    fn resolve_gemini_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("GEMINI_API_KEY", "gemini-key");
        std::env::remove_var("MAE_AI_PROVIDER");
        let cfg = Config {
            ai: AiSection {
                provider: Some("gemini".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_ai_config(&cfg).unwrap();
        assert_eq!(resolved.provider_type, "gemini");
        assert_eq!(resolved.api_key.as_deref(), Some("gemini-key"));
        assert_eq!(resolved.model, "gemini-2.5-flash"); // default
        std::env::remove_var("GEMINI_API_KEY");
    }

    #[test]
    fn resolve_deepseek_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("DEEPSEEK_API_KEY", "ds-key");
        std::env::remove_var("MAE_AI_PROVIDER");
        std::env::remove_var("MAE_AI_BASE_URL");
        std::env::remove_var("MAE_AI_MODEL");
        let cfg = Config {
            ai: AiSection {
                provider: Some("deepseek".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_ai_config(&cfg).unwrap();
        assert_eq!(resolved.provider_type, "openai"); // sugar maps to openai
        assert_eq!(resolved.api_key.as_deref(), Some("ds-key"));
        assert_eq!(resolved.model, "deepseek-chat"); // default model
        assert!(resolved
            .base_url
            .as_deref()
            .unwrap()
            .contains("deepseek.com"));
        std::env::remove_var("DEEPSEEK_API_KEY");
    }

    #[test]
    fn resolve_api_key_command() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("MAE_AI_PROVIDER");
        std::env::remove_var("MAE_AI_BASE_URL");
        std::env::remove_var("MAE_AI_MODEL");
        let cfg = Config {
            ai: AiSection {
                provider: Some("claude".into()),
                api_key_command: Some("echo secret-from-command".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_ai_config(&cfg).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("secret-from-command"));
    }

    #[test]
    fn resolve_env_overrides_api_key_command() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("ANTHROPIC_API_KEY", "env-key");
        std::env::remove_var("MAE_AI_PROVIDER");
        std::env::remove_var("MAE_AI_BASE_URL");
        std::env::remove_var("MAE_AI_MODEL");
        let cfg = Config {
            ai: AiSection {
                provider: Some("claude".into()),
                api_key_command: Some("echo command-key".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_ai_config(&cfg).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("env-key"));
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn resolve_no_credentials_returns_none() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("MAE_AI_BASE_URL");
        std::env::remove_var("MAE_AI_PROVIDER");
        let cfg = Config::default();
        assert!(resolve_ai_config(&cfg).is_none());
    }

    #[test]
    fn resolve_file_model_overridden_by_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("MAE_AI_MODEL", "env-model");
        std::env::set_var("ANTHROPIC_API_KEY", "test-key");
        std::env::remove_var("MAE_AI_PROVIDER");
        std::env::remove_var("MAE_AI_BASE_URL");
        let cfg = Config {
            ai: AiSection {
                provider: Some("claude".into()),
                model: Some("file-model".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_ai_config(&cfg).unwrap();
        assert_eq!(resolved.model, "env-model");
        std::env::remove_var("MAE_AI_MODEL");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    // --- Permission policy resolution tests ---

    #[test]
    fn resolve_permission_default_is_trusted() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MAE_AI_PERMISSIONS");
        let cfg = Config::default();
        let policy = resolve_permission_policy(&cfg);
        assert_eq!(policy.auto_approve_up_to, PermissionTier::Shell);
    }

    #[test]
    fn resolve_permission_from_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MAE_AI_PERMISSIONS");
        let cfg = Config {
            ai: AiSection {
                auto_approve_tier: Some("full".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let policy = resolve_permission_policy(&cfg);
        assert_eq!(policy.auto_approve_up_to, PermissionTier::Privileged);
    }

    #[test]
    fn resolve_permission_env_overrides_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("MAE_AI_PERMISSIONS", "readonly");
        let cfg = Config {
            ai: AiSection {
                auto_approve_tier: Some("full".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let policy = resolve_permission_policy(&cfg);
        assert_eq!(policy.auto_approve_up_to, PermissionTier::ReadOnly);
        std::env::remove_var("MAE_AI_PERMISSIONS");
    }

    #[test]
    fn resolve_permission_all_tiers() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MAE_AI_PERMISSIONS");
        let tiers = [
            ("readonly", PermissionTier::ReadOnly),
            ("standard", PermissionTier::Write),
            ("trusted", PermissionTier::Shell),
            ("full", PermissionTier::Privileged),
        ];
        for (name, expected) in tiers {
            let cfg = Config {
                ai: AiSection {
                    auto_approve_tier: Some(name.into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let policy = resolve_permission_policy(&cfg);
            assert_eq!(
                policy.auto_approve_up_to, expected,
                "tier '{}' mismatch",
                name
            );
        }
    }

    #[test]
    fn resolve_permission_unknown_tier_defaults_to_trusted() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MAE_AI_PERMISSIONS");
        let cfg = Config {
            ai: AiSection {
                auto_approve_tier: Some("bogus".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let policy = resolve_permission_policy(&cfg);
        assert_eq!(policy.auto_approve_up_to, PermissionTier::Shell);
    }

    #[test]
    fn config_with_permission_tier_round_trips() {
        let cfg = Config {
            ai: AiSection {
                auto_approve_tier: Some("full".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.ai.auto_approve_tier.as_deref(), Some("full"));
    }

    // --- Agent auto-approve tests ---

    #[test]
    fn auto_approve_tools_defaults_to_true() {
        let cfg = Config::default();
        assert!(cfg.agents.auto_approve_tools);
    }

    #[test]
    fn auto_approve_tools_env_override() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var("MAE_AGENTS_AUTO_APPROVE", "0");
        let cfg = Config::default();
        assert!(!cfg.agents.auto_approve_tools_effective());
        std::env::remove_var("MAE_AGENTS_AUTO_APPROVE");
    }

    #[test]
    fn auto_approve_tools_config_false() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var("MAE_AGENTS_AUTO_APPROVE");
        let s = r#"
            [agents]
            auto_approve_tools = false
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert!(!cfg.agents.auto_approve_tools_effective());
    }

    #[test]
    fn auto_approve_tools_round_trips() {
        let cfg = Config {
            agents: AgentsSection {
                auto_mcp_json: true,
                auto_approve_tools: false,
            },
            ..Default::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert!(!back.agents.auto_approve_tools);
    }

    #[test]
    fn lsp_section_parses() {
        let s = r#"
            [lsp.rust]
            command = "rust-analyzer"

            [lsp.python]
            command = "pylsp"

            [lsp.typescript]
            command = "typescript-language-server"
            args = ["--stdio"]
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(cfg.lsp.servers.len(), 3);
        assert_eq!(cfg.lsp.servers["rust"].command, "rust-analyzer");
        assert_eq!(cfg.lsp.servers["python"].command, "pylsp");
        assert_eq!(
            cfg.lsp.servers["typescript"].command,
            "typescript-language-server"
        );
        assert_eq!(cfg.lsp.servers["typescript"].args, vec!["--stdio"]);
        assert!(cfg.lsp.servers["rust"].args.is_empty());
    }

    #[test]
    fn lsp_section_empty_by_default() {
        let cfg = Config::default();
        assert!(cfg.lsp.servers.is_empty());
    }

    #[test]
    fn lsp_section_round_trips() {
        let mut servers = std::collections::HashMap::new();
        servers.insert(
            "zig".to_string(),
            LspLanguageConfig {
                command: "zls".into(),
                args: vec![],
            },
        );
        let cfg = Config {
            lsp: LspSection { servers },
            ..Default::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.lsp.servers["zig"].command, "zls");
    }

    // --- PSK config deserialization tests ---

    #[test]
    fn collab_psk_command_parses_from_toml() {
        let s = r#"
            [collaboration]
            psk_command = "cat ~/.config/mae/collab-psk.txt"
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(
            cfg.collaboration.psk_command.as_deref(),
            Some("cat ~/.config/mae/collab-psk.txt")
        );
        assert!(cfg.collaboration.psk.is_none());
    }

    #[test]
    fn collab_psk_plaintext_parses_from_toml() {
        let s = r#"
            [collaboration]
            psk = "my-secret-key"
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(cfg.collaboration.psk.as_deref(), Some("my-secret-key"));
        assert!(cfg.collaboration.psk_command.is_none());
    }

    #[test]
    fn collab_psk_both_fields_parse_from_toml() {
        let s = r#"
            [collaboration]
            server_address = "192.168.1.10:9473"
            psk_command = "pass show mae/psk"
            psk = "fallback-key"
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(
            cfg.collaboration.psk_command.as_deref(),
            Some("pass show mae/psk")
        );
        assert_eq!(cfg.collaboration.psk.as_deref(), Some("fallback-key"));
        assert_eq!(
            cfg.collaboration.server_address.as_deref(),
            Some("192.168.1.10:9473")
        );
    }

    #[test]
    fn collab_kb_sync_mode_parses_from_toml() {
        let s = r#"
            [collaboration]
            kb_sync_mode = "manual"
        "#;
        let cfg: Config = toml::from_str(s).unwrap();
        assert_eq!(cfg.collaboration.kb_sync_mode.as_deref(), Some("manual"));
    }

    #[test]
    fn collab_section_round_trips() {
        let cfg = Config {
            collaboration: CollaborationSection {
                psk_command: Some("pass show mae/psk".into()),
                psk: Some("fallback".into()),
                kb_sync_mode: Some("on_save".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(
            back.collaboration.psk_command.as_deref(),
            Some("pass show mae/psk")
        );
        assert_eq!(back.collaboration.psk.as_deref(), Some("fallback"));
        assert_eq!(back.collaboration.kb_sync_mode.as_deref(), Some("on_save"));
    }
}
