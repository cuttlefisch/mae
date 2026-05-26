//! Scheme LSP bridge — handles LSP intents for `.scm` files in-process.
//!
//! Instead of forwarding scheme intents to an external LSP server, this
//! module queries the live SchemeRuntime's VM directly (Swank-style).
//! Responses are applied to the editor synchronously.

use mae_core::{
    CompletionItem as CoreCompletionItem, Diagnostic as CoreDiagnostic,
    DiagnosticSeverity as CoreSeverity, Editor, LspIntent,
};
use mae_scheme::lsp as scheme_lsp;
use mae_scheme::SchemeRuntime;
use tracing::debug;

/// Extract the `language_id` from an `LspIntent`, if present.
fn intent_language_id(intent: &LspIntent) -> Option<&str> {
    match intent {
        LspIntent::DidOpen { language_id, .. }
        | LspIntent::DidChange { language_id, .. }
        | LspIntent::DidSave { language_id, .. }
        | LspIntent::DidClose { language_id, .. }
        | LspIntent::GotoDefinition { language_id, .. }
        | LspIntent::FindReferences { language_id, .. }
        | LspIntent::Hover { language_id, .. }
        | LspIntent::Completion { language_id, .. }
        | LspIntent::CodeAction { language_id, .. }
        | LspIntent::PrepareRename { language_id, .. }
        | LspIntent::Rename { language_id, .. }
        | LspIntent::Format { language_id, .. }
        | LspIntent::RangeFormat { language_id, .. }
        | LspIntent::WorkspaceSymbol { language_id, .. }
        | LspIntent::DocumentSymbols { language_id, .. }
        | LspIntent::DocumentHighlight { language_id, .. }
        | LspIntent::SignatureHelp { language_id, .. } => Some(language_id.as_str()),
    }
}

/// Drain scheme-specific LSP intents from the editor and handle them in-process.
///
/// Call this BEFORE `drain_lsp_intents` so scheme intents never reach the
/// external LSP task. Returns true if any intent was handled (needs redraw).
pub(crate) fn drain_scheme_lsp_intents(editor: &mut Editor, scheme: &SchemeRuntime) -> bool {
    if editor.pending_lsp_requests.is_empty() {
        return false;
    }

    // Partition: scheme intents handled here, others left for drain_lsp_intents
    let mut scheme_intents = Vec::new();
    let mut other_intents = Vec::new();
    for intent in std::mem::take(&mut editor.pending_lsp_requests) {
        if intent_language_id(&intent) == Some("scheme") {
            scheme_intents.push(intent);
        } else {
            other_intents.push(intent);
        }
    }
    editor.pending_lsp_requests = other_intents;

    if scheme_intents.is_empty() {
        return false;
    }

    let vm = scheme.vm();
    let mut needs_redraw = false;

    for intent in scheme_intents {
        match intent {
            LspIntent::DidOpen { uri, text, .. } | LspIntent::DidChange { uri, text, .. } => {
                // Run diagnostics on the changed buffer
                let file = uri_to_file(&uri);
                let diags = scheme_lsp::diagnostics(vm, &text, &file);
                let core_diags: Vec<CoreDiagnostic> = diags
                    .into_iter()
                    .map(|d| CoreDiagnostic {
                        line: d.line,
                        col_start: d.column,
                        col_end: d.column,
                        end_line: d.line,
                        severity: match d.severity {
                            scheme_lsp::SchemeDiagnosticSeverity::Error => CoreSeverity::Error,
                            scheme_lsp::SchemeDiagnosticSeverity::Warning => CoreSeverity::Warning,
                        },
                        message: d.message,
                        source: Some("mae-scheme".into()),
                        code: None,
                    })
                    .collect();
                let count = core_diags.len();
                let changed = editor.diagnostics.set(uri.clone(), core_diags);
                debug!(uri = %uri, count, "scheme diagnostics published");
                if changed {
                    needs_redraw = true;
                }
            }
            LspIntent::Completion {
                uri,
                line,
                character,
                ..
            } => {
                // Get line text from the active buffer
                let line_text = get_buffer_line(editor, &uri, line as usize);
                let (prefix, _) = scheme_lsp::extract_word_at(&line_text, character);
                let completions = scheme_lsp::completions(vm, &prefix);
                let core_items: Vec<CoreCompletionItem> = completions
                    .into_iter()
                    .map(|c| {
                        let sigil = match c.kind {
                            scheme_lsp::SchemeSymbolKind::Function => 'f',
                            scheme_lsp::SchemeSymbolKind::Variable => 'v',
                            scheme_lsp::SchemeSymbolKind::Keyword => 'k',
                            scheme_lsp::SchemeSymbolKind::Macro => 'm',
                        };
                        CoreCompletionItem {
                            insert_text: c.label.clone(),
                            label: c.label,
                            detail: c.detail,
                            kind_sigil: sigil,
                        }
                    })
                    .collect();
                editor.apply_completion_result(core_items);
                needs_redraw = true;
            }
            LspIntent::Hover {
                uri,
                line,
                character,
                ..
            } => {
                let line_text = get_buffer_line(editor, &uri, line as usize);
                let (symbol, _) = scheme_lsp::extract_word_at(&line_text, character);
                if let Some(hover) = scheme_lsp::hover(vm, &symbol) {
                    editor.apply_hover_result(hover.contents);
                    needs_redraw = true;
                }
            }
            LspIntent::DocumentSymbols { uri, .. } => {
                if let Some(text) = get_buffer_text(editor, &uri) {
                    let file = uri_to_file(&uri);
                    let symbols = scheme_lsp::document_symbols(&text, &file);
                    let entries: Vec<mae_core::SymbolOutlineEntry> = symbols
                        .into_iter()
                        .map(|s| {
                            let kind_icon = match s.kind {
                                scheme_lsp::SchemeSymbolKind::Function => 'f',
                                scheme_lsp::SchemeSymbolKind::Variable => 'v',
                                scheme_lsp::SchemeSymbolKind::Keyword => 'k',
                                scheme_lsp::SchemeSymbolKind::Macro => 'm',
                            };
                            mae_core::SymbolOutlineEntry {
                                name: s.name,
                                kind: format!("{:?}", s.kind),
                                kind_icon,
                                line: s.line as usize,
                                depth: 0,
                                detail: None,
                            }
                        })
                        .collect();
                    if editor.symbol_outline_pending {
                        editor.apply_symbol_outline_result(&entries);
                        needs_redraw = true;
                    } else if editor.breadcrumb_symbols_pending {
                        editor.apply_breadcrumb_symbols(&entries);
                        needs_redraw = true;
                    }
                }
            }
            LspIntent::SignatureHelp {
                uri,
                line,
                character,
                ..
            } => {
                let line_text = get_buffer_line(editor, &uri, line as usize);
                let symbol = find_enclosing_call(&line_text, character as usize);
                if let Some(sig) = symbol.and_then(|s| scheme_lsp::signature_help(vm, &s)) {
                    // Compute byte offsets of each parameter in the label string
                    let mut params = Vec::new();
                    for p in &sig.parameters {
                        if let Some(start) = sig.label.find(p.as_str()) {
                            params.push((start, start + p.len()));
                        }
                    }
                    let infos = vec![mae_core::SignatureHelpInfo {
                        label: sig.label,
                        parameters: params,
                        documentation: sig.documentation,
                    }];
                    editor.apply_signature_help_result(infos, 0, 0);
                    needs_redraw = true;
                }
            }
            // Notifications we can safely ignore for the in-process LSP
            LspIntent::DidSave { .. } | LspIntent::DidClose { .. } => {}
            LspIntent::GotoDefinition {
                uri,
                line,
                character,
                ..
            } => {
                let line_text = get_buffer_line(editor, &uri, line as usize);
                let (symbol, _) = scheme_lsp::extract_word_at(&line_text, character);
                if let Some(loc) = scheme_lsp::goto_definition(vm, &symbol) {
                    // Convert SourceLocation to LspLocation format
                    let target_uri = if loc.file.starts_with('/') {
                        format!("file://{}", loc.file)
                    } else {
                        uri.clone() // Same file
                    };
                    let target_line = loc.line.saturating_sub(1) as usize; // 1-indexed → 0-indexed
                    let core_loc = mae_core::LspLocation {
                        uri: target_uri,
                        range: mae_core::LspRange {
                            start_line: target_line as u32,
                            start_character: loc.column.saturating_sub(1),
                            end_line: target_line as u32,
                            end_character: loc.column.saturating_sub(1),
                        },
                    };
                    if let Some(other_file) = editor.apply_definition_result(vec![core_loc]) {
                        // Need to open the file — queue it
                        let path = other_file
                            .uri
                            .strip_prefix("file://")
                            .unwrap_or(&other_file.uri);
                        editor.open_file(path);
                    }
                    needs_redraw = true;
                } else {
                    editor.set_status(format!("[Scheme LSP] no definition for '{}'", symbol));
                    needs_redraw = true;
                }
            }
            LspIntent::FindReferences { .. } => {
                editor.set_status("[Scheme LSP] find-references not yet implemented");
                needs_redraw = true;
            }
            _ => {
                debug!("scheme LSP: unhandled intent");
            }
        }
    }

    // Mark scheme LSP as connected (synthetic — no external server)
    if !editor.lsp_servers.contains_key("scheme") {
        editor.lsp_servers.insert(
            "scheme".to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Connected,
                command: "mae-scheme (in-process)".into(),
                binary_found: true,
            },
        );
    }

    needs_redraw
}

/// Strip `file://` prefix to get a filename for diagnostics.
fn uri_to_file(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// Get a specific line of text from a buffer matching the given URI.
fn get_buffer_line(editor: &Editor, uri: &str, line: usize) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    for buf in &editor.buffers {
        if let Some(fp) = buf.file_path() {
            if fp.to_string_lossy() == path {
                if line < buf.display_line_count() {
                    return buf.line_text(line);
                }
                break;
            }
        }
    }
    String::new()
}

/// Get the full text of a buffer matching the given URI.
fn get_buffer_text(editor: &Editor, uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    for buf in &editor.buffers {
        if let Some(fp) = buf.file_path() {
            if fp.to_string_lossy() == path {
                return Some(buf.text());
            }
        }
    }
    None
}

/// Find the enclosing function call name at a given position.
/// Scans backwards from `col` looking for `(name`.
fn find_enclosing_call(line: &str, col: usize) -> Option<String> {
    let bytes = line.as_bytes();
    let mut pos = col.min(bytes.len());

    // Scan backwards for opening paren
    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b'(' {
            // Found opening paren — extract the symbol after it
            let rest = &line[pos + 1..];
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
                .unwrap_or(rest.len());
            let name = &rest[..end];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_language_id() {
        let intent = LspIntent::Completion {
            uri: "file:///test.scm".into(),
            language_id: "scheme".into(),
            line: 0,
            character: 0,
        };
        assert_eq!(intent_language_id(&intent), Some("scheme"));
    }

    #[test]
    fn test_find_enclosing_call() {
        assert_eq!(
            find_enclosing_call("(map f xs)", 5),
            Some("map".to_string())
        );
        assert_eq!(
            find_enclosing_call("(define (foo x) body)", 13),
            Some("foo".to_string())
        );
        assert_eq!(find_enclosing_call("hello", 3), None);
    }

    #[test]
    fn test_uri_to_file() {
        assert_eq!(uri_to_file("file:///tmp/test.scm"), "/tmp/test.scm");
        assert_eq!(uri_to_file("/tmp/test.scm"), "/tmp/test.scm");
    }
}
