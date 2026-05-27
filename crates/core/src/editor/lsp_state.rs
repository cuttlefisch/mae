//! LSP (Language Server Protocol) state extracted from Editor.
//! Runtime state for completion, hover, peek, symbols, code actions,
//! diagnostics, and intent queues. Option fields (completion_max_items,
//! lsp_hover_popup, etc.) remain on Editor as they are registered in
//! OptionRegistry and exposed via `(set-option!)`.

use std::collections::HashMap;

use super::diagnostics::DiagnosticStore;
use super::{
    CodeActionMenu, CompletionItem, DocumentHighlightRange, HoverPopup, LspServerInfo,
    PeekReferencesState, PeekState, SignatureHelpState, SymbolOutlineEntry, SymbolOutlineState,
};
use crate::lsp_intent::LspIntent;

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
