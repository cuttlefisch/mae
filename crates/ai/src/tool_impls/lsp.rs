//! LSP-related AI tool implementations.
//!
//! Exposes `lsp_diagnostics` (synchronous — reads the editor's diagnostic
//! store) plus three *deferred* tools that issue async LSP requests:
//!
//! - `lsp_definition` — textDocument/definition
//! - `lsp_references` — textDocument/references
//! - `lsp_hover` — textDocument/hover
//!
//! The deferred tools queue an `LspIntent` on the editor and return
//! `Ok(())`. The executor marks the tool call as `Deferred`, and the
//! main loop holds the AI reply channel until the matching
//! `LspTaskEvent` arrives.

use mae_core::lsp_intent::{language_id_from_path, path_to_uri, LspIntent};
use mae_core::{DiagnosticSeverity, Editor};
use serde_json::{json, Value};

use crate::tool_impls::resolve_buffer_idx;

/// Shape returned by `lsp_diagnostics`:
///
/// - `scope`: `"buffer"` or `"all"` (echoes the effective scope)
/// - `counts`: `{ error, warning, info, hint, total }`
/// - `files`: array of `{ uri, path, diagnostics: [...] }`
///
/// Each diagnostic entry: `{ line, col_start, col_end, end_line,
/// severity, message, source?, code? }`. Positions are 1-indexed for AI
/// consumption (matches `:diagnostics` output and editor line numbers).
pub fn execute_lsp_diagnostics(editor: &Editor, args: &Value) -> Result<String, String> {
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("buffer");

    // Resolve target buffer URI when scope == "buffer".
    let buffer_uri = if scope == "buffer" {
        let idx = resolve_buffer_idx(editor, args)?;
        editor.buffers[idx].file_path().map(mae_core::path_to_uri)
    } else {
        None
    };

    let mut files_json: Vec<Value> = Vec::new();
    let mut entries: Vec<(&String, &Vec<mae_core::Diagnostic>)> =
        editor.diagnostics.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    for (uri, diags) in entries {
        if scope == "buffer" {
            // Only include the active buffer's URI.
            match &buffer_uri {
                Some(u) if u == uri => {}
                _ => continue,
            }
        }
        let path = uri.strip_prefix("file://").unwrap_or(uri).to_string();
        let mut sorted = diags.clone();
        sorted.sort_by_key(|d| (d.line, d.col_start));
        let diag_list: Vec<Value> = sorted
            .iter()
            .map(|d| {
                let mut obj = json!({
                    "line": d.line + 1,
                    "col_start": d.col_start + 1,
                    "col_end": d.col_end + 1,
                    "end_line": d.end_line + 1,
                    "severity": severity_str(d.severity),
                    "message": d.message,
                });
                if let Some(src) = &d.source {
                    obj["source"] = json!(src);
                }
                if let Some(code) = &d.code {
                    obj["code"] = json!(code);
                }
                obj
            })
            .collect();
        files_json.push(json!({
            "uri": uri,
            "path": path,
            "diagnostics": diag_list,
        }));
    }

    // Global severity counts are always across the whole store, matching
    // `:diagnostics`. The file list above is already scope-filtered.
    let (e, w, i, h) = editor.diagnostics.severity_counts();
    let counts = json!({
        "error": e,
        "warning": w,
        "info": i,
        "hint": h,
        "total": e + w + i + h,
    });

    let effective_scope = if scope == "buffer" && buffer_uri.is_none() {
        // Buffer had no file path — nothing to scope to.
        "none"
    } else {
        scope
    };

    let out = json!({
        "scope": effective_scope,
        "counts": counts,
        "files": files_json,
    });
    Ok(out.to_string())
}

// ---------------------------------------------------------------------------
// Deferred LSP tools: queue an intent and return Ok(()) for deferred handling
// ---------------------------------------------------------------------------

/// Resolve LSP context for AI tools: buffer (by name or active), position
/// (from args or cursor). Returns (uri, language_id, line, character).
fn resolve_lsp_context(
    editor: &Editor,
    args: &Value,
) -> Result<(String, String, u32, u32), String> {
    let idx = resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];
    let path = buf
        .file_path()
        .ok_or("Buffer has no file path — LSP unavailable")?;
    let language_id =
        language_id_from_path(path).ok_or("No language server configured for this file type")?;
    let uri = path_to_uri(path);

    // Position: args override → cursor position of focused window
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|l| l.saturating_sub(1) as u32) // AI sends 1-indexed
        .unwrap_or_else(|| editor.window_mgr.focused_window().cursor_row as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|c| c.saturating_sub(1) as u32) // AI sends 1-indexed
        .unwrap_or_else(|| editor.window_mgr.focused_window().cursor_col as u32);

    Ok((uri, language_id, line, character))
}

/// Queue a `textDocument/definition` request for the AI.
pub fn execute_lsp_definition(editor: &mut Editor, args: &Value) -> Result<(), String> {
    let (uri, language_id, line, character) = resolve_lsp_context(editor, args)?;
    editor.pending_lsp_requests.push(LspIntent::GotoDefinition {
        uri,
        language_id,
        line,
        character,
    });
    Ok(())
}

/// Queue a `textDocument/references` request for the AI.
pub fn execute_lsp_references(editor: &mut Editor, args: &Value) -> Result<(), String> {
    let (uri, language_id, line, character) = resolve_lsp_context(editor, args)?;
    editor.pending_lsp_requests.push(LspIntent::FindReferences {
        uri,
        language_id,
        line,
        character,
        include_declaration: true,
    });
    Ok(())
}

/// Queue a `textDocument/hover` request for the AI.
pub fn execute_lsp_hover(editor: &mut Editor, args: &Value) -> Result<(), String> {
    let (uri, language_id, line, character) = resolve_lsp_context(editor, args)?;
    editor.pending_lsp_requests.push(LspIntent::Hover {
        uri,
        language_id,
        line,
        character,
    });
    Ok(())
}

/// Queue a `workspace/symbol` request for the AI.
pub fn execute_lsp_workspace_symbol(editor: &mut Editor, args: &Value) -> Result<(), String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'query' argument")?;
    let language_id = args
        .get("language_id")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'language_id' argument")?;
    editor
        .pending_lsp_requests
        .push(LspIntent::WorkspaceSymbol {
            language_id: language_id.to_string(),
            query: query.to_string(),
        });
    Ok(())
}

/// Queue a `textDocument/documentSymbol` request for the AI.
pub fn execute_lsp_document_symbols(editor: &mut Editor, args: &Value) -> Result<(), String> {
    let idx = resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];
    let path = buf
        .file_path()
        .ok_or("Buffer has no file path — LSP unavailable")?;
    let language_id =
        language_id_from_path(path).ok_or("No language server configured for this file type")?;
    let uri = path_to_uri(path);
    editor
        .pending_lsp_requests
        .push(LspIntent::DocumentSymbols { uri, language_id });
    Ok(())
}

fn severity_str(s: DiagnosticSeverity) -> &'static str {
    match s {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Information => "info",
        DiagnosticSeverity::Hint => "hint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::{Buffer, Diagnostic};
    use std::path::PathBuf;

    fn diag(line: u32, col: u32, sev: DiagnosticSeverity, msg: &str) -> Diagnostic {
        Diagnostic {
            line,
            col_start: col,
            col_end: col + 1,
            end_line: line,
            severity: sev,
            message: msg.into(),
            source: None,
            code: None,
        }
    }

    fn ed_with_file(path: &str) -> Editor {
        let mut b = Buffer::new();
        b.set_file_path(PathBuf::from(path));
        Editor::with_buffer(b)
    }

    #[test]
    fn diagnostics_empty_returns_valid_json() {
        let ed = Editor::new();
        let out = execute_lsp_diagnostics(&ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["counts"]["total"], 0);
        assert!(v["files"].as_array().unwrap().is_empty());
    }

    #[test]
    fn diagnostics_buffer_scope_filters_to_active() {
        let mut ed = ed_with_file("/tmp/a.rs");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        ed.diagnostics.set(
            "file:///tmp/other.rs".into(),
            vec![diag(1, 2, DiagnosticSeverity::Warning, "meh")],
        );
        let out = execute_lsp_diagnostics(&ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "/tmp/a.rs");
        assert_eq!(files[0]["diagnostics"][0]["message"], "bad");
        // Counts are global.
        assert_eq!(v["counts"]["total"], 2);
        assert_eq!(v["counts"]["error"], 1);
        assert_eq!(v["counts"]["warning"], 1);
    }

    #[test]
    fn diagnostics_all_scope_includes_every_file() {
        let mut ed = ed_with_file("/tmp/a.rs");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        ed.diagnostics.set(
            "file:///tmp/other.rs".into(),
            vec![diag(1, 2, DiagnosticSeverity::Hint, "nit")],
        );
        let out = execute_lsp_diagnostics(&ed, &json!({"scope": "all"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let files = v["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn diagnostics_positions_are_one_indexed() {
        let mut ed = ed_with_file("/tmp/a.rs");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(5, 7, DiagnosticSeverity::Error, "x")],
        );
        let out = execute_lsp_diagnostics(&ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let d = &v["files"][0]["diagnostics"][0];
        assert_eq!(d["line"], 6);
        assert_eq!(d["col_start"], 8);
    }

    #[test]
    fn diagnostics_buffer_without_file_returns_none_scope() {
        let mut ed = Editor::new();
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![diag(0, 0, DiagnosticSeverity::Error, "bad")],
        );
        // Active buffer is [scratch] — no file path.
        let out = execute_lsp_diagnostics(&ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["scope"], "none");
        assert!(v["files"].as_array().unwrap().is_empty());
    }

    #[test]
    fn diagnostics_preserves_source_and_code() {
        let mut ed = ed_with_file("/tmp/a.rs");
        ed.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![Diagnostic {
                line: 0,
                col_start: 0,
                col_end: 1,
                end_line: 0,
                severity: DiagnosticSeverity::Error,
                message: "unresolved".into(),
                source: Some("rustc".into()),
                code: Some("E0432".into()),
            }],
        );
        let out = execute_lsp_diagnostics(&ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let d = &v["files"][0]["diagnostics"][0];
        assert_eq!(d["source"], "rustc");
        assert_eq!(d["code"], "E0432");
        assert_eq!(d["severity"], "error");
    }

    // --- Deferred LSP tools ---

    #[test]
    fn lsp_definition_queues_intent() {
        let mut ed = ed_with_file("/tmp/a.rs");
        execute_lsp_definition(&mut ed, &json!({})).unwrap();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::GotoDefinition { .. }
        ));
    }

    #[test]
    fn lsp_definition_with_position_override() {
        let mut ed = ed_with_file("/tmp/a.rs");
        execute_lsp_definition(&mut ed, &json!({"line": 5, "character": 10})).unwrap();
        match &ed.pending_lsp_requests[0] {
            LspIntent::GotoDefinition {
                line, character, ..
            } => {
                assert_eq!(*line, 4); // 1-indexed → 0-indexed
                assert_eq!(*character, 9);
            }
            other => panic!("expected GotoDefinition, got {:?}", other),
        }
    }

    #[test]
    fn lsp_definition_errors_for_scratch_buffer() {
        let mut ed = Editor::new();
        let result = execute_lsp_definition(&mut ed, &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no file path"));
    }

    #[test]
    fn lsp_definition_errors_for_unknown_language() {
        let mut ed = ed_with_file("/tmp/a.xyz");
        let result = execute_lsp_definition(&mut ed, &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("language server"));
    }

    #[test]
    fn lsp_references_queues_intent() {
        let mut ed = ed_with_file("/tmp/a.rs");
        execute_lsp_references(&mut ed, &json!({})).unwrap();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::FindReferences { .. }
        ));
    }

    #[test]
    fn lsp_hover_queues_intent() {
        let mut ed = ed_with_file("/tmp/a.rs");
        execute_lsp_hover(&mut ed, &json!({})).unwrap();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        assert!(matches!(
            ed.pending_lsp_requests[0],
            LspIntent::Hover { .. }
        ));
    }

    #[test]
    fn lsp_definition_with_buffer_name() {
        let mut ed = ed_with_file("/tmp/a.rs");
        let mut b = Buffer::new();
        b.set_file_path(PathBuf::from("/tmp/b.py"));
        // set_file_path overrides name, so set it after
        b.name = "other".into();
        ed.buffers.push(b);

        execute_lsp_definition(&mut ed, &json!({"buffer_name": "other"})).unwrap();
        match &ed.pending_lsp_requests[0] {
            LspIntent::GotoDefinition { language_id, .. } => {
                assert_eq!(language_id, "python");
            }
            other => panic!("expected GotoDefinition, got {:?}", other),
        }
    }

    // --- Workspace symbol ---

    #[test]
    fn workspace_symbol_queues_intent() {
        let mut ed = Editor::new();
        execute_lsp_workspace_symbol(
            &mut ed,
            &json!({"query": "MyStruct", "language_id": "rust"}),
        )
        .unwrap();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        match &ed.pending_lsp_requests[0] {
            LspIntent::WorkspaceSymbol { query, language_id } => {
                assert_eq!(query, "MyStruct");
                assert_eq!(language_id, "rust");
            }
            other => panic!("expected WorkspaceSymbol, got {:?}", other),
        }
    }

    #[test]
    fn workspace_symbol_missing_query_errors() {
        let mut ed = Editor::new();
        let result = execute_lsp_workspace_symbol(&mut ed, &json!({"language_id": "rust"}));
        assert!(result.is_err());
    }

    #[test]
    fn workspace_symbol_missing_language_id_errors() {
        let mut ed = Editor::new();
        let result = execute_lsp_workspace_symbol(&mut ed, &json!({"query": "foo"}));
        assert!(result.is_err());
    }

    // --- Document symbols ---

    #[test]
    fn document_symbols_queues_intent() {
        let mut ed = ed_with_file("/tmp/a.rs");
        execute_lsp_document_symbols(&mut ed, &json!({})).unwrap();
        assert_eq!(ed.pending_lsp_requests.len(), 1);
        match &ed.pending_lsp_requests[0] {
            LspIntent::DocumentSymbols { uri, language_id } => {
                assert!(uri.contains("/tmp/a.rs"));
                assert_eq!(language_id, "rust");
            }
            other => panic!("expected DocumentSymbols, got {:?}", other),
        }
    }

    #[test]
    fn document_symbols_errors_for_scratch_buffer() {
        let mut ed = Editor::new();
        let result = execute_lsp_document_symbols(&mut ed, &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no file path"));
    }
}
