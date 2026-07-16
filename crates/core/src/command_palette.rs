//! Fuzzy command palette overlay (SPC SPC).
//!
//! Mirrors `FilePicker` in shape so the renderer and key handler can
//! reuse the same mental model: `query` drives `filtered` via
//! `update_filter`, Up/Down move `selected`, Enter executes the
//! selection. The source is `CommandRegistry`, which means every
//! registered command — including `help`, `describe-key`, and
//! `describe-command` — is automatically searchable.

use crate::commands::CommandRegistry;
use crate::file_picker::score_match;

/// One entry in the palette: command name plus its one-line doc.
#[derive(Debug, Clone)]
pub struct PaletteEntry {
    pub name: String,
    pub doc: String,
    /// Extra searchable text (e.g. KB node body). Not displayed, only matched.
    pub searchable_extra: Option<String>,
}

/// What to do with the selected entry when the user presses Enter.
///
/// The palette UI is the same in all cases — the only difference is
/// what the key handler does on `Enter`. `Execute` runs the command
/// (SPC SPC), `Describe` opens the `cmd:<name>` help node (SPC h c),
/// `SetTheme` applies the selected theme name (SPC t s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PalettePurpose {
    Execute,
    Describe,
    SetTheme,
    KbSearch,
    SwitchBuffer,
    SetSplashArt,
    RecentFile,
    SwitchProject,
    AiMode,
    AiProfile,
    GitBranch,
    ForgetProject,
    KbFindOrCreate,
    KbInsertLink,
    MiniDialog,
    CollabJoin,
    SetupAiProvider,
    SetupCollabMode,
    SetKeymapFlavor,
    SetKbSearchScope,
}

impl PalettePurpose {
    /// Human-readable label for this palette kind, used in popup titles.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Execute => "Commands",
            Self::Describe => "Describe Command",
            Self::SetTheme => "Themes",
            Self::KbSearch => "MAE Help",
            Self::SwitchBuffer => "Buffers",
            Self::SetSplashArt => "Splash Art",
            Self::RecentFile => "Recent Files",
            Self::SwitchProject => "Projects",
            Self::AiMode => "AI Operating Mode",
            Self::AiProfile => "AI Prompt Profile",
            Self::GitBranch => "Git Branch",
            Self::ForgetProject => "Forget Project",
            Self::KbFindOrCreate => "Find or Create",
            Self::KbInsertLink => "Insert Link",
            Self::MiniDialog => "Dialog",
            Self::CollabJoin => "Join Document",
            Self::SetupAiProvider => "AI Provider",
            Self::SetupCollabMode => "Collaboration Mode",
            Self::SetKeymapFlavor => "Choose Keybindings",
            Self::SetKbSearchScope => "KB Search Scope",
        }
    }
}

// ---------------------------------------------------------------------------
// MiniDialog — reusable multi-field interactive dialog
// ---------------------------------------------------------------------------

/// What interactive command opened this mini-dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiniDialogKind {
    EditLink,
    /// y/n confirmation (delete file, kill unsaved buffer, etc.)
    Confirm,
    /// One text field (rename, create file, save-as, tags, etc.)
    SingleInput,
}

/// One editable field in a mini-dialog.
#[derive(Debug, Clone)]
pub struct MiniDialogField {
    /// Prompt label (e.g. "URL", "Label").
    pub label: String,
    /// Current text value.
    pub value: String,
    /// Hint text when value is empty.
    pub placeholder: String,
}

/// Context needed to apply the dialog result back to the buffer.
#[derive(Debug, Clone)]
pub enum MiniDialogContext {
    LinkEdit {
        buf_idx: usize,
        byte_start: usize,
        byte_end: usize,
        is_org: bool,
    },
    FileDelete {
        path: std::path::PathBuf,
        close_buffer: bool,
    },
    FileRename {
        old_path: std::path::PathBuf,
    },
    FileCopy {
        src_path: std::path::PathBuf,
    },
    FileSaveAs,
    FileTreeRename {
        path: std::path::PathBuf,
    },
    FileTreeCreate {
        parent: std::path::PathBuf,
    },
    OrgSetTags {
        heading_line: usize,
    },
    AgendaFilterTag,
    RevertBuffer {
        buf_idx: usize,
    },
    DailyGotoDate,
    CollabResolvePath {
        buf_idx: usize,
        resolved_path: std::path::PathBuf,
    },
    SetupAiModel {
        provider: String,
    },
    SetupAiKeyCommand {
        provider: String,
        model: String,
    },
    SetupCollabAddress,
    SetupCollabPsk {
        address: String,
    },
    SetupKbNotesDir,
    /// Confirm-only, no extra data needed: `kb_search_scope` is already
    /// applied by the time this resolves (the picker sets it before opening
    /// this prompt) — on confirm just opens the graph view with no explicit
    /// center, so `resolve_graph_center`/`kb_owner_of_scoped` (Phase 5) pick
    /// the newly-scoped instance's own default node. Closes the gap between
    /// `:kb-set-search-scope` (switch which KB you're working in) and
    /// actually seeing that KB's graph — previously two unconnected steps.
    KbGraphOpenPrompt,
    /// ADR-024: a `BlockingReply` notification routed to a modal — the y/N answer
    /// is sent on the notification's reply channel (`pending_notif_reply`). The
    /// generalized successor to the bespoke TOFU `PeerKeyAccept` prompt (ADR-017):
    /// the host-key trust prompt is now just one consumer of this mechanism.
    Notification {
        notif_id: u64,
    },
    /// #269: a babel source block whose `:eval` policy resolved to
    /// `NeedsConfirmation` (interactive path only — the AI/MCP path refuses
    /// outright instead of opening a dialog, since there's no human to
    /// answer it). Execution is deferred until the user confirms; `block`
    /// is a snapshot from when the dialog opened, re-executed against the
    /// buffer's current content on confirm (`Editor::babel_run_block`).
    BabelConfirm {
        buf_idx: usize,
        block: Box<crate::babel::SrcBlock>,
    },
}

/// State for a multi-field mini-dialog (edit-link, rename, etc.)
#[derive(Debug, Clone)]
pub struct MiniDialogState {
    /// What interactive command opened this dialog.
    pub kind: MiniDialogKind,
    /// The field prompts and current values.
    pub fields: Vec<MiniDialogField>,
    /// Which field is currently being edited (0-indexed).
    pub active_field: usize,
    /// Context needed to apply the result.
    pub context: MiniDialogContext,
}

impl MiniDialogState {
    /// Title for the dialog window.
    pub fn title(&self) -> &'static str {
        match self.kind {
            MiniDialogKind::EditLink => "Edit Link",
            MiniDialogKind::Confirm => match &self.context {
                MiniDialogContext::Notification { .. } => "Action Required",
                _ => "Confirm",
            },
            MiniDialogKind::SingleInput => match &self.context {
                MiniDialogContext::FileRename { .. } | MiniDialogContext::FileTreeRename { .. } => {
                    "Rename"
                }
                MiniDialogContext::FileCopy { .. } => "Copy File",
                MiniDialogContext::FileSaveAs => "Save As",
                MiniDialogContext::FileTreeCreate { .. } => "Create",
                MiniDialogContext::OrgSetTags { .. } => "Set Tags",
                MiniDialogContext::AgendaFilterTag => "Filter by Tag",
                MiniDialogContext::SetupAiModel { .. } => "AI Model",
                MiniDialogContext::SetupAiKeyCommand { .. } => "API Key Command",
                MiniDialogContext::SetupCollabAddress => "Server Address",
                MiniDialogContext::SetupCollabPsk { .. } => "PSK Command",
                MiniDialogContext::SetupKbNotesDir => "Notes Directory",
                _ => "Input",
            },
        }
    }

    /// Whether this dialog is a simple confirmation (no text input).
    pub fn is_confirm(&self) -> bool {
        self.kind == MiniDialogKind::Confirm
    }

    /// Create a confirmation dialog (yes/no).
    pub fn confirm(question: impl Into<String>, context: MiniDialogContext) -> Self {
        Self {
            kind: MiniDialogKind::Confirm,
            fields: vec![MiniDialogField {
                label: question.into(),
                value: String::new(),
                placeholder: String::new(),
            }],
            active_field: 0,
            context,
        }
    }

    /// Create a single-input dialog with an optional pre-filled value.
    pub fn single_input(
        label: impl Into<String>,
        value: impl Into<String>,
        placeholder: impl Into<String>,
        context: MiniDialogContext,
    ) -> Self {
        Self {
            kind: MiniDialogKind::SingleInput,
            fields: vec![MiniDialogField {
                label: label.into(),
                value: value.into(),
                placeholder: placeholder.into(),
            }],
            active_field: 0,
            context,
        }
    }
}

/// State for the command palette overlay.
pub struct CommandPalette {
    /// User's query string.
    pub query: String,
    /// All registered commands, sorted alphabetically.
    pub entries: Vec<PaletteEntry>,
    /// Indices into `entries` matching the current query, ranked by score.
    pub filtered: Vec<usize>,
    /// Currently selected index within `filtered`.
    pub selected: usize,
    /// What to do with the selection on Enter.
    pub purpose: PalettePurpose,
    /// When true, the query input line is selected (not any list item).
    /// Used by KbFindOrCreate/KbInsertLink to offer a "create from query" action.
    pub query_selected: bool,
}

impl CommandPalette {
    /// Snapshot the registry into an execute-purpose palette. Commands
    /// are sorted by name up front so the "empty query" view is
    /// predictable.
    pub fn from_registry(reg: &CommandRegistry) -> Self {
        Self::with_purpose(reg, PalettePurpose::Execute)
    }

    /// Same fuzzy-search overlay but Enter opens the help node for the
    /// selected command instead of executing it. Used by `SPC h c` /
    /// `describe-command`.
    pub fn for_describe(reg: &CommandRegistry) -> Self {
        Self::with_purpose(reg, PalettePurpose::Describe)
    }

    /// Help search palette: entries are KB node ids + titles, Enter opens
    /// the selected node in the KB buffer. Used by `SPC h s`.
    pub fn for_help_search(nodes: &[(String, String)]) -> Self {
        let mut entries: Vec<PaletteEntry> = nodes
            .iter()
            .map(|(id, title)| PaletteEntry {
                name: id.clone(),
                doc: title.clone(),
                searchable_extra: None,
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::KbSearch,
            query_selected: false,
        }
    }

    /// Theme picker palette. Used by `SPC t s` / `set-theme`.
    pub fn for_themes(names: &[&str]) -> Self {
        Self::with_name_list(names, PalettePurpose::SetTheme)
    }

    /// Buffer picker palette. Used by `SPC b b` / `switch-buffer`.
    pub fn for_buffers(names: &[&str]) -> Self {
        Self::with_name_list(names, PalettePurpose::SwitchBuffer)
    }

    /// Recent file picker palette. Used by `SPC f r` / `SPC p r`.
    pub fn for_recent_files(names: &[&str]) -> Self {
        Self::with_name_list(names, PalettePurpose::RecentFile)
    }

    /// Project switch palette. Used by `SPC p p` / `project-switch`.
    pub fn for_project_switch(roots: &[&str]) -> Self {
        Self::with_name_list(roots, PalettePurpose::SwitchProject)
    }

    /// AI mode picker palette. Used by `:ai-set-mode`.
    pub fn for_ai_mode(modes: &[&str]) -> Self {
        Self::with_name_list(modes, PalettePurpose::AiMode)
    }

    /// AI profile picker palette. Used by `:ai-set-profile`.
    pub fn for_ai_profile(profiles: &[&str]) -> Self {
        Self::with_name_list(profiles, PalettePurpose::AiProfile)
    }

    /// Git branch picker palette. Used by `b b` in git-status.
    pub fn for_git_branch(branches: &[&str]) -> Self {
        Self::with_name_list(branches, PalettePurpose::GitBranch)
    }

    /// Forget project palette. Used by `SPC p d` / `project-forget`.
    pub fn for_forget_project(roots: &[&str]) -> Self {
        Self::with_name_list(roots, PalettePurpose::ForgetProject)
    }

    /// KB find-or-create palette: pre-populated with all KB nodes.
    /// Typing filters; Enter on a match opens it, Enter with no match creates.
    /// Used by `SPC n c` / `SPC n f`.
    /// Accepts `(id, title, body)` triples — body is stored in `searchable_extra`
    /// (truncated to 500 chars) so fuzzy search matches body content.
    /// The caller is responsible for sorting (alphabetical, activity, etc.).
    pub fn for_kb_find_or_create(nodes: &[(String, String, String)]) -> Self {
        let mut palette = CommandPalette {
            query: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            purpose: PalettePurpose::KbFindOrCreate,
            query_selected: false,
        };
        palette.set_kb_find_entries(nodes);
        palette
    }

    /// Replace the kb-find entries from `(id, title, body)` triples, keeping the
    /// current `query`. Used both for initial population and for the lazy
    /// re-search refresh on large KBs (where the server already ranked the
    /// window, so `filtered` is the full set in order — no client re-filter).
    pub fn set_kb_find_entries(&mut self, nodes: &[(String, String, String)]) {
        self.entries = nodes
            .iter()
            .map(|(id, title, body)| PaletteEntry {
                name: id.clone(),
                doc: title.clone(),
                searchable_extra: if body.is_empty() {
                    None
                } else {
                    // Truncate to 500 chars to avoid a 73KB outlier dominating memory.
                    Some(body.chars().take(500).collect())
                },
            })
            .collect();
        self.filtered = (0..self.entries.len()).collect();
        self.selected = 0;
        if self.has_create_from_query() {
            self.query_selected = !self.query.is_empty() && self.filtered.is_empty();
        }
    }

    /// KB insert link palette: populated with all KB node ids + titles.
    /// Used by `SPC n i` / `kb-insert-link`.
    pub fn for_kb_insert_link(nodes: &[(String, String)]) -> Self {
        let entries: Vec<PaletteEntry> = nodes
            .iter()
            .map(|(id, title)| PaletteEntry {
                name: id.clone(),
                doc: title.clone(),
                searchable_extra: None,
            })
            .collect();
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::KbInsertLink,
            query_selected: false,
        }
    }

    /// Splash art picker palette. Lists built-in + custom registered arts.
    pub fn for_splash_art(editor: &crate::Editor) -> Self {
        let names = crate::render_common::splash::available_splash_names(editor);
        let entries: Vec<PaletteEntry> = names
            .into_iter()
            .map(|(name, kind)| PaletteEntry {
                name,
                doc: kind,
                searchable_extra: None,
            })
            .collect();
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::SetSplashArt,
            query_selected: false,
        }
    }

    /// Collab join palette: server documents to join. Used by `SPC C j`.
    pub fn for_collab_join(names: &[&str]) -> Self {
        Self::with_name_list(names, PalettePurpose::CollabJoin)
    }

    /// Setup: AI provider selection. Used by `:setup-ai`.
    pub fn for_setup_ai_provider() -> Self {
        Self::with_name_list(
            &["claude", "openai", "gemini", "ollama", "deepseek", "skip"],
            PalettePurpose::SetupAiProvider,
        )
    }

    /// Setup: collaboration mode selection. Used by `:setup-collab`.
    /// Guided picker for the collaboration mode (`:setup-collab`). Each entry explains the
    /// tier (ADR-035 `daemon_mode`) so a newcomer can choose with context; the `name` is the
    /// token the setup dispatch consumes, so those stay `solo|loopback|network|skip`.
    pub fn for_setup_collab_mode() -> Self {
        let entries = vec![
            PaletteEntry {
                name: "solo".to_string(),
                doc: "No daemon — fully local. Edits are still CRDT (full undo/redo, offline). Zero config; instant upgrade to loopback/network later.".to_string(),
                searchable_extra: Some("local offline default none no daemon".to_string()),
            },
            PaletteEntry {
                name: "loopback".to_string(),
                doc: "Local daemon (127.0.0.1:9473) — coordinate several MAE instances / AI agents on THIS machine. Persistence + multi-client on one box.".to_string(),
                searchable_extra: Some("localhost multi-agent same machine on-demand".to_string()),
            },
            PaletteEntry {
                name: "network".to_string(),
                doc: "Shared daemon over the network — multi-user collaboration + KB sharing. Key-mode auth, E2E encryption, identity rotation/recovery, and the P2P mesh live here.".to_string(),
                searchable_extra: Some("multi-user lan server shared remote e2e encryption mesh p2p".to_string()),
            },
            PaletteEntry {
                name: "skip".to_string(),
                doc: "Don't configure collaboration now — leave settings unchanged.".to_string(),
                searchable_extra: Some("cancel later none".to_string()),
            },
        ];
        let filtered = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::SetupCollabMode,
            query_selected: false,
        }
    }

    /// Choose-keybindings picker (dashboard quick-action / `:choose-keymap-flavor`).
    /// Each entry explains the flavor so a newcomer can pick with context; the
    /// selection dispatches `keymap-set-flavor <name>` (live switch).
    pub fn for_keymap_flavor() -> Self {
        let entries = vec![
            PaletteEntry {
                name: "doom".to_string(),
                doc: "Modal (vim/evil): edit in Normal mode, SPC opens the command menu. For vim/Emacs users.".to_string(),
                searchable_extra: Some("modal vim evil normal".to_string()),
            },
            PaletteEntry {
                name: "nonmodal".to_string(),
                doc: "Non-modal (CUA): type normally like VSCode/TextEdit, C-; opens the command menu. For most newcomers.".to_string(),
                searchable_extra: Some("cua vscode textedit insert beginner".to_string()),
            },
        ];
        let filtered = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::SetKeymapFlavor,
            query_selected: false,
        }
    }

    /// Guided picker for the default KB search scope (`kb_search_scope`).
    /// Always offers the three keyword scopes, then one entry per registered
    /// federated instance (so the user can scope searches to a single KB).
    /// Selection dispatches a `set-option!` on `kb_search_scope`.
    pub fn for_kb_search_scope(instances: &[&str]) -> Self {
        let mut entries = vec![
            PaletteEntry {
                name: "all".to_string(),
                doc: "Search the primary KB plus every federated instance (default).".to_string(),
                searchable_extra: Some("everything federated".to_string()),
            },
            PaletteEntry {
                name: "local".to_string(),
                doc: "Search only the primary (local) KB.".to_string(),
                searchable_extra: Some("primary only".to_string()),
            },
            PaletteEntry {
                name: "remote".to_string(),
                doc: "Search only shared/collaborative instances.".to_string(),
                searchable_extra: Some("shared collaborative federated".to_string()),
            },
        ];
        for inst in instances {
            entries.push(PaletteEntry {
                name: (*inst).to_string(),
                doc: format!("Search only the '{inst}' instance."),
                searchable_extra: Some("instance kb".to_string()),
            });
        }
        let filtered = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::SetKbSearchScope,
            query_selected: false,
        }
    }

    /// Build a palette from a simple name list with the given purpose.
    pub fn with_name_list(names: &[&str], purpose: PalettePurpose) -> Self {
        let entries: Vec<PaletteEntry> = names
            .iter()
            .map(|n| PaletteEntry {
                name: n.to_string(),
                doc: String::new(),
                searchable_extra: None,
            })
            .collect();
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose,
            query_selected: false,
        }
    }

    fn with_purpose(reg: &CommandRegistry, purpose: PalettePurpose) -> Self {
        let mut entries: Vec<PaletteEntry> = reg
            .list_commands()
            .into_iter()
            .map(|c| PaletteEntry {
                name: c.name.clone(),
                doc: c.doc.clone(),
                searchable_extra: None,
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose,
            query_selected: false,
        }
    }

    /// Re-score and re-rank entries against the current query.
    /// Matches against both `name` and `doc` fields, taking the better score.
    pub fn update_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.entries.len()).collect();
        } else {
            let q: Vec<char> = self.query.to_lowercase().chars().collect();
            let mut scored: Vec<(usize, i64)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, e)| {
                    let name_score = score_match(&e.name, &q);
                    let doc_score = if e.doc.is_empty() {
                        None
                    } else {
                        score_match(&e.doc, &q)
                    };
                    let extra_score = e.searchable_extra.as_ref().and_then(|s| score_match(s, &q));
                    name_score.max(doc_score).max(extra_score).map(|s| (idx, s))
                })
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.1));
            self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
        }
        self.selected = 0;
        // For find-or-create palettes, auto-select query line when no matches.
        if self.has_create_from_query() {
            self.query_selected = !self.query.is_empty() && self.filtered.is_empty();
        }
    }

    pub fn move_down(&mut self) {
        if self.query_selected {
            self.query_selected = false;
            self.selected = 0;
            return;
        }
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn move_up(&mut self) {
        if self.query_selected {
            return; // already at top
        }
        if self.has_create_from_query()
            && !self.query.is_empty()
            && (self.filtered.is_empty() || self.selected == 0)
        {
            self.query_selected = true;
            return;
        }
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Name of the currently selected command, if any.
    /// Returns `None` when `query_selected` is true (user is on the "create" line).
    pub fn selected_name(&self) -> Option<&str> {
        if self.query_selected {
            return None;
        }
        if self.filtered.is_empty() {
            return None;
        }
        let idx = self.filtered[self.selected];
        Some(&self.entries[idx].name)
    }

    /// Whether this palette supports creating from the query text.
    pub fn has_create_from_query(&self) -> bool {
        matches!(
            self.purpose,
            PalettePurpose::KbFindOrCreate | PalettePurpose::KbInsertLink
        )
    }

    /// The entry at position `pos` in the filtered list.
    pub fn entry_at(&self, pos: usize) -> Option<&PaletteEntry> {
        let &idx = self.filtered.get(pos)?;
        self.entries.get(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_purpose_labels_non_empty() {
        let purposes = [
            PalettePurpose::Execute,
            PalettePurpose::Describe,
            PalettePurpose::SetTheme,
            PalettePurpose::KbSearch,
            PalettePurpose::SwitchBuffer,
            PalettePurpose::SetSplashArt,
            PalettePurpose::RecentFile,
            PalettePurpose::SwitchProject,
            PalettePurpose::AiMode,
            PalettePurpose::AiProfile,
            PalettePurpose::GitBranch,
            PalettePurpose::ForgetProject,
            PalettePurpose::KbFindOrCreate,
            PalettePurpose::KbInsertLink,
            PalettePurpose::MiniDialog,
            PalettePurpose::CollabJoin,
            PalettePurpose::SetupAiProvider,
            PalettePurpose::SetupCollabMode,
            PalettePurpose::SetKeymapFlavor,
            PalettePurpose::SetKbSearchScope,
        ];
        for p in &purposes {
            assert!(!p.label().is_empty(), "{:?} has empty label", p);
        }
        assert_eq!(PalettePurpose::Execute.label(), "Commands");
        assert_eq!(PalettePurpose::GitBranch.label(), "Git Branch");
        assert_eq!(PalettePurpose::ForgetProject.label(), "Forget Project");
    }

    #[test]
    fn kb_search_scope_picker_lists_keywords_and_instances() {
        let palette = CommandPalette::for_kb_search_scope(&["Work", "Research"]);
        assert_eq!(palette.purpose, PalettePurpose::SetKbSearchScope);
        let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
        // Three keyword scopes first, then each registered instance.
        assert_eq!(names, vec!["all", "local", "remote", "Work", "Research"]);
        // Every entry is searchable in the empty-query filter.
        assert_eq!(palette.filtered.len(), 5);
    }

    #[test]
    fn from_registry_sorts_alphabetically() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("quit", "Quit editor");
        reg.register_builtin("alpha", "First");
        reg.register_builtin("middle", "Middle");
        let palette = CommandPalette::from_registry(&reg);
        let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "middle", "quit"]);
    }

    #[test]
    fn empty_query_returns_all() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("a", "A");
        reg.register_builtin("b", "B");
        let palette = CommandPalette::from_registry(&reg);
        assert_eq!(palette.filtered.len(), 2);
    }

    #[test]
    fn filter_subsequence_match() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("switch-buffer", "Switch buffer");
        reg.register_builtin("save-and-quit", "Save and quit");
        reg.register_builtin("quit", "Quit");
        let mut palette = CommandPalette::from_registry(&reg);
        palette.query = "sb".into();
        palette.update_filter();
        let names: Vec<&str> = palette
            .filtered
            .iter()
            .map(|&i| palette.entries[i].name.as_str())
            .collect();
        assert!(
            names.contains(&"switch-buffer"),
            "switch-buffer should match sb (s+b on word boundaries), got {:?}",
            names
        );
    }

    #[test]
    fn help_commands_are_searchable() {
        // Guard the primary motivation for this overlay: help and
        // describe-* commands ship in the registry and therefore must
        // appear in the palette. Each family has to be reachable via a
        // plausible query.
        let reg = CommandRegistry::with_builtins();

        let mut palette = CommandPalette::from_registry(&reg);
        palette.query = "help".into();
        palette.update_filter();
        let help_names: Vec<&str> = palette
            .filtered
            .iter()
            .map(|&i| palette.entries[i].name.as_str())
            .collect();
        assert!(
            help_names.contains(&"help"),
            "'help' not found via query 'help'"
        );
        assert!(
            help_names.iter().any(|n| n.starts_with("help-")),
            "help-* commands should be reachable via 'help' query"
        );

        let mut palette = CommandPalette::from_registry(&reg);
        palette.query = "describe".into();
        palette.update_filter();
        let desc_names: Vec<&str> = palette
            .filtered
            .iter()
            .map(|&i| palette.entries[i].name.as_str())
            .collect();
        assert!(desc_names.contains(&"describe-key"));
        assert!(desc_names.contains(&"describe-command"));
    }

    #[test]
    fn no_match_leaves_filtered_empty() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("save", "Save");
        let mut palette = CommandPalette::from_registry(&reg);
        palette.query = "zzzzzz".into();
        palette.update_filter();
        assert!(palette.filtered.is_empty());
        assert_eq!(palette.selected_name(), None);
    }

    #[test]
    fn filter_prefers_exact_prefix() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("save", "Save current buffer");
        reg.register_builtin("save-and-quit", "Save and quit");
        reg.register_builtin("force-quit", "Quit without saving");
        let mut palette = CommandPalette::from_registry(&reg);
        palette.query = "save".into();
        palette.update_filter();
        assert_eq!(
            palette.entries[palette.filtered[0]].name, "save",
            "exact prefix match should come first"
        );
    }

    #[test]
    fn selected_wraps_around() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("a", "a");
        reg.register_builtin("b", "b");
        reg.register_builtin("c", "c");
        let mut palette = CommandPalette::from_registry(&reg);
        assert_eq!(palette.selected, 0);
        palette.move_up(); // wraps to last
        assert_eq!(palette.selected, 2);
        palette.move_down(); // wraps back to 0
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn selected_name_returns_current_entry() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("a", "a");
        reg.register_builtin("b", "b");
        let palette = CommandPalette::from_registry(&reg);
        assert_eq!(palette.selected_name(), Some("a"));
    }

    #[test]
    fn selection_resets_after_filter() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("aaa", "");
        reg.register_builtin("aab", "");
        reg.register_builtin("aac", "");
        let mut palette = CommandPalette::from_registry(&reg);
        palette.selected = 2;
        palette.query = "a".into();
        palette.update_filter();
        assert_eq!(palette.selected, 0, "selection must reset on filter");
    }

    #[test]
    fn palette_searchable_extra_matches() {
        let nodes = vec![(
            "zed-arch".to_string(),
            "Zed Architecture".to_string(),
            "The collaboration layer uses DeltaDB for state sync.".to_string(),
        )];
        let mut palette = CommandPalette::for_kb_find_or_create(&nodes);
        palette.query = "DeltaDB".into();
        palette.update_filter();
        assert_eq!(
            palette.filtered.len(),
            1,
            "body content in searchable_extra should match"
        );
    }

    #[test]
    fn palette_title_match_ranks_above_body_match() {
        let nodes = vec![
            (
                "a".to_string(),
                "DeltaDB Overview".to_string(),
                "empty body".to_string(),
            ),
            (
                "b".to_string(),
                "Zed Architecture".to_string(),
                "Uses DeltaDB for collaboration".to_string(),
            ),
        ];
        let mut palette = CommandPalette::for_kb_find_or_create(&nodes);
        palette.query = "DeltaDB".into();
        palette.update_filter();
        assert_eq!(palette.filtered.len(), 2);
        // Title match (node a) should rank first
        assert_eq!(
            palette.entries[palette.filtered[0]].name, "a",
            "title match should rank above body match"
        );
    }
}
