//! MAE configuration file loading, precedence resolution, and first-run wizard.
//!
//! Precedence (highest → lowest):
//!   1. Environment variables (MAE_AI_PROVIDER, ANTHROPIC_API_KEY, MAE_AI_MODEL, …)
//!   2. `$XDG_CONFIG_HOME/mae/config.toml` (defaults to `~/.config/mae/config.toml`)
//!   3. Built-in defaults
//!
//! The first-run wizard writes a complete `config.toml` on first launch when
//! stdin is a TTY and no config file is present. Mirrors pudb's first-run
//! preferences dialog but as a simple stdio prompt (runs before the TUI starts).
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
    #[serde(default)]
    pub ai: AiSection,
    #[serde(default)]
    pub editor: EditorSection,
    #[serde(default)]
    pub agents: AgentsSection,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiSection {
    /// "claude" | "openai" | "ollama"
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub timeout_secs: Option<u64>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    /// Permission tier for AI/MCP tool execution:
    ///   "readonly"  — buffer reads only
    ///   "standard"  — reads + edits
    ///   "trusted"   — reads + edits + shell (default, container-first)
    ///   "full"      — everything including quit/force-quit
    /// Env override: MAE_AI_PERMISSIONS (highest precedence).
    pub auto_approve_tier: Option<String>,
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
    pub font_size: Option<f32>,
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

/// Load the config file, returning `Config::default()` if it doesn't exist or
/// is unreadable. Parse errors are logged (not fatal).
pub fn load_config() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<Config>(&contents) {
            Ok(cfg) => {
                debug!(path = %path.display(), "loaded config");
                cfg
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "config parse failed, using defaults");
                Config::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            debug!(path = %path.display(), "no config file, using defaults");
            Config::default()
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "config read failed, using defaults");
            Config::default()
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

/// Resolve final AI `ProviderConfig` with precedence: env > file > defaults.
///
/// Returns `None` when no credentials or local endpoint are configured
/// anywhere — the AI is simply disabled in that case, not an error.
pub fn resolve_ai_config(file_config: &Config) -> Option<ProviderConfig> {
    let file = &file_config.ai;

    // Provider: env > file > "claude"
    let raw_provider = std::env::var("MAE_AI_PROVIDER")
        .ok()
        .or_else(|| file.provider.clone())
        .unwrap_or_else(|| "claude".into());

    // "ollama" is syntactic sugar for openai-compatible + local URL.
    let (provider_type, ollama_default_url) = match raw_provider.as_str() {
        "ollama" => (
            "openai".to_string(),
            Some("http://localhost:11434/v1".to_string()),
        ),
        other => (other.to_string(), None),
    };

    // API key: env (provider-specific) > file
    let api_key = match provider_type.as_str() {
        "openai" => std::env::var("OPENAI_API_KEY")
            .ok()
            .or_else(|| file.api_key.clone()),
        _ => std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .or_else(|| file.api_key.clone()),
    };

    // Base URL: env > file > ollama-default
    let base_url = std::env::var("MAE_AI_BASE_URL")
        .ok()
        .or_else(|| file.base_url.clone())
        .or(ollama_default_url);

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

    // Model: env > file > per-provider default
    let model = std::env::var("MAE_AI_MODEL")
        .ok()
        .or_else(|| file.model.clone())
        .unwrap_or_else(|| match provider_type.as_str() {
            "openai" => "gpt-4o".to_string(),
            _ => "claude-sonnet-4-5".to_string(),
        });

    let timeout_secs = std::env::var("MAE_AI_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .or(file.timeout_secs)
        .unwrap_or(300);

    let max_tokens = file.max_tokens.unwrap_or(8192);

    Some(ProviderConfig {
        provider_type,
        api_key,
        model,
        base_url,
        max_tokens,
        temperature: file.temperature,
        timeout_secs,
        budget: file.budget.clone(),
    })
}

/// Resolve AI permission policy with precedence: env > file > default (trusted).
pub fn resolve_permission_policy(config: &Config) -> PermissionPolicy {
    let tier_str = std::env::var("MAE_AI_PERMISSIONS")
        .ok()
        .or_else(|| config.ai.auto_approve_tier.clone())
        .unwrap_or_else(|| "trusted".into());
    let tier = match tier_str.as_str() {
        "readonly" => PermissionTier::ReadOnly,
        "standard" => PermissionTier::Write,
        "trusted" => PermissionTier::Shell,
        "full" => PermissionTier::Privileged,
        _ => {
            warn!(tier = %tier_str, "unknown AI permission tier, defaulting to 'trusted'");
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
    let mut cfg = load_config();
    match key {
        "theme" => cfg.editor.theme = Some(value.to_string()),
        "splash_art" => cfg.editor.splash_art = Some(value.to_string()),
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
        || std::env::var("MAE_AI_BASE_URL").is_ok()
    {
        return Ok(false);
    }

    run_wizard().map(|_| true)
}

/// Interactive wizard body. Separated from the gating logic so it can be
/// invoked explicitly (`mae --init-config`) even when env vars are set.
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
        "  (You can re-run this any time with `mae --init-config`.)"
    )?;
    writeln!(out)?;

    writeln!(out, "  AI provider:")?;
    writeln!(
        out,
        "    1. claude  — Anthropic Claude (requires ANTHROPIC_API_KEY)"
    )?;
    writeln!(out, "    2. openai  — OpenAI API (requires OPENAI_API_KEY)")?;
    writeln!(
        out,
        "    3. ollama  — Local Ollama (no key, uses http://localhost:11434)"
    )?;
    writeln!(out, "    4. skip    — Don't configure AI now")?;
    let choice = prompt(&mut out, "Choice [1-4, default=4]", "4")?;

    let mut cfg = Config::default();

    let (provider, ask_key, default_model, default_base) = match choice.as_str() {
        "1" | "claude" => ("claude", true, "claude-sonnet-4-5", None),
        "2" | "openai" => ("openai", true, "gpt-4o", None),
        "3" | "ollama" => ("ollama", false, "llama3", Some("http://localhost:11434/v1")),
        _ => {
            writeln!(
                out,
                "  Skipped. Written empty config so the wizard won't run again."
            )?;
            let path = save_config(&cfg)?;
            writeln!(out, "  Wrote {}", path.display())?;
            writeln!(out)?;
            return Ok(());
        }
    };

    cfg.ai.provider = Some(provider.into());

    let model = prompt(
        &mut out,
        &format!("Model [{}]", default_model),
        default_model,
    )?;
    cfg.ai.model = Some(model);

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
        writeln!(
            out,
            "  API key: leave blank to keep reading ${}_API_KEY from the environment.",
            if provider == "openai" {
                "OPENAI"
            } else {
                "ANTHROPIC"
            }
        )?;
        let key = prompt(&mut out, "API key (blank = env var)", "")?;
        if !key.is_empty() {
            cfg.ai.api_key = Some(key);
        }
    }

    let theme = prompt(&mut out, "Theme [default]", "default")?;
    cfg.editor.theme = Some(theme);

    let path = save_config(&cfg)?;
    writeln!(out)?;
    writeln!(out, "  Wrote {}", path.display())?;
    writeln!(out, "  Launching MAE...")?;
    writeln!(out)?;
    info!(path = %path.display(), "first-run wizard complete");
    Ok(())
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
# Provider: \"claude\" | \"openai\" | \"ollama\"\n\
# (\"ollama\" is a shortcut for openai-compatible + http://localhost:11434/v1)\n\
# provider = \"claude\"\n\
\n\
# Model identifier. Leave unset for the provider default.\n\
# Claude defaults:  claude-sonnet-4-5  (also: claude-opus-4-6, claude-haiku-4-5-20251001)\n\
# OpenAI defaults:  gpt-4o\n\
# Ollama examples:  llama3, codellama, qwen2.5-coder\n\
# model = \"claude-sonnet-4-5\"\n\
\n\
# Base URL for the API. Leave unset for provider defaults.\n\
# base_url = \"http://localhost:11434/v1\"\n\
\n\
# API key. If unset, ANTHROPIC_API_KEY / OPENAI_API_KEY env vars are read.\n\
# Ollama doesn't need a key.\n\
# api_key = \"...\"\n\
\n\
# Permission tier for AI/MCP tool execution.\n\
# Tiers: \"readonly\", \"standard\", \"trusted\" (default), \"full\"\n\
# Env override: MAE_AI_PERMISSIONS=full\n\
# auto_approve_tier = \"trusted\"\n\
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
# Bundled themes: default, gruvbox-dark, nord, tokyo-night, catppuccin, solarized-light, dracula\n\
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
        assert_eq!(resolved.provider_type, "openai");
        assert_eq!(resolved.model, "llama3");
        assert!(resolved.base_url.as_deref().unwrap().contains("localhost"));
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
}
