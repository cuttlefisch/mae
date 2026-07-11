//! LSP (Language Server Protocol) state extracted from Editor.
//! Runtime state for completion, hover, peek, symbols, code actions,
//! diagnostics, and intent queues. Option fields (completion_max_items,
//! lsp_hover_popup, etc.) remain on Editor as they are registered in
//! OptionRegistry and exposed via `(set-option!)`.

use std::collections::HashMap;

use super::diagnostics::DiagnosticStore;
use super::DocumentHighlightRange;
use crate::lsp_intent::LspIntent;

/// LSP server connection status, tracked per language_id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspServerStatus {
    Starting,
    Connected,
    Failed,
    Exited,
}

/// Rich LSP server info — status plus discovery metadata.
#[derive(Debug, Clone)]
pub struct LspServerInfo {
    /// Current connection status.
    pub status: LspServerStatus,
    /// The command used to start this server (e.g. "rust-analyzer").
    pub command: String,
    /// Whether the binary was found on PATH at startup.
    pub binary_found: bool,
}

/// A single item in the LSP completion popup.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Display label shown in the popup.
    pub label: String,
    /// Text to insert when accepted (falls back to `label`).
    pub insert_text: String,
    /// Brief detail (e.g. type signature).
    pub detail: Option<String>,
    /// Single-char sigil for the kind (f=function, v=variable, t=type, …).
    pub kind_sigil: char,
}

/// Floating popup showing LSP hover info near the cursor.
#[derive(Debug, Clone)]
pub struct HoverPopup {
    /// Raw markdown from LSP.
    pub contents: String,
    /// Buffer index where hover was requested.
    pub buffer_idx: usize,
    /// Buffer row where K was pressed.
    pub anchor_row: usize,
    /// Buffer col where K was pressed.
    pub anchor_col: usize,
    /// Scroll offset for long content.
    pub scroll_offset: usize,
}

/// Floating popup showing LSP signature help near the cursor.
#[derive(Debug, Clone)]
pub struct SignatureHelpState {
    /// Signatures from LSP.
    pub signatures: Vec<SignatureHelpInfo>,
    /// Which signature is active.
    pub active_signature: usize,
    /// Which parameter is active (highlighted).
    pub active_parameter: usize,
    /// Anchor position where the call started.
    pub anchor_line: usize,
    pub anchor_col: usize,
}

/// A single signature for display.
#[derive(Debug, Clone)]
pub struct SignatureHelpInfo {
    /// Full signature label (e.g. "fn foo(x: i32, y: &str) -> bool").
    pub label: String,
    /// Parameter byte offset ranges in `label`.
    pub parameters: Vec<(usize, usize)>,
    /// Documentation for this signature.
    pub documentation: Option<String>,
}

/// Inline preview of a definition without navigating away.
#[derive(Debug, Clone)]
pub struct PeekState {
    /// File path of the definition.
    pub file_path: String,
    /// Line number of the definition (0-indexed).
    pub line: usize,
    /// Column of the definition.
    pub col: usize,
    /// Context lines around the definition.
    pub context_lines: Vec<String>,
    /// Which line in context_lines is the definition itself.
    pub highlight_line: usize,
    /// Scroll offset within the peek window.
    pub scroll_offset: usize,
}

/// A single item in the LSP code action popup menu.
#[derive(Debug, Clone)]
pub struct CodeActionItem {
    /// Display title of the code action.
    pub title: String,
    /// The kind of the code action (e.g. "quickfix", "refactor").
    pub kind: Option<String>,
    /// JSON-serialized WorkspaceEdit to apply when selected.
    pub edit_json: Option<String>,
}

/// Code action popup menu shown after `SPC c a`.
#[derive(Debug, Clone)]
pub struct CodeActionMenu {
    pub items: Vec<CodeActionItem>,
    pub selected: usize,
}

/// A single entry in the symbol outline popup.
#[derive(Debug, Clone)]
pub struct SymbolOutlineEntry {
    pub name: String,
    /// Human-readable kind (e.g. "function", "struct").
    pub kind: String,
    /// Single-char icon for the kind.
    pub kind_icon: char,
    /// Line number (0-based) of the symbol.
    pub line: usize,
    /// Nesting depth (0 = top-level).
    pub depth: usize,
    /// Optional detail (e.g. type signature).
    pub detail: Option<String>,
}

/// Symbol outline popup state (SPC c o).
#[derive(Debug, Clone)]
pub struct SymbolOutlineState {
    pub entries: Vec<SymbolOutlineEntry>,
    pub selected: usize,
    pub filter: String,
    pub filtered_indices: Vec<usize>,
}

/// Peek references state — cycling through reference locations inline.
#[derive(Debug, Clone)]
pub struct PeekReferencesState {
    /// All reference locations.
    pub locations: Vec<PeekReferenceLocation>,
    /// Currently shown index.
    pub current: usize,
}

/// A single reference location for peek.
#[derive(Debug, Clone)]
pub struct PeekReferenceLocation {
    /// File path.
    pub path: String,
    /// Line number (0-indexed).
    pub line: usize,
    /// Column (0-indexed).
    pub col: usize,
    /// Context lines around the reference.
    pub context: Vec<String>,
}

/// LSP context: intent queues, popup state, diagnostics, and symbol caches.
/// Accessed via `editor.lsp.*`.
pub struct LspContext {
    /// Queue of pending LSP requests for the binary to drain each event-loop tick.
    /// The core cannot call async LSP code directly; instead, commands push
    /// intents here and `main.rs` forwards them to `run_lsp_task`.
    pub pending_requests: Vec<LspIntent>,
    /// LSP trigger characters per language (populated from server capabilities).
    pub trigger_characters: HashMap<String, Vec<String>>,
    /// Signal for the binary to send `workspace/didChangeWorkspaceFolders`
    /// when a project root is first detected after LSP has already started.
    pub pending_root_change: Option<String>,
    /// LSP server info (status + discovery metadata), keyed by language_id.
    pub servers: HashMap<String, LspServerInfo>,
    /// LSP diagnostics keyed by file URI. Replaced wholesale on each
    /// `publishDiagnostics` notification (the LSP contract).
    pub diagnostics: DiagnosticStore,
    /// LSP completion popup state. Empty = no popup visible.
    pub completion_items: Vec<CompletionItem>,
    /// Index of the currently selected completion item.
    pub completion_selected: usize,
    /// Active hover popup (shown via K when lsp_hover_popup=true).
    pub hover_popup: Option<HoverPopup>,
    /// Active signature help popup (triggered on `(` and `,` in insert mode).
    pub signature_help: Option<SignatureHelpState>,
    /// Peek definition preview (shown via SPC l p).
    pub peek_state: Option<PeekState>,
    /// When true, the next GotoDefinition result goes to peek_state instead of jumping.
    pub peek_definition_pending: bool,
    /// Peek references state (SPC l r) — cycle through reference locations in a preview.
    pub peek_references: Option<PeekReferencesState>,
    /// When true, the next FindReferences result populates peek_references.
    pub peek_references_pending: bool,
    /// Symbol outline popup state (SPC c o).
    pub symbol_outline: Option<SymbolOutlineState>,
    /// Whether a document symbol request is pending for the outline popup.
    pub symbol_outline_pending: bool,
    /// Current breadcrumb path (file > module > fn).
    pub breadcrumbs: Option<Vec<String>>,
    /// Cached document symbols for breadcrumb computation (from last symbol request).
    pub cached_doc_symbols: Vec<SymbolOutlineEntry>,
    /// Buffer index the cached symbols belong to.
    pub cached_doc_symbols_buf: Option<usize>,
    /// Whether a document symbol request is pending for breadcrumbs (not outline popup).
    pub breadcrumb_symbols_pending: bool,
    /// Active code action menu (shown via SPC c a).
    pub code_action_menu: Option<CodeActionMenu>,
    /// Symbol occurrence highlights from `textDocument/documentHighlight`.
    /// Cleared on every cursor move; repopulated after idle timeout.
    pub highlight_ranges: Vec<DocumentHighlightRange>,
    /// Generation counter — incremented on cursor move to invalidate stale highlights.
    pub highlight_generation: u64,
}

impl LspContext {
    pub fn new() -> Self {
        Self {
            pending_requests: Vec::new(),
            trigger_characters: HashMap::new(),
            pending_root_change: None,
            servers: HashMap::new(),
            diagnostics: DiagnosticStore::default(),
            completion_items: Vec::new(),
            completion_selected: 0,
            hover_popup: None,
            signature_help: None,
            peek_state: None,
            peek_definition_pending: false,
            peek_references: None,
            peek_references_pending: false,
            symbol_outline: None,
            symbol_outline_pending: false,
            breadcrumbs: None,
            cached_doc_symbols: Vec::new(),
            cached_doc_symbols_buf: None,
            breadcrumb_symbols_pending: false,
            code_action_menu: None,
            highlight_ranges: Vec::new(),
            highlight_generation: 0,
        }
    }
}

impl Default for LspContext {
    fn default() -> Self {
        Self::new()
    }
}
