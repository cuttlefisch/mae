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
    if let Some(root_uri) = editor.pending_lsp_root_change.take() {
        let _ = lsp_tx.try_send(LspCommand::DidChangeWorkspaceFolders {
            added: vec![root_uri],
        });
    }

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
        // Stubs: these intents are queued but the LSP client doesn't
        // handle them yet.
        LspIntent::Rename { .. } | LspIntent::Format { .. } => LspCommand::DidClose {
            uri: String::new(),
            language_id: String::new(),
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
            if !editor.lsp_servers.contains_key(&language_id) {
                editor.lsp_servers.insert(
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
            if let Some(info) = editor.lsp_servers.get_mut(&language_id) {
                info.status = mae_core::LspServerStatus::Failed;
            } else {
                editor.lsp_servers.insert(
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
            if let Some(info) = editor.lsp_servers.get_mut(&language_id) {
                info.status = mae_core::LspServerStatus::Exited;
            } else {
                editor.lsp_servers.insert(
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
            editor.apply_hover_result(contents);
            true
        }
        LspTaskEvent::DiagnosticsPublished { uri, diagnostics } => {
            // Receiving diagnostics proves the server is alive — transition
            // from Starting→Connected in case ServerStarted was missed.
            mark_connected_from_uri(editor, &uri);
            let count = diagnostics.len();
            let core_diags: Vec<CoreDiagnostic> =
                diagnostics.into_iter().map(lsp_diag_to_core).collect();
            let changed = editor.diagnostics.set(uri.clone(), core_diags);
            debug!(uri = %uri, count, "diagnostics published");
            if changed {
                let (e, w, _, _) = editor.diagnostics.severity_counts();
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
        LspTaskEvent::WorkspaceSymbolResult { .. } => false,
        LspTaskEvent::DocumentSymbolResult { .. } => false,
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
            if let Some(info) = editor.lsp_servers.get_mut(&lang) {
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
            editor.switch_to_buffer(idx);
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
        editor.lsp_servers.insert(
            "rust".to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Starting,
                command: "rust-analyzer".into(),
                binary_found: true,
            },
        );
        assert_eq!(
            editor.lsp_servers["rust"].status,
            mae_core::LspServerStatus::Starting
        );
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.rs");
        assert_eq!(
            editor.lsp_servers["rust"].status,
            mae_core::LspServerStatus::Connected,
            "diagnostics/response should transition Starting → Connected"
        );
    }

    #[test]
    fn mark_connected_does_not_override_failed() {
        let mut editor = Editor::new();
        editor.lsp_servers.insert(
            "rust".to_string(),
            mae_core::LspServerInfo {
                status: mae_core::LspServerStatus::Failed,
                command: "rust-analyzer".into(),
                binary_found: false,
            },
        );
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.rs");
        assert_eq!(
            editor.lsp_servers["rust"].status,
            mae_core::LspServerStatus::Failed,
            "should not override Failed status"
        );
    }

    #[test]
    fn mark_connected_unknown_language_is_noop() {
        let mut editor = Editor::new();
        mark_connected_from_uri(&mut editor, "file:///tmp/foo.xyz");
        assert!(editor.lsp_servers.is_empty());
    }
}
