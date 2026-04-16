//! LSP-related AI tool implementations.
//!
//! Currently exposes `lsp_diagnostics`, which returns a structured JSON
//! view of the editor's diagnostic store. This is the biggest feedback
//! loop the AI has when editing code: "what errors did the language
//! server report?"
//!
//! The dynamic LSP requests (definition / references / hover) are
//! already reachable via the registry commands `command_lsp_goto_definition`,
//! `command_lsp_find_references`, `command_lsp_hover` — their *results*
//! currently land in the status bar. Promoting those to structured tool
//! output requires a request/response round-trip through the async LSP
//! task, which will land in a follow-up.

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
}
