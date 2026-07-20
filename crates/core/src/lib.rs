//! mae-core: Buffer management, editor state, event dispatch.
//!
//! @stability: stable
//! @since: 0.1.0

pub mod agenda_view;
pub use mae_babel as babel;
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
pub mod driven_window;
pub mod editor;
pub mod event_record;
pub use mae_export as export;
pub mod file_browser;
pub mod file_lock;
pub mod file_picker;
pub mod file_tree;
pub mod foldable_view;
pub mod git_status;
pub mod graph_view;
pub mod graph_view_support;
pub mod grapheme;
pub mod heading;
pub mod hooks;
pub mod image_meta;
pub mod input;
pub mod kb_seed;
pub mod kb_sharing;
pub mod kb_view;
pub mod keymap;
pub mod keymap_registry;
pub mod link_detect;
pub mod lock_stats;
pub mod lsp_intent;
pub mod messages;
pub mod notifications;
pub mod notifications_view;
pub mod options;
pub mod project;
pub mod render_common;
pub mod search;
pub mod session;
pub mod swap;
pub mod syntax;
pub mod table;
pub mod text_utils;
pub mod theme;
pub mod visual_buffer;
pub mod window;
pub mod word;
pub mod wrap;

pub use buffer::{BabelEditContext, Buffer, BufferKind, BufferLocalOptions};

/// A Scheme-defined AI tool registered via `register-ai-tool!`.
#[derive(Debug, Clone)]
pub struct SchemeToolDef {
    pub name: String,
    pub description: String,
    /// (param_name, param_type, param_description)
    pub params: Vec<(String, String, String)>,
    pub required: Vec<String>,
    /// Scheme function name to call with JSON args
    pub handler_fn: String,
    /// Permission tier: "read", "write", "shell", "privileged"
    pub permission: String,
}
pub use buffer_mode::BufferMode;
pub use buffer_view::BufferView;
pub use command_palette::{CommandPalette, PaletteEntry, PalettePurpose};
pub use commands::{Command, CommandRegistry, CommandSource};
pub use conversation::Conversation;
pub use dap_intent::{BreakpointSpec, DapIntent, DapSpawnConfig, StepKind};
pub use debug::{
    Breakpoint, DebugState, DebugTarget, DebugThread, SchemeErrorEntry, Scope, StackFrame,
    Variable, WatchExpression,
};
pub use debug_view::{DebugLineItem, DebugView};
pub use editor::{
    is_builtin_node, BlameEntry, BlameOverlay, CaptureState, CodeActionItem, CodeActionMenu,
    CollabIntent, CollabStatus, CompletionItem, DaemonControl, DaemonMode, Diagnostic,
    DiagnosticSeverity, DiagnosticStore, DocumentHighlightRange, EditRecord, Editor, HighlightKind,
    HoverPopup, InputLock, JoinedNode, KbCollabAction, KbResolution, LspLocation, LspRange,
    LspServerInfo, LspServerStatus, PeekReferenceLocation, PeekReferencesState, PeekState,
    SignatureHelpInfo, SignatureHelpState, SymbolOutlineEntry, SymbolOutlineState,
    DEFAULT_COLLAB_ADDRESS, DEFAULT_COLLAB_PORT, KB_DEFAULT_NAME, KB_SYNC_MODE_DEFAULT,
};
pub use file_browser::{Activation as BrowserActivation, BrowserEntry, FileBrowser};
pub use file_picker::FilePicker;
pub use graph_view::{
    flatten_scene_graph, GraphLayoutIntent, GraphLayoutMode, GraphNavDirection, GraphStyleOptions,
    GraphView, GraphViewIntent,
};
pub use hooks::HookRegistry;
pub use input::{InputEvent, MouseButton};
pub use kb_view::{KbLinkSpan, KbPreviewIntent, KbPreviewPopup, KbView};
pub use keymap::{
    parse_key_seq, parse_key_seq_spaced, Key, KeyPress, Keymap, LookupResult, WhichKeyEntry,
};
pub use lsp_intent::{language_id_from_path, path_to_uri, LspIntent};
pub use mae_kb::{
    parse_links, BrokenLink, BrokenLinkKind, KnowledgeBase, Node as KbNode, NodeKind as KbNodeKind,
};
pub use messages::{LogEntry, MessageLevel, MessageLog, MessageLogHandle};
pub use options::{OptionDef, OptionKind, OptionRegistry};
pub use project::{
    detect_project_root, Project, ProjectConfig, ProjectList, RecentFiles, RecentProjects,
};
pub use search::{SearchDirection, SearchMatch, SearchState};
pub mod redraw;
pub use display_policy::{DisplayAction, DisplayPolicy};
pub use driven_window::DrivenWindow;
pub use syntax::{
    compute_markdown_style_spans, compute_markup_spans, compute_markup_spans_for_range,
    compute_org_style_spans, detect_code_block_lines, detect_code_block_lines_for_range,
    language_for_buffer, language_for_path, language_from_id, language_from_modeline,
    language_from_shebang, HighlightSpan, Language, MarkupCache, MarkupFlavor, SyntaxMap,
    SyntaxSpanMap, ViewportCodeBlockCache,
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
