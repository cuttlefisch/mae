//! LSP bridge — translates between editor-side intents and LSP transport commands,
//! and handles incoming LSP events.

use mae_ai::{DeferredKind, ToolResult};
use mae_core::{
    CompletionItem as CoreCompletionItem, Diagnostic as CoreDiagnostic,
    DiagnosticSeverity as CoreSeverity, Editor, LspIntent, LspLocation, LspRange,
};
use mae_lsp::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, LspCommand, LspTaskEvent, Position,
};
use tracing::{debug, info, warn};

/// Drain all pending LSP intents from the editor and forward them to the LSP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
pub(crate) fn drain_lsp_intents(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
) {
    if editor.pending_lsp_requests.is_empty() {
        return;
    }
    let intents = std::mem::take(&mut editor.pending_lsp_requests);
    for intent in intents {
        let cmd = intent_to_lsp_command(intent);
        if lsp_tx.try_send(cmd).is_err() {
            warn!("LSP command channel full or closed — intent dropped");
        }
    }
}

/// Translate an editor-side `LspIntent` into a transport-layer `LspCommand`.
fn intent_to_lsp_command(intent: LspIntent) -> LspCommand {
    match intent {
        LspIntent::DidOpen {
            uri,
            language_id,
            text,
        } => LspCommand::DidOpen {
            uri,
            language_id,
            text,
        },
        LspIntent::DidChange {
            uri,
            language_id,
            text,
        } => LspCommand::DidChange {
            uri,
            language_id,
            text,
        },
        LspIntent::DidSave {
            uri,
            language_id,
            text,
        } => LspCommand::DidSave {
            uri,
            language_id,
            text,
        },
        LspIntent::DidClose { uri, language_id } => LspCommand::DidClose { uri, language_id },
        LspIntent::GotoDefinition {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::GotoDefinition {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::FindReferences {
            uri,
            language_id,
            line,
            character,
            include_declaration,
        } => LspCommand::FindReferences {
            uri,
            language_id,
            position: Position { line, character },
            include_declaration,
        },
        LspIntent::Hover {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::Hover {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::Completion {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::Completion {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::WorkspaceSymbol { language_id, query } => {
            LspCommand::WorkspaceSymbol { language_id, query }
        }
        LspIntent::DocumentSymbols { uri, language_id } => {
            LspCommand::DocumentSymbols { uri, language_id }
        }
        // Stubs: these intents are queued but the LSP client doesn't
        // handle them yet. Log and ignore until Phase 4a M5.
        LspIntent::CodeAction { .. } | LspIntent::Rename { .. } | LspIntent::Format { .. } => {
            LspCommand::DidClose {
                uri: String::new(),
                language_id: String::new(),
            }
        }
    }
}

/// Handle an event from the LSP task — update editor state or open a new buffer.
pub(crate) fn handle_lsp_event(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    event: LspTaskEvent,
) {
    match event {
        LspTaskEvent::ServerStarted { language_id } => {
            info!(language = %language_id, "LSP server started");
            editor.set_status(format!("[LSP] {} server started", language_id));
        }
        LspTaskEvent::ServerStartFailed { language_id, error } => {
            warn!(language = %language_id, error = %error, "LSP server failed to start");
            editor.set_status(format!("[LSP] {}: {}", language_id, error));
        }
        LspTaskEvent::ServerExited { language_id } => {
            warn!(language = %language_id, "LSP server exited");
            editor.set_status(format!("[LSP] {} server exited", language_id));
        }
        LspTaskEvent::DefinitionResult { uri: _, locations } => {
            let core_locs: Vec<LspLocation> = locations
                .into_iter()
                .map(|l| LspLocation {
                    uri: l.uri,
                    range: LspRange {
                        start_line: l.range.start.line,
                        start_character: l.range.start.character,
                        end_line: l.range.end.line,
                        end_character: l.range.end.character,
                    },
                })
                .collect();
            if let Some(other_file_loc) = editor.apply_definition_result(core_locs) {
                // Different file — open it and jump.
                open_location(editor, lsp_tx, other_file_loc);
            }
        }
        LspTaskEvent::ReferencesResult { uri: _, locations } => {
            let core_locs: Vec<LspLocation> = locations
                .into_iter()
                .map(|l| LspLocation {
                    uri: l.uri,
                    range: LspRange {
                        start_line: l.range.start.line,
                        start_character: l.range.start.character,
                        end_line: l.range.end.line,
                        end_character: l.range.end.character,
                    },
                })
                .collect();
            editor.apply_references_result(core_locs);
        }
        LspTaskEvent::HoverResult { contents, .. } => {
            editor.apply_hover_result(contents);
        }
        LspTaskEvent::DiagnosticsPublished { uri, diagnostics } => {
            let count = diagnostics.len();
            let core_diags: Vec<CoreDiagnostic> =
                diagnostics.into_iter().map(lsp_diag_to_core).collect();
            editor.diagnostics.set(uri.clone(), core_diags);
            debug!(uri = %uri, count, "diagnostics published");
            // Surface a summary in the status line so users notice new
            // problems without having to open the diagnostics buffer.
            let (e, w, _, _) = editor.diagnostics.severity_counts();
            if e + w > 0 {
                editor.set_status(format!("[LSP] {} errors, {} warnings", e, w));
            }
        }
        LspTaskEvent::ServerNotification {
            language_id,
            notification,
        } => {
            debug!(
                language = %language_id,
                method = %notification.method,
                "LSP server notification"
            );
        }
        LspTaskEvent::CompletionResult { uri: _, items, .. } => {
            let core_items: Vec<CoreCompletionItem> = items
                .into_iter()
                .map(|item| CoreCompletionItem {
                    insert_text: item.insert_text.unwrap_or_else(|| item.label.clone()),
                    label: item.label,
                    detail: item.detail,
                    kind_sigil: item.kind.sigil(),
                })
                .collect();
            editor.apply_completion_result(core_items);
        }
        // Workspace/document symbol results are only consumed by the deferred
        // AI tool flow (try_complete_deferred). If no deferred call is pending
        // they are silently dropped here.
        LspTaskEvent::WorkspaceSymbolResult { .. } => {}
        LspTaskEvent::DocumentSymbolResult { .. } => {}
        LspTaskEvent::Error { message } => {
            warn!(error = %message, "LSP error");
            editor.set_status(format!("[LSP] {}", message));
        }
    }
}

/// Check if an incoming LSP event matches a pending deferred AI tool call.
/// If so, format a structured JSON result and return it. The caller is
/// responsible for sending it via the held oneshot reply channel.
pub(crate) fn try_complete_deferred(
    event: &LspTaskEvent,
    kind: DeferredKind,
    tool_call_id: &str,
) -> Option<ToolResult> {
    match (kind, event) {
        (DeferredKind::LspDefinition, LspTaskEvent::DefinitionResult { locations, .. }) => {
            let locs: Vec<serde_json::Value> = locations
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "uri": l.uri,
                        "path": l.uri.strip_prefix("file://").unwrap_or(&l.uri),
                        "line": l.range.start.line + 1,
                        "character": l.range.start.character + 1,
                        "end_line": l.range.end.line + 1,
                        "end_character": l.range.end.character + 1,
                    })
                })
                .collect();
            let output = if locs.is_empty() {
                serde_json::json!({"locations": [], "message": "definition not found"})
            } else {
                serde_json::json!({"locations": locs})
            };
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: "lsp_definition".into(),
                success: true,
                output: output.to_string(),
            })
        }
        (DeferredKind::LspReferences, LspTaskEvent::ReferencesResult { locations, .. }) => {
            let locs: Vec<serde_json::Value> = locations
                .iter()
                .map(|l| {
                    serde_json::json!({
                        "uri": l.uri,
                        "path": l.uri.strip_prefix("file://").unwrap_or(&l.uri),
                        "line": l.range.start.line + 1,
                        "character": l.range.start.character + 1,
                    })
                })
                .collect();
            let count = locs.len();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: "lsp_references".into(),
                success: true,
                output: serde_json::json!({"count": count, "references": locs}).to_string(),
            })
        }
        (DeferredKind::LspHover, LspTaskEvent::HoverResult { contents, .. }) => {
            let output = if contents.is_empty() {
                serde_json::json!({"contents": "", "message": "no hover info"})
            } else {
                serde_json::json!({"contents": contents})
            };
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: "lsp_hover".into(),
                success: true,
                output: output.to_string(),
            })
        }
        (DeferredKind::LspWorkspaceSymbol, LspTaskEvent::WorkspaceSymbolResult { symbols }) => {
            let syms: Vec<serde_json::Value> = symbols
                .iter()
                .map(|s| {
                    let mut obj = serde_json::json!({
                        "name": s.name,
                        "kind": s.kind.label(),
                        "path": s.location.uri.strip_prefix("file://").unwrap_or(&s.location.uri),
                        "line": s.location.range.start.line + 1,
                        "character": s.location.range.start.character + 1,
                    });
                    if let Some(ref cn) = s.container_name {
                        obj["container_name"] = serde_json::json!(cn);
                    }
                    obj
                })
                .collect();
            let count = syms.len();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: "lsp_workspace_symbol".into(),
                success: true,
                output: serde_json::json!({"count": count, "symbols": syms}).to_string(),
            })
        }
        (DeferredKind::LspDocumentSymbols, LspTaskEvent::DocumentSymbolResult { symbols, .. }) => {
            fn format_doc_symbol(s: &mae_lsp::protocol::DocumentSymbol) -> serde_json::Value {
                let mut obj = serde_json::json!({
                    "name": s.name,
                    "kind": s.kind.label(),
                    "line": s.range.start.line + 1,
                    "end_line": s.range.end.line + 1,
                });
                if let Some(ref d) = s.detail {
                    obj["detail"] = serde_json::json!(d);
                }
                if !s.children.is_empty() {
                    obj["children"] = serde_json::Value::Array(
                        s.children.iter().map(format_doc_symbol).collect(),
                    );
                }
                obj
            }
            let syms: Vec<serde_json::Value> = symbols.iter().map(format_doc_symbol).collect();
            Some(ToolResult {
                tool_call_id: tool_call_id.to_string(),
                tool_name: "lsp_document_symbols".into(),
                success: true,
                output: serde_json::json!({"symbols": syms}).to_string(),
            })
        }
        // Also handle LSP errors while a deferred call is pending
        (_, LspTaskEvent::Error { message }) => Some(ToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: match kind {
                DeferredKind::LspDefinition => "lsp_definition",
                DeferredKind::LspReferences => "lsp_references",
                DeferredKind::LspHover => "lsp_hover",
                DeferredKind::LspWorkspaceSymbol => "lsp_workspace_symbol",
                DeferredKind::LspDocumentSymbols => "lsp_document_symbols",
            }
            .into(),
            success: false,
            output: format!("LSP error: {}", message),
        }),
        _ => None,
    }
}

/// Strip `file://` prefix from a URI to get a filesystem path.
fn uri_to_path(uri: &str) -> Option<&str> {
    uri.strip_prefix("file://")
}

/// Open the buffer at `loc.uri` (if not already open) and jump the cursor to
/// `loc.range.start`. After opening we also forward a fresh didOpen intent
/// so the newly-focused buffer is known to the language server.
fn open_location(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    loc: LspLocation,
) {
    let Some(path) = uri_to_path(&loc.uri) else {
        editor.set_status(format!("[LSP] cannot open non-file URI: {}", loc.uri));
        return;
    };

    // If the buffer is already loaded, just switch to it.
    let existing = editor
        .buffers
        .iter()
        .position(|b| b.file_path().map(|p| p.to_string_lossy()) == Some(path.into()));

    match existing {
        Some(idx) => {
            editor.switch_to_buffer(idx);
        }
        None => {
            // open_file queues a didOpen via file_ops
            editor.open_file(path);
        }
    }

    // Place the cursor.
    let idx = editor.active_buffer_idx();
    let line_count = editor.buffers[idx].line_count();
    let target_row = (loc.range.start_line as usize).min(line_count.saturating_sub(1));
    let target_col = loc.range.start_character as usize;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = target_row;
    win.cursor_col = target_col;
    win.clamp_cursor(&editor.buffers[idx]);

    // Drain any intents produced by open_file.
    drain_lsp_intents(editor, lsp_tx);
    editor.set_status(format!(
        "[LSP] opened {} at {}:{}",
        path,
        target_row + 1,
        target_col + 1
    ));
}

/// Translate an `mae_lsp::Diagnostic` into the core representation.
/// The core crate has no LSP dependency, so the binary performs the crosswalk.
pub(crate) fn lsp_diag_to_core(d: LspDiagnostic) -> CoreDiagnostic {
    CoreDiagnostic {
        line: d.range.start.line,
        col_start: d.range.start.character,
        col_end: d.range.end.character,
        end_line: d.range.end.line,
        severity: match d.severity {
            DiagnosticSeverity::Error => CoreSeverity::Error,
            DiagnosticSeverity::Warning => CoreSeverity::Warning,
            DiagnosticSeverity::Information => CoreSeverity::Information,
            DiagnosticSeverity::Hint => CoreSeverity::Hint,
        },
        message: d.message,
        source: d.source,
        code: d.code,
    }
}
