pub mod buffer;
pub mod clipboard;
pub mod command_palette;
pub mod commands;
pub mod conversation;
pub mod dap_intent;
pub mod debug;
pub mod debug_view;
pub mod diff;
pub mod editor;
pub mod event_record;
pub mod file_browser;
pub mod file_picker;
pub mod file_tree;
pub mod git_status;
pub mod grapheme;
pub mod heading;
pub mod help_view;
pub mod hooks;
pub mod input;
pub mod kb_seed;
pub mod keymap;
pub mod link_detect;
pub mod lock_stats;
pub mod lsp_intent;
pub mod messages;
pub mod options;
pub mod project;
pub mod render_common;
pub mod search;
pub mod session;
pub mod syntax;
pub mod theme;
pub mod visual_buffer;
pub mod window;
pub mod word;
pub mod wrap;

pub use buffer::{Buffer, BufferKind};
pub use command_palette::{CommandPalette, PaletteEntry, PalettePurpose};
pub use commands::{Command, CommandRegistry, CommandSource};
pub use conversation::Conversation;
pub use dap_intent::{BreakpointSpec, DapIntent, DapSpawnConfig, StepKind};
pub use debug::{
    Breakpoint, DebugState, DebugTarget, DebugThread, SchemeErrorEntry, Scope, StackFrame, Variable,
};
pub use debug_view::{DebugLineItem, DebugView};
pub use editor::{
    CompletionItem, Diagnostic, DiagnosticSeverity, DiagnosticStore, EditRecord, Editor, InputLock,
    LspLocation, LspRange, LspServerStatus,
};
pub use file_browser::{Activation as BrowserActivation, BrowserEntry, FileBrowser};
pub use file_picker::FilePicker;
pub use help_view::{HelpLinkSpan, HelpView};
pub use hooks::HookRegistry;
pub use input::{InputEvent, MouseButton};
pub use keymap::{
    parse_key_seq, parse_key_seq_spaced, Key, KeyPress, Keymap, LookupResult, WhichKeyEntry,
};
pub use lsp_intent::{language_id_from_path, path_to_uri, LspIntent};
pub use mae_kb::{parse_links, KnowledgeBase, Node as KbNode, NodeKind as KbNodeKind};
pub use messages::{LogEntry, MessageLevel, MessageLog, MessageLogHandle};
pub use options::{OptionDef, OptionKind, OptionRegistry};
pub use project::{detect_project_root, Project, ProjectConfig, RecentFiles};
pub use search::{SearchDirection, SearchMatch, SearchState};
pub use syntax::{language_for_path, HighlightSpan, Language, SyntaxMap, SyntaxSpanMap};
pub use theme::{
    bundled_theme_names, default_theme, BundledResolver, NamedColor, Theme, ThemeColor, ThemeError,
    ThemeResolver, ThemeStyle,
};
pub use window::{
    Direction, LayoutNode, Rect as WinRect, SplitDirection, Window, WindowId, WindowManager,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualType {
    Char,
    Line,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual(VisualType),
    Command,
    ConversationInput,
    Search,
    FilePicker,
    FileBrowser,
    CommandPalette,
    /// Git status "porcelain" UI (Phase 6 M5).
    GitStatus,
    /// Terminal emulator — keys go directly to PTY. Exit with Ctrl-\ Ctrl-n.
    ShellInsert,
}
