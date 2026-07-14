//! Editor option registry — single source of truth for all configurable options.
//!
//! Every option has a canonical name (underscore-separated, used by `:set`),
//! optional aliases (hyphen-separated, used by Scheme's `set-option!`),
//! documentation, type, default value, and an optional config.toml key path.
//!
//! Uses `Cow<'static, str>` so built-in options pay zero allocation cost
//! while module-defined options can register at runtime with owned Strings.
//!
//! The registry is queried by:
//! - `:set` ex-command handler
//! - `set-option!` Scheme function (via `Editor::set_option`)
//! - `execute_set_option` AI tool (via `Editor::set_option`)
//! - `describe-option` command
//! - KB auto-generation (`install_option_nodes`)

use std::borrow::Cow;

/// The kind of value an option accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    Int,
    String,
    Float,
    Theme,
}

impl std::fmt::Display for OptionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptionKind::Bool => write!(f, "boolean"),
            OptionKind::Int => write!(f, "integer"),
            OptionKind::String => write!(f, "string"),
            OptionKind::Float => write!(f, "float"),
            OptionKind::Theme => write!(f, "theme name"),
        }
    }
}

/// Metadata for a single editor option.
///
/// Uses `Cow<'static, str>` so built-in options use zero-cost `&'static str`
/// while module-defined options can register with owned `String` at runtime.
pub struct OptionDef {
    /// Canonical name: `"line_numbers"` (underscore-separated).
    pub name: Cow<'static, str>,
    /// Alternative names (e.g. Scheme hyphenated form): `["line-numbers"]`.
    pub aliases: Vec<Cow<'static, str>>,
    /// Human-readable documentation.
    pub doc: Cow<'static, str>,
    /// Value type.
    pub kind: OptionKind,
    /// Default value as a string.
    pub default_value: Cow<'static, str>,
    /// TOML path in config.toml, if persistable. e.g. `"editor.line_numbers"`.
    pub config_key: Option<Cow<'static, str>>,
    /// Valid values for enum-like options (tab completion). Empty = any value.
    pub valid_values: Vec<Cow<'static, str>>,
}

/// Registry of all known editor options.
pub struct OptionRegistry {
    options: Vec<OptionDef>,
}

impl Default for OptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper macro to construct a static OptionDef without Cow boilerplate.
macro_rules! opt {
    ($name:expr, $aliases:expr, $doc:expr, $kind:expr, $default:expr, $key:expr, $vals:expr) => {
        OptionDef {
            name: Cow::Borrowed($name),
            aliases: {
                const A: &[&str] = $aliases;
                A.iter().map(|s| Cow::Borrowed(*s)).collect()
            },
            doc: Cow::Borrowed($doc),
            kind: $kind,
            default_value: Cow::Borrowed($default),
            config_key: $key.map(Cow::Borrowed),
            valid_values: {
                const V: &[&str] = $vals;
                V.iter().map(|s| Cow::Borrowed(*s)).collect()
            },
        }
    };
}

impl OptionRegistry {
    pub fn new() -> Self {
        OptionRegistry {
            options: vec![
                opt!("line_numbers", &["line-numbers", "show-line-numbers"],
                    "Show line numbers in the gutter",
                    OptionKind::Bool, "true", Some("editor.line_numbers"), &[]),
                opt!("relative_line_numbers", &["relative-line-numbers"],
                    "Use relative line numbering (distance from cursor)",
                    OptionKind::Bool, "false", Some("editor.relative_line_numbers"), &[]),
                opt!("word_wrap", &["word-wrap"],
                    "Soft-wrap long lines at the window edge",
                    OptionKind::Bool, "false", Some("editor.word_wrap"), &[]),
                opt!("break_indent", &["break-indent"],
                    "Indent wrapped continuation lines to match the original indentation",
                    OptionKind::Bool, "true", Some("editor.break_indent"), &[]),
                opt!("show_break", &["show-break"],
                    "Character prefix for wrapped continuation lines",
                    OptionKind::String, "↪ ", Some("editor.show_break"), &[]),
                opt!("org_hide_emphasis_markers", &["org-hide-emphasis-markers"],
                    "Hide *bold* and /italic/ markers in Org-mode",
                    OptionKind::Bool, "false", Some("editor.org_hide_emphasis_markers"), &[]),
                opt!("show_fps", &["show-fps"],
                    "Show FPS/frame-timing overlay in the status bar",
                    OptionKind::Bool, "false", Some("editor.show_fps"), &[]),
                opt!("font_size", &["font-size"],
                    "GUI font size in points (6.0–72.0). Takes effect immediately.",
                    OptionKind::Float, "14.0", Some("editor.font_size"), &[]),
                opt!("font_family", &["font-family"],
                    "Primary GUI monospace font family",
                    OptionKind::String, "", Some("editor.font_family"), &[]),
                opt!("icon_font_family", &["icon-font-family"],
                    "Secondary GUI font family for icons and symbols (fallback)",
                    OptionKind::String, "", Some("editor.icon_font_family"), &[]),
                opt!("theme", &[],
                    "Color theme name (use `:theme <name>` or `SPC t t` to cycle)",
                    OptionKind::Theme, "default", Some("editor.theme"), &[]),
                opt!("splash_art", &["splash-art"],
                    "ASCII art variant for the splash screen",
                    OptionKind::String, "bat", None, &[]),
                opt!("splash_image_width", &["splash-image-width"],
                    "Max width percentage for splash image rendering area (10\u{2013}80)",
                    OptionKind::Int, "25", None, &[]),
                opt!("splash_image_height", &["splash-image-height"],
                    "Max height percentage of viewport for splash image (5\u{2013}50)",
                    OptionKind::Int, "20", None, &[]),
                opt!("splash_show_logo", &["splash-show-logo"],
                    "Show ASCII MAE logo text below splash art/image",
                    OptionKind::Bool, "true", None, &[]),
                opt!("debug_mode", &["debug-mode"],
                    "Show RSS/CPU/frame-time in the status bar (Emacs --debug-init equivalent)",
                    OptionKind::Bool, "false", Some("editor.debug_mode"), &[]),
                opt!("clipboard", &["clipboard-mode"],
                    "Clipboard integration: unnamedplus, unnamed, or internal",
                    OptionKind::String, "unnamed", Some("editor.clipboard"),
                    &["unnamedplus", "unnamed", "internal"]),
                opt!("ai_tier", &["ai-tier"],
                    "Current AI permission tier (ReadOnly, Write, Shell, Privileged)",
                    OptionKind::String, "ReadOnly", Some("ai.auto_approve_tier"), &["ReadOnly", "Write", "Shell", "Privileged"]),
                opt!("ai_editor", &["ai-editor"],
                    "Command to launch for AI agent shell sessions (e.g. mae-agent, claude, aider)",
                    OptionKind::String, "mae-agent", Some("ai.editor"), &[]),
                opt!("ai_agent_login_shell", &["ai-agent-login-shell"],
                    "Wrap the AI agent shell command in the user's login+interactive shell \
                     (sources ~/.bashrc, ~/.zshrc, etc.) so exported env vars — auth tokens, \
                     PATH additions — reach the agent process, matching what a normal \
                     terminal gets. Disable if a slow/noisy shell rc delays agent launch.",
                    OptionKind::Bool, "true", Some("ai.agent_login_shell"), &[]),
                opt!("ai_provider", &["ai-provider"],
                    "AI API provider: claude, openai, gemini, ollama, deepseek",
                    OptionKind::String, "", Some("ai.provider"), &["claude", "openai", "gemini", "ollama", "deepseek"]),
                opt!("ai_model", &["ai-model"],
                    "AI model identifier (empty = provider default)",
                    OptionKind::String, "", Some("ai.model"), &[]),
                opt!("ai_api_key_command", &["ai-api-key-command"],
                    "Shell command whose stdout is the API key",
                    OptionKind::String, "", Some("ai.api_key_command"), &[]),
                opt!("ai_base_url", &["ai-base-url"],
                    "Base URL override for the AI API endpoint",
                    OptionKind::String, "", Some("ai.base_url"), &[]),
                opt!("ai_mode", &["ai-mode"],
                    "AI operating mode: standard, plan, or auto-accept",
                    OptionKind::String, "standard", Some("ai.mode"),
                    &["standard", "plan", "auto-accept"]),
                opt!("ai_profile", &["ai-profile"],
                    "Active AI prompt profile: pair-programmer, explorer, planner, reviewer",
                    OptionKind::String, "pair-programmer", Some("ai.profile"),
                    &["pair-programmer", "explorer", "planner", "reviewer"]),
                opt!("ai_thinking", &["ai-thinking"],
                    "Reasoning/thinking mode for supported providers: true, false, high, medium, low (empty = provider default)",
                    OptionKind::String, "", Some("ai.thinking"),
                    &["", "true", "false", "high", "medium", "low"]),
                opt!("restore_session", &["restore-session"],
                    "Automatically restore the previous session on startup (per-project)",
                    OptionKind::Bool, "false", Some("editor.restore_session"), &[]),
                opt!("insert_ctrl_d", &["insert-ctrl-d"],
                    "Insert-mode C-d behavior: dedent or delete-forward",
                    OptionKind::String, "dedent", Some("editor.insert_ctrl_d"),
                    &["dedent", "delete-forward"]),
                opt!("autosave_interval", &["autosave-interval"],
                    "Auto-save interval in seconds (0 = disabled). Saves all modified file-backed buffers.",
                    OptionKind::Int, "0", Some("editor.autosave_interval"), &[]),
                opt!("ignorecase", &[],
                    "Case-insensitive search (like vim :set ignorecase)",
                    OptionKind::Bool, "false", Some("editor.ignorecase"), &[]),
                opt!("heading_scale", &["heading-scale"],
                    "Scale heading font size in org/markdown buffers (GUI only)",
                    OptionKind::Bool, "true", Some("editor.heading_scale"), &[]),
                opt!("inline_images", &["inline-images"],
                    "Display images inline in org/markdown buffers (GUI renders image, TUI shows placeholder). Toggle with SPC t i.",
                    OptionKind::Bool, "true", Some("editor.inline_images"), &[]),
                opt!("smartcase", &[],
                    "When ignorecase is on and pattern contains uppercase, search case-sensitively",
                    OptionKind::Bool, "false", Some("editor.smartcase"), &[]),
                opt!("swap_file", &["swap-file"],
                    "Enable swap file writing for crash recovery (non-destructive, separate from autosave)",
                    OptionKind::Bool, "true", Some("editor.swap_file"), &[]),
                opt!("swap_directory", &["swap-directory"],
                    "Custom swap file directory (empty = XDG default ~/.local/share/mae/swap/)",
                    OptionKind::String, "", Some("editor.swap_directory"), &[]),
                opt!("scrollbar", &[],
                    "Show vertical scrollbar in the GUI",
                    OptionKind::Bool, "true", Some("editor.scrollbar"), &[]),
                opt!("nyan_mode", &["nyan-mode"],
                    "Show nyan cat progress indicator in the status bar",
                    OptionKind::Bool, "false", Some("editor.nyan_mode"), &[]),
                opt!("keymap_flavor", &["keymap-flavor"],
                    "Keybinding flavor: doom (default, modal) or nonmodal (CUA-style). Selects which keymap-<flavor> module loads at startup; switch live with :keymap-set-flavor.",
                    OptionKind::String, "doom", Some("editor.keymap_flavor"), &[]),
                opt!("default_mode", &["default-mode"],
                    "Editor mode at startup: normal (default, modal) or insert (non-modal). Set by the keymap flavor; applied after modules + config load.",
                    OptionKind::String, "normal", Some("editor.default_mode"), &[]),
                opt!("link_descriptive", &["link-descriptive"],
                    "Show link labels instead of raw markup (Emacs org-link-descriptive). When true, markdown links and org-style links display as styled labels.",
                    OptionKind::Bool, "true", Some("editor.link_descriptive"), &[]),
                opt!("render_markup", &["render-markup"],
                    "Apply inline styling (bold/italic/code) in conversation and KB buffers (both markdown and org syntax)",
                    OptionKind::Bool, "true", Some("editor.render_markup"), &[]),
                opt!("scrolloff", &["scroll-off", "so"],
                    "Minimum lines of context above/below cursor during scrolling",
                    OptionKind::Int, "5", Some("editor.scrolloff"), &[]),
                opt!("lsp_hover_popup", &["lsp-hover-popup"],
                    "Show hover info in a floating popup instead of the status bar",
                    OptionKind::Bool, "true", Some("editor.lsp_hover_popup"), &[]),
                opt!("lsp_diagnostics_inline", &["lsp-diagnostics-inline"],
                    "Show inline diagnostic underlines on error/warning ranges",
                    OptionKind::Bool, "true", Some("editor.lsp_diagnostics_inline"), &[]),
                opt!("lsp_diagnostics_virtual_text", &["lsp-diagnostics-virtual-text"],
                    "Show diagnostic messages as virtual text at end of line",
                    OptionKind::Bool, "true", Some("editor.lsp_diagnostics_virtual_text"), &[]),
                opt!("lsp_completion", &["lsp-completion"],
                    "Enable LSP auto-completion popup in insert mode",
                    OptionKind::Bool, "true", Some("editor.lsp_completion"), &[]),
                opt!("auto_complete", &["auto-complete", "autocomplete"],
                    "Auto-trigger LSP completion on trigger characters (e.g. `.`, `::`)",
                    OptionKind::Bool, "true", Some("editor.auto_complete"), &[]),
                opt!("show_breadcrumbs", &["show-breadcrumbs", "breadcrumbs"],
                    "Show breadcrumb bar (file > symbol ancestry) above buffer",
                    OptionKind::Bool, "false", Some("editor.show_breadcrumbs"), &[]),
                opt!("scroll_speed", &["scroll-speed"],
                    "Mouse scroll speed multiplier (lines per scroll tick)",
                    OptionKind::Int, "3", Some("editor.scroll_speed"), &[]),
                opt!("completion_max_items", &["completion-max-items"],
                    "Maximum items shown in the LSP completion popup",
                    OptionKind::Int, "10", Some("editor.completion_max_items"), &[]),
                opt!("hover_max_lines", &["hover-max-lines"],
                    "Maximum lines shown in the LSP hover popup",
                    OptionKind::Int, "15", Some("editor.hover_max_lines"), &[]),
                opt!("popup_width_pct", &["popup-width-pct"],
                    "File picker/palette popup width as percentage of screen",
                    OptionKind::Int, "70", Some("editor.popup_width_pct"), &[]),
                opt!("popup_height_pct", &["popup-height-pct"],
                    "File picker/palette popup height as percentage of screen",
                    OptionKind::Int, "60", Some("editor.popup_height_pct"), &[]),
                opt!("scrollbar_width", &["scrollbar-width"],
                    "GUI scrollbar width in pixels (1.0\u{2013}20.0)",
                    OptionKind::Float, "6.0", Some("editor.scrollbar_width"), &[]),
                opt!("file_picker_max_depth", &["file-picker-max-depth"],
                    "Maximum directory recursion depth for the file picker",
                    OptionKind::Int, "12", Some("editor.file_picker_max_depth"), &[]),
                opt!("file_picker_max_candidates", &["file-picker-max-candidates"],
                    "Maximum number of file candidates to collect",
                    OptionKind::Int, "50000", Some("editor.file_picker_max_candidates"), &[]),
                opt!("window_title", &["window-title"],
                    "Window title for the GUI",
                    OptionKind::String, "MAE \u{2014} Modern AI Editor", Some("editor.window_title"), &[]),
                opt!("heading_scale_h1", &["heading-scale-h1"],
                    "Font scale factor for level-1 headings (0.5\u{2013}3.0)",
                    OptionKind::Float, "1.5", Some("editor.heading_scale_h1"), &[]),
                opt!("heading_scale_h2", &["heading-scale-h2"],
                    "Font scale factor for level-2 headings (0.5\u{2013}3.0)",
                    OptionKind::Float, "1.3", Some("editor.heading_scale_h2"), &[]),
                opt!("mouse_autoselect_window", &["mouse-autoselect-window"],
                    "Focus follows mouse: hovering over a window selects it (Emacs mouse-autoselect-window)",
                    OptionKind::Bool, "false", Some("editor.mouse_autoselect_window"), &[]),
                opt!("mouse_wheel_follow_mouse", &["mouse-wheel-follow-mouse"],
                    "Scroll wheel targets window under mouse pointer instead of focused window (Emacs mouse-wheel-follow-mouse)",
                    OptionKind::Bool, "true", Some("editor.mouse_wheel_follow_mouse"), &[]),
                opt!("heading_scale_h3", &["heading-scale-h3"],
                    "Font scale factor for level-3 headings (0.5\u{2013}3.0)",
                    OptionKind::Float, "1.15", Some("editor.heading_scale_h3"), &[]),
                opt!("dashboard_dismiss_on_split", &["dashboard-dismiss-on-split"],
                    "Close dashboard windows when any buffer is displayed via a split. Default false (Doom parity: dashboard stays).",
                    OptionKind::Bool, "false", Some("editor.dashboard_dismiss_on_split"), &[]),
                opt!("org_agenda_files", &["org-agenda-files"],
                    "Directories/files to ingest into KB for agenda. Use :agenda-add/:agenda-remove to manage.",
                    OptionKind::String, "", Some("org.agenda_files"), &[]),
                opt!("large_file_lines", &["large-file-lines"],
                    "Line count threshold for viewport-local syntax highlighting",
                    OptionKind::Int, "5000", Some("performance.large_file_lines"), &[]),
                opt!("degrade_threshold_chars", &["degrade-threshold-chars"],
                    "Character count above which all features degrade (display regions, full markup)",
                    OptionKind::Int, "500000", Some("performance.degrade_threshold_chars"), &[]),
                opt!("degrade_threshold_line_length", &["degrade-threshold-line-length"],
                    "Maximum line length before degradation triggers",
                    OptionKind::Int, "10000", Some("performance.degrade_threshold_line_length"), &[]),
                opt!("display_region_debounce_ms", &["display-region-debounce-ms"],
                    "Milliseconds to debounce display region recomputation after edits",
                    OptionKind::Int, "150", Some("performance.display_region_debounce_ms"), &[]),
                opt!("syntax_reparse_debounce_ms", &["syntax-reparse-debounce-ms"],
                    "Milliseconds to debounce syntax reparse after edits",
                    OptionKind::Int, "50", Some("performance.syntax_reparse_debounce_ms"), &[]),
                opt!("babel_confirm", &["babel-confirm"],
                    "Prompt before executing org-babel source blocks",
                    OptionKind::Bool, "true", Some("babel.confirm"), &[]),
                opt!("babel_timeout", &["babel-timeout"],
                    "Execution timeout in seconds for babel source blocks",
                    OptionKind::Int, "30", Some("babel.timeout"), &[]),
                opt!("babel_inherit_shell_env", &["babel-inherit-shell-env"],
                    "Merge the user's resolved interactive login shell environment (sourcing \
                     .bashrc/.zshrc/.profile via `$SHELL -i -l -c env`, resolved once and \
                     cached) into babel-spawned processes and sessions. Needed because a \
                     GUI-launched editor (desktop launcher, systemd) doesn't inherit shell-rc \
                     variables the way an interactive terminal session does.",
                    OptionKind::Bool, "true", Some("babel.inherit_shell_env"), &[]),
                opt!("babel_cxx_compiler", &["babel-cxx-compiler"],
                    "C++ compiler for org-babel c++/cpp blocks (overridden per-block by :cmd, or by MAE_BABEL_CXX)",
                    OptionKind::String, "c++", Some("babel.cxx_compiler"), &[]),
                opt!("babel_c_compiler", &["babel-c-compiler"],
                    "C compiler for org-babel c blocks (overridden per-block by :cmd, or by MAE_BABEL_CC)",
                    OptionKind::String, "cc", Some("babel.c_compiler"), &[]),
                opt!("babel_cxx_std", &["babel-cxx-std"],
                    "C++ standard passed as -std=<value> for c++/cpp blocks (empty to omit)",
                    OptionKind::String, "c++17", Some("babel.cxx_std"), &[]),
                // --- Knowledge Base ---
                opt!("kb_storage_engine", &["kb-storage-engine"],
                    "Durable KB store backend. sqlite = WAL (default), lets multiple daemon-less mae processes share one store safely. sled = legacy single-writer (exclusive dir lock; a 2nd daemon-less instance can't open it). With sqlite, an existing sled store is auto-migrated ONCE on next launch — a fast bulk conversion (≈1s per few-thousand nodes); the old store is renamed to a .sled.bak (never deleted). Orthogonal to daemon_mode.",
                    OptionKind::String, "sqlite", Some("kb.storage_engine"),
                    &["sqlite", "sled"]),
                opt!("kb_watcher_enabled", &["kb-watcher-enabled"],
                    "Enable/disable file watchers for registered KB instances",
                    OptionKind::Bool, "true", Some("kb.watcher_enabled"), &[]),
                opt!("kb_watcher_debounce_ms", &["kb-watcher-debounce-ms"],
                    "Min ms between watcher drains per instance (coalesce rapid saves)",
                    OptionKind::Int, "500", Some("kb.watcher_debounce_ms"), &[]),
                opt!("kb_max_drain_events", &["kb-max-drain-events"],
                    "Max events processed per idle tick (prevents stalls on burst writes)",
                    OptionKind::Int, "100", Some("kb.max_drain_events"), &[]),
                opt!("kb_search_excerpt_length", &["kb-search-excerpt-length"],
                    "Max bytes for RAG excerpt truncation in kb_search_context",
                    OptionKind::Int, "500", Some("kb.search_excerpt_length"), &[]),
                opt!("kb_search_max_results", &["kb-search-max-results"],
                    "Hard cap for kb_search_context results",
                    OptionKind::Int, "20", Some("kb.search_max_results"), &[]),
                opt!("kb_auto_register", &["kb-auto-register"],
                    "Auto-register org directories found in project root",
                    OptionKind::Bool, "false", Some("kb.auto_register"), &[]),
                opt!("kb_notes_dir", &["kb-notes-dir"],
                    "Default directory for user-created KB notes (org-roam-directory equivalent). New notes are persisted as .org files here.",
                    OptionKind::String, "", Some("kb.notes_dir"), &[]),
                opt!("kb_activity_tracking", &["kb-activity-tracking"],
                    "Record last-accessed/modified/linked timestamps in org property drawers",
                    OptionKind::Bool, "true", Some("kb.activity_tracking"), &[]),
                opt!("kb_activity_decay", &["kb-activity-decay"],
                    "Decay rate for activity scoring (higher = faster decay)",
                    OptionKind::Float, "0.01", Some("kb.activity_decay"), &[]),
                opt!("kb_search_sort", &["kb-search-sort"],
                    "KB search result ordering: relevance (default), activity (recently accessed/modified), alphabetical, recency (session-visited first)",
                    OptionKind::String, "relevance", Some("kb.search_sort"),
                    &["relevance", "activity", "alphabetical", "recency"]),
                opt!("kb_search_scope", &["kb-search-scope"],
                    "Default KB search scope: all (primary + federated), local (primary only), remote (shared/collaborative instances only), or a specific instance name",
                    OptionKind::String, "all", Some("kb.search_scope"), &[]),
                opt!("kb_dailies_dir", &["kb-dailies-dir"],
                    "Directory for daily journal notes. Defaults to kb_notes_dir/daily if unset.",
                    OptionKind::String, "", Some("kb.dailies_dir"), &[]),
                opt!("kb_link_follow_mode", &["kb-link-follow-mode"],
                    "Following a KB-graph link (gx/Enter) outside the *KB* view: kb-view opens the rendered, federation-aware *KB* view (default); source-file jumps straight to the node's raw .org source file instead (#293)",
                    OptionKind::String, "kb-view", Some("kb.link_follow_mode"),
                    &["kb-view", "source-file"]),
                opt!("kb_daily_chain_gap_max", &["kb-daily-chain-gap-max"],
                    "Max days to walk backwards when chain-filling daily notes",
                    OptionKind::Int, "90", Some("kb.daily_chain_gap_max"), &[]),
                opt!("kb_backup_interval", &["kb-backup-interval"],
                    "RESERVED (not yet wired — no automatic-backup task exists; see issue #263). Minutes between automatic KB backups (0 to disable)",
                    OptionKind::Int, "1440", Some("kb.backup_interval"), &[]),
                opt!("kb_backup_retention", &["kb-backup-retention"],
                    "RESERVED (not yet wired — see issue #263). Number of KB backup snapshots to retain",
                    OptionKind::Int, "7", Some("kb.backup_retention"), &[]),
                opt!("format_on_save", &["format-on-save"],
                    "Run formatter before saving buffers",
                    OptionKind::Bool, "false", Some("format.on_save"), &[]),
                opt!("spell_enabled", &["spell-enabled"],
                    "Enable spell checking",
                    OptionKind::Bool, "false", Some("spell.enabled"), &[]),
                opt!("ai_chat_enabled", &["ai-chat-enabled"],
                    "Enable the legacy embedded AI chat window (SPC a p / :ai-prompt custom \
                     conversation buffer + rendering). Deprecated in favor of the model-agnostic \
                     mae-agent TUI harness (SPC a a). When false (default), SPC a p/:ai-prompt \
                     transparently launches the mae-agent shell instead of opening the old \
                     conversation buffer. See ADR-049.",
                    OptionKind::Bool, "false", Some("ai.chat_enabled"), &[]),
                // --- Which-key ---
                opt!("which_key_idle_delay", &["which-key-idle-delay"],
                    "Milliseconds of idle time (no input) after the leader keypad activates \
                     before the which-key popup appears (0 = immediate). Wired via \
                     Editor::on_idle_tick (ROADMAP #83).",
                    OptionKind::Int, "0", Some("which-key.idle-delay"), &[]),
                opt!("which_key_separator", &["which-key-separator"],
                    "Separator between key and description in which-key popup",
                    OptionKind::String, " ", Some("which-key.separator"), &[]),
                opt!("which_key_max_desc_length", &["which-key-max-desc-length"],
                    "Maximum description length in which-key popup",
                    OptionKind::Int, "40", Some("which-key.max-desc-length"), &[]),
                opt!("which_key_max_height_pct", &["which-key-max-height-pct"],
                    "Maximum which-key popup height as percentage of screen (10-90, default 40)",
                    OptionKind::Int, "40", Some("which-key.max-height-pct"), &[]),
                opt!("which_key_sort_order", &["which-key-sort-order"],
                    "Sort order for which-key entries: key (default), desc, none",
                    OptionKind::String, "key", Some("which-key.sort-order"), &["key", "desc", "none"]),
                // --- KB-link hover preview (Part D) ---
                opt!("kb_preview_idle_delay", &["kb-preview-idle-delay"],
                    "Milliseconds of cursor idle time over a KB link before a hover preview \
                     popup would appear. Wired via Editor::on_idle_tick.",
                    OptionKind::Int, "300", Some("kb-preview.idle-delay"), &[]),
                opt!("kb_preview_on_hover", &["kb-preview-on-hover"],
                    "Auto-show the KB-link hover preview popup when the cursor idles over a \
                     link in a KB-view-mode buffer (gated by kb_preview_idle_delay). The manual \
                     kb-preview command/keybinding works regardless of this option.",
                    OptionKind::Bool, "true", Some("kb-preview.on-hover"), &[]),
                opt!("kb_preview_max_lines", &["kb-preview-max-lines"],
                    "Maximum lines shown in the KB-link hover preview popup before scrolling. \
                     Mirrors hover_max_lines.",
                    OptionKind::Int, "15", Some("kb-preview.max-lines"), &[]),
                // --- Native KB graph view (Part C Phase 1) ---
                opt!("kb_graph_default_depth", &["kb-graph-default-depth"],
                    "Default hop radius (SubgraphSpec::max_depth) for (kb-graph-view-open) when \
                     no explicit depth is given.",
                    OptionKind::Int, "2", Some("kb-graph.default-depth"), &[]),
                opt!("kb_graph_include_backlinks", &["kb-graph-include-backlinks"],
                    "Whether the graph view's subgraph extraction walks backlinks as well as \
                     outgoing links.",
                    OptionKind::Bool, "true", Some("kb-graph.include-backlinks"), &[]),
                opt!("kb_graph_node_radius", &["kb-graph-node-radius"],
                    "Node circle radius in logical pixels for the graph view's GUI rendering.",
                    OptionKind::Int, "18", Some("kb-graph.node-radius"), &[]),
                opt!("kb_graph_font_size", &["kb-graph-font-size"],
                    "Node label font size in points for the graph view's GUI rendering. \
                     Independent of the base font_size option (same numeric default, no live \
                     inheritance) — MAE has no general option-inherits-from-option mechanism.",
                    OptionKind::Int, "14", Some("kb-graph.font-size"), &[]),
                opt!("kb_graph_layout_iterations", &["kb-graph-layout-iterations"],
                    "Force-directed layout iteration count run by the background \
                     graph_layout_bridge on each open/refresh/set-depth.",
                    OptionKind::Int, "50", Some("kb-graph.layout-iterations"), &[]),
                opt!("kb_graph_follow_current_node", &["kb-graph-follow-current-node"],
                    "Whether the graph view re-centers on the human/AI's current KB node \
                     automatically. Registered ahead of the Phase 2 command-post wiring that \
                     will read it — currently unused.",
                    OptionKind::Bool, "true", Some("kb-graph.follow-current-node"), &[]),
                opt!("kb_graph_animate", &["kb-graph-animate"],
                    "Whether the graph view's force-layout keeps ticking (physics animation) \
                     after the initial layout settles. Registered ahead of the Phase 3 \
                     graph_layout_bridge extension that will read it — currently unused.",
                    OptionKind::Bool, "false", Some("kb-graph.animate"), &[]),
                opt!("kb_graph_hover_enabled", &["kb-graph-hover-enabled"],
                    "Whether hovering the mouse over a graph-view node highlights it in real \
                     time (immediate, not idle-delayed — see kb_preview_idle_delay for the \
                     unrelated idle-triggered KB-link hover preview).",
                    OptionKind::Bool, "true", Some("kb-graph.hover-enabled"), &[]),
                opt!("kb_graph_view_overlay_dim_opacity", &["kb-graph-view-overlay-dim-opacity"],
                    "Opacity (0.0-1.0) of the dark scrim drawn over the rest of the editor when \
                     the graph view is toggled into full-frame overlay mode \
                     (kb-graph-view-toggle-overlay), so underlying text stays legible but \
                     visually de-emphasized while the graph is on top.",
                    OptionKind::Float, "0.6", Some("kb-graph.view-overlay-dim-opacity"), &[]),
                // --- File tree ---
                opt!("file_tree_focus_on_open", &["file-tree-focus-on-open"],
                    "Auto-focus the file tree window when it opens",
                    OptionKind::Bool, "true", Some("editor.file_tree_focus_on_open"), &[]),
                // --- Collaboration ---
                opt!("collab_server_address", &["collab-server-address"],
                    "TCP address of the mae-daemon",
                    OptionKind::String, "127.0.0.1:9473", Some("collaboration.server_address"), &[]),
                opt!("collab_auto_connect", &["collab-auto-connect"],
                    "Automatically connect to the mae-daemon on startup",
                    OptionKind::Bool, "false", Some("collaboration.auto_connect"), &[]),
                opt!("collab_auto_share", &["collab-auto-share"],
                    "Automatically share new buffers when connected to the mae-daemon",
                    OptionKind::Bool, "false", Some("collaboration.auto_share"), &[]),
                opt!("collab_reconnect_interval", &["collab-reconnect-interval"],
                    "Seconds between automatic reconnection attempts to the mae-daemon",
                    OptionKind::Int, "5", Some("collaboration.reconnect_interval_secs"), &[]),
                opt!("collab_user_name", &["collab-user-name"],
                    "Display name used to attribute collaborative edits",
                    OptionKind::String, "", Some("collaboration.user_name"), &[]),
                opt!("collab_write_timeout_ms", &["collab-write-timeout-ms"],
                    "Peer write timeout in milliseconds",
                    OptionKind::Int, "5000", Some("collaboration.write_timeout_ms"), &[]),
                opt!("collab_max_pending_updates", &["collab-max-pending-updates"],
                    "RESERVED (not yet enforced — see issue #263). Maximum pending updates queued before warning (0 = unlimited)",
                    OptionKind::Int, "1000", Some("collaboration.max_pending_updates"), &[]),
                opt!("collab_reconnect_backoff_factor", &["collab-reconnect-backoff-factor"],
                    "Exponential backoff multiplier for reconnection attempts",
                    OptionKind::Int, "2", Some("collaboration.reconnect_backoff_factor"), &[]),
                opt!("collab_max_reconnect_attempts", &["collab-max-reconnect-attempts"],
                    "Maximum reconnection attempts before giving up (0 = infinite)",
                    OptionKind::Int, "0", Some("collaboration.max_reconnect_attempts"), &[]),
                opt!("collab_batch_update_ms", &["collab-batch-update-ms"],
                    "RESERVED (not yet wired — no coalescing task exists; see issue #263). Milliseconds to batch local updates before sending (0 = immediate)",
                    OptionKind::Int, "0", Some("collaboration.batch_update_ms"), &[]),
                opt!("collab_command_queue_size", &["collab-command-queue-size"],
                    "Bounded capacity of the editor→network command channel (raise for very high edit throughput)",
                    OptionKind::Int, "256", Some("collaboration.command_queue_size"), &[]),
                opt!("collab_force_sync_debounce_secs", &["collab-force-sync-debounce-secs"],
                    "Minimum seconds between force-sync gathers for the same doc (debounce)",
                    OptionKind::Int, "2", Some("collaboration.force_sync_debounce_secs"), &[]),
                opt!("collab_daemon_start_grace_ms", &["collab-daemon-start-grace-ms"],
                    "Milliseconds to wait after spawning a local daemon before connecting to it",
                    OptionKind::Int, "500", Some("collaboration.daemon_start_grace_ms"), &[]),
                opt!("collab_host_key_prompt_timeout_secs", &["collab-host-key-prompt-timeout-secs"],
                    "Seconds to wait for a response to an unknown-daemon host-key trust prompt (key mode) before failing closed",
                    OptionKind::Int, "120", Some("collaboration.host_key_prompt_timeout_secs"), &[]),
                opt!("fill_column", &["fill-column"],
                    "Column at which fill-paragraph wraps text (Emacs fill-column)",
                    OptionKind::Int, "80", Some("editor.fill_column"), &[]),
                opt!("collab_auto_resolve_paths", &["collab-auto-resolve-paths"],
                    "When joining a doc, prompt to map to local project path if project root matches",
                    OptionKind::Bool, "false", Some("collaboration.auto_resolve_paths"), &[]),
                opt!("collab_default_save_dir", &["collab-default-save-dir"],
                    "Default directory for :saveas on joined buffers (empty = CWD)",
                    OptionKind::String, "", Some("collaboration.default_save_dir"), &[]),
                opt!("collab_save_on_remote_update", &["collab-save-on-remote-update"],
                    "Auto-save local file when CRDT update arrives (requires file_path set)",
                    OptionKind::Bool, "false", Some("collaboration.save_on_remote_update"), &[]),
                opt!("collab_heartbeat_interval", &["collab-heartbeat-interval"],
                    "Seconds between heartbeat pings to the mae-daemon (0 = disabled)",
                    OptionKind::Int, "30", Some("collaboration.heartbeat_interval_secs"), &[]),
                opt!("collab_kb_sync_mode", &["collab-kb-sync-mode"],
                    "KB sync mode: \"manual\" (explicit :kb-sync) or \"on_save\" (auto on node edit)",
                    OptionKind::String, "on_save", Some("collaboration.kb_sync_mode"), &[]),
                opt!("collab_psk", &["collab-psk"],
                    "Pre-shared key for mutual authentication with the mae-daemon (plaintext fallback)",
                    OptionKind::String, "", Some("collaboration.psk"), &[]),
                opt!("collab_psk_command", &["collab-psk-command"],
                    "Shell command to retrieve the PSK (preferred over collab_psk for security)",
                    OptionKind::String, "", Some("collaboration.psk_command"), &[]),
                opt!("collab_auth_mode", &["collab-auth-mode"],
                    "Auth for the mae-daemon: none | psk | key (key = Ed25519 trusted-peer identity over mTLS)",
                    OptionKind::String, "psk", Some("collaboration.auth_mode"),
                    &["none", "psk", "key"]),
                opt!("collab_host_key_policy", &["collab-host-key-policy"],
                    "Trust policy for an unknown daemon identity in key mode: prompt | accept-new | strict",
                    OptionKind::String, "prompt", Some("collaboration.host_key_policy"),
                    &["prompt", "accept-new", "strict"]),
                opt!("collab_tls", &["collab-tls"],
                    "Use native mTLS in key mode (recommended; false = plaintext JSON KeyAuth)",
                    OptionKind::Bool, "true", Some("collaboration.tls"), &[]),
                opt!("collab_fence_resolution", &["collab-fence-resolution"],
                    "How to resolve an epoch-fenced KB edit ('rebase required'): prompt (ask via the *Notifications* bus) or auto (adopt authoritative + re-author in the background)",
                    OptionKind::String, "prompt", Some("collaboration.fence_resolution"),
                    &["prompt", "auto"]),

                // --- Notifications / attention bus (ADR-024) ---
                opt!("notify_route_info", &["notify-route-info"],
                    "Surface for Info notifications: status | badge | modal | buffer | silent",
                    OptionKind::String, "status", None,
                    &["status", "badge", "modal", "buffer", "silent"]),
                opt!("notify_route_success", &["notify-route-success"],
                    "Surface for Success notifications: status | badge | modal | buffer | silent",
                    OptionKind::String, "status", None,
                    &["status", "badge", "modal", "buffer", "silent"]),
                opt!("notify_route_warning", &["notify-route-warning"],
                    "Surface for Warning notifications: status | badge | modal | buffer | silent",
                    OptionKind::String, "badge", None,
                    &["status", "badge", "modal", "buffer", "silent"]),
                opt!("notify_route_error", &["notify-route-error"],
                    "Surface for Error notifications: status | badge | modal | buffer | silent",
                    OptionKind::String, "badge", None,
                    &["status", "badge", "modal", "buffer", "silent"]),
                opt!("notify_route_action_required", &["notify-route-action-required"],
                    "Surface for ActionRequired notifications: status | badge | modal | buffer | silent",
                    OptionKind::String, "buffer", None,
                    &["status", "badge", "modal", "buffer", "silent"]),
                opt!("notify_badge_min_severity", &["notify-badge-min-severity"],
                    "Minimum severity counted in the mode-line attention badge",
                    OptionKind::String, "warning", None,
                    &["info", "success", "warning", "error", "action-required"]),

                // --- Daemon ---
                opt!("daemon_mode", &["daemon-mode"],
                    "Editor↔daemon relationship (ADR-035): off = in-process embedded KB only (the floor, no daemon); on-demand = attach to a running daemon or auto-spawn a co-located one (persistence/collab without ceremony); shared = attach to an existing OS-supervised/remote daemon, never spawn (multi-session + P2P). Supersedes daemon_enabled.",
                    OptionKind::String, "off", Some("daemon.mode"),
                    &["off", "on-demand", "shared"]),
                opt!("daemon_enabled", &["daemon-enabled"],
                    "DEPRECATED alias for daemon_mode: true ⇒ on-demand, false ⇒ off. Prefer daemon_mode.",
                    OptionKind::Bool, "false", Some("daemon.enabled"), &[]),
                opt!("daemon_socket", &["daemon-socket"],
                    "Unix socket path for daemon communication (empty = auto: $XDG_RUNTIME_DIR/mae-daemon.sock, matching the daemon's bind path)",
                    OptionKind::String, "", Some("daemon.socket"), &[]),
                opt!("daemon_cache_size", &["daemon-cache-size"],
                    "Maximum nodes in LRU cache (0 = unbounded)",
                    OptionKind::Int, "200", Some("daemon.cache_size"), &[]),
                opt!("daemon_default", &["daemon-default"],
                    "EXPERIMENTAL (Phase D, ADR-029): when a local daemon hosts the primary KB, host + thin-start it on the daemon (CRDT source of truth) instead of the editor's on-disk store. Default off. Known gaps until the thin-client ADR: agenda + ranked KB search are not yet daemon-routed and read empty under a thin mirror.",
                    OptionKind::Bool, "false", Some("daemon.host_primary"), &[]),
            ],
        }
    }

    /// Find an option by canonical name or alias.
    pub fn find(&self, name: &str) -> Option<&OptionDef> {
        self.options
            .iter()
            .find(|o| o.name.as_ref() == name || o.aliases.iter().any(|a| a.as_ref() == name))
    }

    /// List all registered options.
    pub fn list(&self) -> &[OptionDef] {
        &self.options
    }

    /// Return all canonical option names (for tab completion).
    pub fn all_names(&self) -> Vec<String> {
        self.options.iter().map(|o| o.name.to_string()).collect()
    }

    /// Check if an option exists by canonical name or alias.
    pub fn has_option(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Register an option at runtime (from modules or Scheme `define-option!`).
    /// Logs a warning if overwriting an existing option.
    pub fn register_dynamic(
        &mut self,
        name: String,
        aliases: Vec<String>,
        doc: String,
        kind: OptionKind,
        default_value: String,
        config_key: Option<String>,
    ) {
        if let Some(existing) = self.find(&name) {
            eprintln!(
                "[warn] Option '{}' already registered (overwriting, was: {})",
                name, existing.doc
            );
            self.options.retain(|o| o.name.as_ref() != name);
        }
        self.options.push(OptionDef {
            name: Cow::Owned(name),
            aliases: aliases.into_iter().map(Cow::Owned).collect(),
            doc: Cow::Owned(doc),
            kind,
            default_value: Cow::Owned(default_value),
            config_key: config_key.map(Cow::Owned),
            valid_values: vec![],
        });
    }

    /// Unregister an option by canonical name (for module unload).
    pub fn unregister(&mut self, name: &str) -> bool {
        let before = self.options.len();
        self.options.retain(|o| o.name.as_ref() != name);
        self.options.len() < before
    }
}

/// Parse a string as an integer value.
pub fn parse_option_int(s: &str) -> Result<i64, String> {
    s.parse().map_err(|_| format!("Invalid integer: '{}'", s))
}

/// Parse a string as a boolean value. Accepts: true, #t, 1, yes, on → true; everything else → false.
pub fn parse_option_bool(s: &str) -> Result<bool, String> {
    match s {
        "true" | "#t" | "1" | "yes" | "on" => Ok(true),
        "false" | "#f" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("Invalid boolean: '{}' (expected true/false)", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_finds_by_canonical_name() {
        let reg = OptionRegistry::new();
        assert!(reg.find("line_numbers").is_some());
        assert!(reg.find("word_wrap").is_some());
        assert!(reg.find("theme").is_some());
    }

    #[test]
    fn registry_finds_by_alias() {
        let reg = OptionRegistry::new();
        let opt = reg.find("line-numbers").unwrap();
        assert_eq!(opt.name, "line_numbers");

        let opt = reg.find("show-line-numbers").unwrap();
        assert_eq!(opt.name, "line_numbers");
    }

    #[test]
    fn registry_returns_none_for_unknown() {
        let reg = OptionRegistry::new();
        assert!(reg.find("nonexistent").is_none());
    }

    #[test]
    fn registry_lists_all_options() {
        let reg = OptionRegistry::new();
        assert!(reg.list().len() >= 8);
    }

    #[test]
    fn lsp_options_registered() {
        let reg = OptionRegistry::new();
        assert!(reg.find("lsp_hover_popup").is_some());
        assert!(reg.find("lsp-hover-popup").is_some());
        assert!(reg.find("lsp_diagnostics_inline").is_some());
        assert!(reg.find("lsp_diagnostics_virtual_text").is_some());
        assert!(reg.find("lsp_completion").is_some());
        assert!(reg.find("lsp-completion").is_some());
    }

    #[test]
    fn lsp_options_defaults() {
        let reg = OptionRegistry::new();
        assert_eq!(reg.find("lsp_hover_popup").unwrap().default_value, "true");
        assert_eq!(
            reg.find("lsp_diagnostics_inline").unwrap().default_value,
            "true"
        );
        assert_eq!(
            reg.find("lsp_diagnostics_virtual_text")
                .unwrap()
                .default_value,
            "true"
        );
        assert_eq!(reg.find("lsp_completion").unwrap().default_value, "true");
    }

    #[test]
    fn parse_bool_variants() {
        assert_eq!(parse_option_bool("true"), Ok(true));
        assert_eq!(parse_option_bool("#t"), Ok(true));
        assert_eq!(parse_option_bool("1"), Ok(true));
        assert_eq!(parse_option_bool("false"), Ok(false));
        assert_eq!(parse_option_bool("0"), Ok(false));
        assert!(parse_option_bool("maybe").is_err());
    }

    #[test]
    fn scrolloff_option_registered() {
        let reg = OptionRegistry::new();
        let opt = reg.find("scrolloff").unwrap();
        assert_eq!(opt.name, "scrolloff");
        assert_eq!(opt.default_value, "5");
        assert_eq!(opt.config_key.as_deref(), Some("editor.scrolloff"));
        // Alias lookup
        assert!(reg.find("so").is_some());
        assert_eq!(reg.find("so").unwrap().name, "scrolloff");
        assert!(reg.find("scroll-off").is_some());
    }

    #[test]
    fn int_option_kind_display() {
        assert_eq!(format!("{}", OptionKind::Int), "integer");
    }

    #[test]
    fn parse_option_int_valid() {
        assert_eq!(parse_option_int("42"), Ok(42));
        assert_eq!(parse_option_int("-1"), Ok(-1));
        assert!(parse_option_int("abc").is_err());
    }

    #[test]
    fn new_options_registered() {
        let reg = OptionRegistry::new();
        assert!(reg.find("scroll_speed").is_some());
        assert!(reg.find("completion_max_items").is_some());
        assert!(reg.find("hover_max_lines").is_some());
        assert!(reg.find("popup_width_pct").is_some());
        assert!(reg.find("popup_height_pct").is_some());
        assert!(reg.find("scrollbar_width").is_some());
        assert!(reg.find("file_picker_max_depth").is_some());
        assert!(reg.find("file_picker_max_candidates").is_some());
        assert!(reg.find("window_title").is_some());
        assert!(reg.find("heading_scale_h1").is_some());
        assert!(reg.find("heading_scale_h2").is_some());
        assert!(reg.find("heading_scale_h3").is_some());
    }

    #[test]
    fn new_options_aliases() {
        let reg = OptionRegistry::new();
        assert_eq!(reg.find("scroll-speed").unwrap().name, "scroll_speed");
        assert_eq!(
            reg.find("heading-scale-h1").unwrap().name,
            "heading_scale_h1"
        );
    }

    #[test]
    fn no_duplicate_canonical_option_names() {
        // CLAUDE.md #7: the OptionRegistry is the single source of truth for
        // user-visible config. A duplicate canonical name means `find()` silently
        // shadows one definition (last-write-wins on `:set-save` persistence).
        let reg = OptionRegistry::new();
        let mut seen = std::collections::HashSet::new();
        for opt in reg.list() {
            assert!(
                seen.insert(opt.name.as_ref()),
                "duplicate canonical option name: {}",
                opt.name
            );
        }
    }

    #[test]
    fn snake_case_options_expose_kebab_case_alias() {
        // Invariant guard: every snake_case canonical name (`collab_x`, `foo_bar`)
        // must also resolve under its kebab-case spelling (`collab-x`, `foo-bar`),
        // because Scheme's `(set-option! …)` and the `:set` command accept the
        // hyphenated form. A new `opt!` that forgets the alias regresses the
        // config surface uniformly across the editor; this catches it at compile-
        // adjacent test time rather than in a live session. (Verifies the whole
        // registry, with the security/connect-critical `collab_*` keys as the
        // motivating case — see B2 in the collab test-gap plan.)
        let reg = OptionRegistry::new();
        for opt in reg.list() {
            if !opt.name.contains('_') {
                continue;
            }
            let kebab = opt.name.replace('_', "-");
            let found = reg.find(&kebab);
            assert!(
                found.is_some(),
                "option `{}` has no kebab-case alias `{}`",
                opt.name,
                kebab
            );
            assert_eq!(
                found.unwrap().name,
                opt.name,
                "kebab alias `{}` resolves to `{}`, not its own canonical `{}`",
                kebab,
                found.unwrap().name,
                opt.name
            );
        }
    }

    #[test]
    fn every_collab_option_has_kebab_alias_and_config_key() {
        // Connect-critical + security-relevant collab keys specifically: each must
        // carry both its kebab alias (Scheme/`:set` access) and a `config_key`
        // (so `:set-save` persists it to init.scm). Without the config_key a
        // collab setting can be set live but silently fails to survive a restart.
        let reg = OptionRegistry::new();
        let collab: Vec<_> = reg
            .list()
            .iter()
            .filter(|o| o.name.starts_with("collab_"))
            .collect();
        assert!(
            collab.len() >= 10,
            "expected the full collab option set, found {}",
            collab.len()
        );
        for opt in collab {
            let kebab = opt.name.replace('_', "-");
            assert_eq!(
                reg.find(&kebab).map(|o| o.name.as_ref()),
                Some(opt.name.as_ref()),
                "collab option `{}` missing kebab alias `{}`",
                opt.name,
                kebab
            );
            assert!(
                opt.config_key.is_some(),
                "collab option `{}` has no config_key (not :set-save persistable)",
                opt.name
            );
        }
    }
}
