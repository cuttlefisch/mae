pub mod buffer;
pub mod buffer_mode;
pub mod buffer_view;
pub mod clipboard;
pub mod command_palette;
pub mod commands;
pub mod conversation;
pub mod cursor;
pub mod dap_intent;
pub mod debug;
pub mod debug_view;
pub mod diff;
pub mod display_policy;
pub mod display_region;
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
pub mod image_meta;
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
pub mod swap;
pub mod syntax;
pub mod theme;
pub mod visual_buffer;
pub mod window;
pub mod word;
pub mod wrap;

pub use buffer::{Buffer, BufferKind, BufferLocalOptions};
pub use buffer_mode::BufferMode;
pub use buffer_view::BufferView;
pub use command_palette::{CommandPalette, PaletteEntry, PalettePurpose};
pub use commands::{Command, CommandRegistry, CommandSource};
pub use conversation::Conversation;
pub use dap_intent::{BreakpointSpec, DapIntent, DapSpawnConfig, StepKind};
pub use debug::{
    Breakpoint, DebugState, DebugTarget, DebugThread, SchemeErrorEntry, Scope, StackFrame, Variable,
};
pub use debug_view::{DebugLineItem, DebugView};
pub use editor::{
    CodeActionItem, CodeActionMenu, CompletionItem, Diagnostic, DiagnosticSeverity,
    DiagnosticStore, DocumentHighlightRange, EditRecord, Editor, HighlightKind, HoverPopup,
    InputLock, LspLocation, LspRange, LspServerInfo, LspServerStatus,
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
pub mod redraw;
pub use display_policy::{DisplayAction, DisplayPolicy};
pub use syntax::{
    compute_markdown_style_spans, compute_markup_spans, compute_org_style_spans,
    detect_code_block_lines, language_for_buffer, language_for_path, language_from_id,
    language_from_modeline, language_from_shebang, HighlightSpan, Language, MarkupFlavor,
    SyntaxMap, SyntaxSpanMap,
};
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
    /// Terminal emulator — keys go directly to PTY. Exit with Ctrl-\ Ctrl-n.
    ShellInsert,
}
