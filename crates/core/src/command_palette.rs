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
    HelpSearch,
    SwitchBuffer,
    SetSplashArt,
    RecentFile,
    SwitchProject,
    AiMode,
    AiProfile,
    GitBranch,
    MiniDialog,
}

impl PalettePurpose {
    /// Human-readable label for this palette kind, used in popup titles.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Execute => "Commands",
            Self::Describe => "Describe Command",
            Self::SetTheme => "Themes",
            Self::HelpSearch => "Help Topics",
            Self::SwitchBuffer => "Buffers",
            Self::SetSplashArt => "Splash Art",
            Self::RecentFile => "Recent Files",
            Self::SwitchProject => "Projects",
            Self::AiMode => "AI Operating Mode",
            Self::AiProfile => "AI Prompt Profile",
            Self::GitBranch => "Git Branch",
            Self::MiniDialog => "Dialog",
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
            MiniDialogKind::Confirm => "Confirm",
            MiniDialogKind::SingleInput => match &self.context {
                MiniDialogContext::FileRename { .. } | MiniDialogContext::FileTreeRename { .. } => {
                    "Rename"
                }
                MiniDialogContext::FileCopy { .. } => "Copy File",
                MiniDialogContext::FileSaveAs => "Save As",
                MiniDialogContext::FileTreeCreate { .. } => "Create",
                MiniDialogContext::OrgSetTags { .. } => "Set Tags",
                MiniDialogContext::AgendaFilterTag => "Filter by Tag",
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
    /// the selected node in the help buffer. Used by `SPC h s`.
    pub fn for_help_search(nodes: &[(String, String)]) -> Self {
        let mut entries: Vec<PaletteEntry> = nodes
            .iter()
            .map(|(id, title)| PaletteEntry {
                name: id.clone(),
                doc: title.clone(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::HelpSearch,
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

    /// Splash art picker palette. More art variants will be added in a
    /// follow-up PR — the infrastructure supports any number of entries.
    pub fn for_splash_art() -> Self {
        let entries = vec![PaletteEntry {
            name: "bat".to_string(),
            doc: "Bat with spread wings".to_string(),
        }];
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose: PalettePurpose::SetSplashArt,
        }
    }

    fn with_name_list(names: &[&str], purpose: PalettePurpose) -> Self {
        let entries: Vec<PaletteEntry> = names
            .iter()
            .map(|n| PaletteEntry {
                name: n.to_string(),
                doc: String::new(),
            })
            .collect();
        let filtered: Vec<usize> = (0..entries.len()).collect();
        CommandPalette {
            query: String::new(),
            entries,
            filtered,
            selected: 0,
            purpose,
        }
    }

    fn with_purpose(reg: &CommandRegistry, purpose: PalettePurpose) -> Self {
        let mut entries: Vec<PaletteEntry> = reg
            .list_commands()
            .into_iter()
            .map(|c| PaletteEntry {
                name: c.name.clone(),
                doc: c.doc.clone(),
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
                    name_score.max(doc_score).map(|s| (idx, s))
                })
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.1));
            self.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
        }
        self.selected = 0;
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Name of the currently selected command, if any.
    pub fn selected_name(&self) -> Option<&str> {
        if self.filtered.is_empty() {
            return None;
        }
        let idx = self.filtered[self.selected];
        Some(&self.entries[idx].name)
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
            PalettePurpose::HelpSearch,
            PalettePurpose::SwitchBuffer,
            PalettePurpose::SetSplashArt,
            PalettePurpose::RecentFile,
            PalettePurpose::SwitchProject,
            PalettePurpose::AiMode,
            PalettePurpose::AiProfile,
            PalettePurpose::GitBranch,
            PalettePurpose::MiniDialog,
        ];
        for p in &purposes {
            assert!(!p.label().is_empty(), "{:?} has empty label", p);
        }
        assert_eq!(PalettePurpose::Execute.label(), "Commands");
        assert_eq!(PalettePurpose::GitBranch.label(), "Git Branch");
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
}
