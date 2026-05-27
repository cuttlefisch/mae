//! LSP bridge — translates between editor-side intents and LSP transport commands,
//! and handles incoming LSP events.

use mae_ai::{DeferredKind, ToolResult};
use mae_core::{
    CodeActionItem, CompletionItem as CoreCompletionItem, Diagnostic as CoreDiagnostic,
    DiagnosticSeverity as CoreSeverity, Editor, LspIntent, LspLocation, LspRange,
};
use mae_lsp::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, LspCommand, LspTaskEvent, Position,
};
use tracing::{debug, info, warn};

/// Convert LSP code actions to core CodeActionItems.
fn lsp_actions_to_items(actions: Vec<mae_lsp::CodeAction>) -> Vec<CodeActionItem> {
    actions
        .into_iter()
        .map(|a| {
            let edit_json = a.edit.as_ref().map(|we| {
                let entries: Vec<(String, Vec<serde_json::Value>)> = we
                    .changes
                    .iter()
                    .map(|(uri, edits)| {
                        let edits_json: Vec<serde_json::Value> = edits
                            .iter()
                            .map(|e| {
                                serde_json::json!({
                                    "start_line": e.range.start.line,
                                    "start_character": e.range.start.character,
                                    "end_line": e.range.end.line,
                                    "end_character": e.range.end.character,
                                    "new_text": e.new_text,
                                })
                            })
                            .collect();
                        (uri.clone(), edits_json)
                    })
                    .collect();
                serde_json::to_string(&entries).unwrap_or_default()
            });
            CodeActionItem {
                title: a.title,
                kind: a.kind,
                edit_json,
            }
        })
        .collect()
}

/// Drain all pending LSP intents from the editor and forward them to the LSP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
pub(crate) fn drain_lsp_intents(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
) {
    // Late project detection: update LSP root_uri.
    if let Some(root_uri) = editor.lsp.pending_root_change.take() {
        let _ = lsp_tx.try_send(LspCommand::DidChangeWorkspaceFolders {
            added: vec![root_uri],
        });
    }

    if editor.lsp.pending_requests.is_empty() {
        return;
    }
    let intents = std::mem::take(&mut editor.lsp.pending_requests);
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
        LspIntent::CodeAction {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::CodeAction {
            uri,
            language_id,
            range: mae_lsp::Range {
                start: Position { line, character },
                end: Position { line, character },
            },
            diagnostics: Vec::new(),
        },
        LspIntent::DocumentHighlight {
            uri,
            language_id,
            line,
            character,
            generation,
        } => LspCommand::DocumentHighlight {
            uri,
            language_id,
            position: Position { line, character },
            generation,
        },
        LspIntent::SignatureHelp {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::SignatureHelp {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::PrepareRename {
            uri,
            language_id,
            line,
            character,
        } => LspCommand::PrepareRename {
            uri,
            language_id,
            position: Position { line, character },
        },
        LspIntent::Rename {
            uri,
            language_id,
            line,
            character,
            new_name,
        } => LspCommand::Rename {
            uri,
            language_id,
            position: Position { line, character },
            new_name,
        },
        LspIntent::Format { uri, language_id } => LspCommand::Format { uri, language_id },
        LspIntent::RangeFormat {
            uri,
            language_id,
            start_line,
            start_char,
            end_line,
            end_char,
        } => LspCommand::RangeFormat {
            uri,
            language_id,
            start: Position {
                line: start_line,
                character: start_char,
            },
            end: Position {
                line: end_line,
                character: end_char,
            },
        },
    }
}

/// Handle an event from the LSP task — update editor state or open a new buffer.
/// Handle an LSP event. Returns `true` if the display needs a redraw.
pub(crate) fn handle_lsp_event(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    event: LspTaskEvent,
) -> bool {
    match event {
        LspTaskEvent::ServerStarted { language_id } => {
            info!(language = %language_id, "LSP server started (indexing)");
            // Don't set Connected yet — the server process is running but may
            // still be indexing. We transition to Connected only when we get
            // an actual successful response (diagnostics, hover, etc.) via
            // mark_connected_from_uri(). This prevents showing ✓ while the
            // server is still unable to answer queries.
            if !editor.lsp.servers.contains_key(&language_id) {
                editor.lsp.servers.insert(
                    language_id.clone(),
                    mae_core::LspServerInfo {
                        status: mae_core::LspServerStatus::Starting,
                        command: String::new(),
                        binary_found: true,
                    },
                );
            }
            // Re-send didOpen for all open buffers of this language so the
            // server knows about them and will push diagnostics once ready.
            resync_open_buffers(editor, lsp_tx, &language_id);
            editor.set_status(format!("[LSP] {} indexing\u{2026}", language_id));
            true
        }
        LspTaskEvent::ServerStartFailed { language_id, error } => {
            warn!(language = %language_id, error = %error, "LSP server failed to start");
            if let Some(info) = editor.lsp.servers.get_mut(&language_id) {
                info.status = mae_core::LspServerStatus::Failed;
            } else {
                editor.lsp.servers.insert(
                    language_id.clone(),
                    mae_core::LspServerInfo {
                        status: mae_core::LspServerStatus::Failed,
                        command: String::new(),
                        binary_found: false,
                    },
                );
            }
            editor.set_status(format!("[LSP] {}: {}", language_id, error));
            true
        }
        LspTaskEvent::ServerExited { language_id } => {
            warn!(language = %language_id, "LSP server exited");
            if let Some(info) = editor.lsp.servers.get_mut(&language_id) {
                info.status = mae_core::LspServerStatus::Exited;
            } else {
                editor.lsp.servers.insert(
                    language_id.clone(),
                    mae_core::LspServerInfo {
                        status: mae_core::LspServerStatus::Exited,
                        command: String::new(),
                        binary_found: false,
                    },
                );
            }
            editor.set_status(format!("[LSP] {} server exited", language_id));
            true
        }
        LspTaskEvent::DefinitionResult { uri, locations } => {
            mark_connected_from_uri(editor, &uri);
            // Check if this was a peek request rather than a jump.
            if editor.lsp.peek_definition_pending {
                editor.lsp.peek_definition_pending = false;
                if let Some(loc) = locations.first() {
                    let file_path = loc
                        .uri
                        .strip_prefix("file://")
                        .unwrap_or(&loc.uri)
                        .to_string();
                    let line = loc.range.start.line as usize;
                    let col = loc.range.start.character as usize;
                    editor.apply_peek_definition_result(file_path, line, col);
                } else {
                    editor.set_status("[LSP] no definition found");
                }
                return true;
            }
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
                open_location(editor, lsp_tx, other_file_loc);
            }
            true
        }
        LspTaskEvent::ReferencesResult { uri, locations } => {
            mark_connected_from_uri(editor, &uri);
            if editor.lsp.peek_references_pending {
                editor.lsp.peek_references_pending = false;
                // Build peek reference locations with context lines from open buffers.
                let peek_locs: Vec<mae_core::PeekReferenceLocation> = locations
                    .iter()
                    .map(|l| {
                        let path = l.uri.strip_prefix("file://").unwrap_or(&l.uri).to_string();
                        let line = l.range.start.line as usize;
                        let col = l.range.start.character as usize;
                        // Try to read context from open buffers.
                        let context: Vec<String> = editor
                            .buffers
                            .iter()
                            .find(|b| {
                                b.file_path()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .as_deref()
                                    == Some(path.as_str())
                            })
                            .map(|buf| {
                                let start = line.saturating_sub(3);
                                let end = (line + 4).min(buf.display_line_count());
                                (start..end).map(|i| buf.line_text(i)).collect()
                            })
                            .unwrap_or_default();
                        mae_core::PeekReferenceLocation {
                            path,
                            line,
                            col,
                            context,
                        }
                    })
                    .collect();
                if peek_locs.is_empty() {
                    editor.set_status("[LSP] no references found");
                } else {
                    let total = peek_locs.len();
                    editor.lsp.peek_references = Some(mae_core::PeekReferencesState {
                        locations: peek_locs,
                        current: 0,
                    });
                    editor.update_peek_references_preview();
                    editor.set_status(format!("[LSP] {} reference(s) — SPC l r n/p cycle", total));
                }
                return true;
            }
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
            true
        }
        LspTaskEvent::HoverResult { uri, contents, .. } => {
            mark_connected_from_uri(editor, &uri);
            // Only show hover if the active buffer still matches the request URI.
            // Prevents stale async responses from popping up after buffer switch/kill.
            let active_path = editor
                .active_buffer()
                .file_path()
                .map(|p| p.to_string_lossy().into_owned());
            let uri_path = uri.strip_prefix("file://").unwrap_or(&uri);
            let matches = active_path.as_deref() == Some(uri_path);
            if matches {
                editor.apply_hover_result(contents);
            }
            true
        }
        LspTaskEvent::DiagnosticsPublished { uri, diagnostics } => {
            // Receiving diagnostics proves the server is alive — transition
            // from Starting→Connected in case ServerStarted was missed.
            mark_connected_from_uri(editor, &uri);
            let count = diagnostics.len();
            let core_diags: Vec<CoreDiagnostic> =
                diagnostics.into_iter().map(lsp_diag_to_core).collect();
            let changed = editor.lsp.diagnostics.set(uri.clone(), core_diags);
            debug!(uri = %uri, count, "diagnostics published");
            if changed {
                let (e, w, _, _) = editor.lsp.diagnostics.severity_counts();
                if e + w > 0 {
                    editor.set_status(format!("[LSP] {} errors, {} warnings", e, w));
                }
            }
            // Always redraw when transitioning from Starting, even if diags unchanged.
            true
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
            false
        }
        LspTaskEvent::CompletionResult { uri, items, .. } => {
            mark_connected_from_uri(editor, &uri);
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
            true
        }
        // Workspace/document symbol results are only consumed by the deferred
        // AI tool flow (try_complete_deferred). If no deferred call is pending
        // they are silently dropped here — no redraw needed.
        LspTaskEvent::CodeActionResult { uri, actions } => {
            mark_connected_from_uri(editor, &uri);
            let items = lsp_actions_to_items(actions);
            editor.apply_code_action_result_items(items);
            true
        }
        LspTaskEvent::DocumentHighlightResult {
            highlights,
            generation,
        } => {
            use mae_core::HighlightKind as HK;
            let core_highlights: Vec<mae_core::DocumentHighlightRange> = highlights
                .into_iter()
                .map(|h| mae_core::DocumentHighlightRange {
                    start_line: h.range.start.line as usize,
                    start_col: h.range.start.character as usize,
                    end_line: h.range.end.line as usize,
                    end_col: h.range.end.character as usize,
                    kind: match h.kind {
                        mae_lsp::DocumentHighlightKind::Read => HK::Read,
                        mae_lsp::DocumentHighlightKind::Write => HK::Write,
                        mae_lsp::DocumentHighlightKind::Text => HK::Text,
                    },
                })
                .collect();
            editor.apply_document_highlight_result(core_highlights, generation);
            true
        }
        LspTaskEvent::SignatureHelpResult {
            signatures,
            active_signature,
            active_parameter,
        } => {
            let infos: Vec<mae_core::SignatureHelpInfo> = signatures
                .into_iter()
                .map(|s| mae_core::SignatureHelpInfo {
                    label: s.label,
                    parameters: s
                        .parameters
                        .iter()
                        .map(|p| (p.label_start, p.label_end))
                        .collect(),
                    documentation: s.documentation,
                })
                .collect();
            editor.apply_signature_help_result(infos, active_signature, active_parameter);
            true
        }
        LspTaskEvent::RenameResult { edits } => {
            // Convert WorkspaceEdit to the JSON format expected by show_rename_preview.
            let entries: Vec<(String, Vec<serde_json::Value>)> = edits
                .changes
                .iter()
                .map(|(uri, text_edits)| {
                    let edits_json: Vec<serde_json::Value> = text_edits
                        .iter()
                        .map(|e| {
                            serde_json::json!({
                                "start_line": e.range.start.line,
                                "start_character": e.range.start.character,
                                "end_line": e.range.end.line,
                                "end_character": e.range.end.character,
                                "new_text": e.new_text,
                            })
                        })
                        .collect();
                    (uri.clone(), edits_json)
                })
                .collect();
            let edits_json = serde_json::to_string(&entries).unwrap_or_default();
            // Count total edits for status message
            let total: usize = edits.changes.iter().map(|(_, e)| e.len()).sum();
            let file_count = edits.changes.len();
            editor.show_rename_preview(
                &edits_json,
                &format!("{} edits in {} files", total, file_count),
            );
            true
        }
        LspTaskEvent::FormatResult { uri, edits } => {
            if edits.is_empty() {
                editor.set_status("[LSP] no formatting changes");
                return true;
            }
            // Convert to the JSON format the core apply_workspace_edit_json expects.
            let edits_json: Vec<serde_json::Value> = edits
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "start_line": e.range.start.line,
                        "start_character": e.range.start.character,
                        "end_line": e.range.end.line,
                        "end_character": e.range.end.character,
                        "new_text": e.new_text,
                    })
                })
                .collect();
            let count = edits.len();
            let ws_edit =
                serde_json::to_string(&vec![(uri.clone(), edits_json)]).unwrap_or_default();
            editor.apply_format_edits_json(&ws_edit, count);
            true
        }
        LspTaskEvent::PrepareRenameResult { placeholder, .. } => {
            // Pre-fill the rename prompt with the placeholder text.
            if let Some(name) = placeholder {
                editor.set_mode(mae_core::Mode::Command);
                editor.vi.command_line = format!("lsp-rename {}", name);
                editor.vi.command_cursor = editor.vi.command_line.len();
                editor.set_status("Edit name and press Enter to rename");
            } else {
                editor.set_mode(mae_core::Mode::Command);
                editor.vi.command_line = "lsp-rename ".to_string();
                editor.vi.command_cursor = editor.vi.command_line.len();
                editor.set_status("Enter new name for symbol");
            }
            true
        }
        LspTaskEvent::TriggerCharacters {
            language_id,
            characters,
        } => {
            editor
                .lsp
                .trigger_characters
                .insert(language_id, characters);
            false
        }
        LspTaskEvent::WorkspaceSymbolResult { .. } => false,
        LspTaskEvent::DocumentSymbolResult { uri, symbols } => {
            mark_connected_from_uri(editor, &uri);
            // Flatten hierarchical DocumentSymbol tree into outline entries.
            fn flatten_symbols(
                symbols: &[mae_lsp::protocol::DocumentSymbol],
                depth: usize,
                out: &mut Vec<mae_core::SymbolOutlineEntry>,
            ) {
                for s in symbols {
                    let kind_label = s.kind.label();
                    let kind_icon = match s.kind {
                        mae_lsp::protocol::SymbolKind::Function
                        | mae_lsp::protocol::SymbolKind::Method
                        | mae_lsp::protocol::SymbolKind::Constructor => 'f',
                        mae_lsp::protocol::SymbolKind::Struct
                        | mae_lsp::protocol::SymbolKind::Class
                        | mae_lsp::protocol::SymbolKind::Interface => 's',
                        mae_lsp::protocol::SymbolKind::Enum => 'e',
                        mae_lsp::protocol::SymbolKind::Module
                        | mae_lsp::protocol::SymbolKind::Namespace
                        | mae_lsp::protocol::SymbolKind::Package => 'm',
                        mae_lsp::protocol::SymbolKind::Variable
                        | mae_lsp::protocol::SymbolKind::Constant => 'v',
                        mae_lsp::protocol::SymbolKind::Field
                        | mae_lsp::protocol::SymbolKind::Property => 'p',
                        mae_lsp::protocol::SymbolKind::TypeParameter => 't',
                        _ => ' ',
                    };
                    out.push(mae_core::SymbolOutlineEntry {
                        name: s.name.clone(),
                        kind: kind_label.to_string(),
                        kind_icon,
                        line: s.range.start.line as usize,
                        depth,
                        detail: s.detail.clone(),
                    });
                    flatten_symbols(&s.children, depth + 1, out);
                }
            }
            if editor.lsp.symbol_outline_pending {
                let mut entries = Vec::new();
                flatten_symbols(&symbols, 0, &mut entries);
                editor.apply_symbol_outline_result(&entries);
                true
            } else if editor.lsp.breadcrumb_symbols_pending {
                let mut entries = Vec::new();
                flatten_symbols(&symbols, 0, &mut entries);
                editor.apply_breadcrumb_symbols(&entries);
                true
            } else {
                false
            }
        }
        LspTaskEvent::Error { message } => {
            warn!(error = %message, "LSP error");
            editor.set_status(format!("[LSP] {}", message));
            true
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
        (_, LspTaskEvent::Error { message }) if kind.is_lsp() => Some(ToolResult {
            tool_call_id: tool_call_id.to_string(),
            tool_name: kind.tool_name().into(),
            success: false,
            output: format!("LSP error: {}", message),
        }),
        _ => None,
    }
}

/// Re-send `didOpen` for every open buffer whose language matches `language_id`.
/// Called after `ServerStarted` so the server knows about all currently open files.
fn resync_open_buffers(
    editor: &mut Editor,
    lsp_tx: &tokio::sync::mpsc::Sender<LspCommand>,
    language_id: &str,
) {
    for buf in &editor.buffers {
        let Some(path) = buf.file_path() else {
            continue;
        };
        if mae_core::lsp_intent::language_id_from_path(path).as_deref() != Some(language_id) {
            continue;
        }
        let uri = mae_core::lsp_intent::path_to_uri(path);
        let text = buf.text();
        let cmd = LspCommand::DidOpen {
            uri,
            language_id: language_id.to_string(),
            text,
        };
        if lsp_tx.try_send(cmd).is_err() {
            warn!("LSP resync: channel full, skipping buffer");
        }
    }
}

/// Strip `file://` prefix from a URI to get a filesystem path.
fn uri_to_path(uri: &str) -> Option<&str> {
    uri.strip_prefix("file://")
}

/// Mark the LSP server for the given URI's language as Connected.
/// Called when we get any successful response, as a fallback in case
/// `ServerStarted` was missed or the server was pre-started.
fn mark_connected_from_uri(editor: &mut Editor, uri: &str) {
    if let Some(path) = uri_to_path(uri) {
        if let Some(lang) = mae_core::lsp_intent::language_id_from_path(std::path::Path::new(path))
        {
            if let Some(info) = editor.lsp.servers.get_mut(&lang) {
                if info.status == mae_core::LspServerStatus::Starting {
                    info.status = mae_core::LspServerStatus::Connected;
                }
            }
        }
    }
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
            editor.display_buffer_and_focus(idx);
        }
        None => {
            // open_file queues a didOpen via file_ops
            editor.open_file(path);
        }
    }

    // Place the cursor.
    let idx = editor.active_buffer_idx();
    let line_count = editor.buffers[idx].display_line_count();
    let target_row = (loc.range.start_line as usize).min(line_count.saturating_sub(1));
    let target_col = loc.range.start_character as usize;
    let vh = editor.viewport_height;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = target_row;
    win.cursor_col = target_col;
    win.clamp_cursor(&editor.buffers[idx]);
    win.scroll_center(vh);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_connected_from_uri_transitions_starting() {
        let mut editor = Editor::new();
        editor.lsp.servers.insert(
            "rust".to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Starting,
                command: "rust-analyzer".into(),
                binary_found: true,
            },
        );
        assert_eq!(
            editor.lsp.servers["rust"].status,
            mae_core::LspServerStatus::Starting
        );
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.rs");
        assert_eq!(
            editor.lsp.servers["rust"].status,
            mae_core::LspServerStatus::Connected,
            "diagnostics/response should transition Starting → Connected"
        );
    }

    #[test]
    fn mark_connected_does_not_override_failed() {
        let mut editor = Editor::new();
        editor.lsp.servers.insert(
            "rust".to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Failed,
                command: "rust-analyzer".into(),
                binary_found: false,
            },
        );
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.rs");
        assert_eq!(
            editor.lsp.servers["rust"].status,
            mae_core::LspServerStatus::Failed,
            "should not override Failed status"
        );
    }

    #[test]
    fn mark_connected_unknown_language_is_noop() {
        let mut editor = Editor::new();
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.xyz");
        assert!(editor.lsp.servers.is_empty());
    }
}
