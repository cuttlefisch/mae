//! Syntax-tree AI tool.
//!
//! Lets the AI inspect the tree-sitter parse tree of a buffer. Uses the
//! editor's cached `SyntaxMap` — parses only on cache miss.
//!
//! - `scope="buffer"` (default): returns the full root S-expression.
//! - `scope="cursor"`: returns only the named node kind at the cursor.

use mae_core::Editor;
use serde_json::{json, Value};

use crate::tool_impls::resolve_buffer_idx;

pub fn execute_syntax_tree(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("buffer");

    let buf_idx = resolve_buffer_idx(editor, args)?;

    // Make sure the target buffer is the active one so the helpers read the
    // right buffer — saving/restoring is cheap and keeps the helper signature
    // simple. (Both helpers read from `active_buffer_idx`.)
    let original_idx = editor.active_buffer_idx();
    let restore_needed = buf_idx != original_idx;
    if restore_needed {
        editor.window_mgr.focused_window_mut().buffer_idx = buf_idx;
    }

    let language = editor.syntax.language_of(buf_idx);
    let result = match (scope, language) {
        (_, None) => Err("Buffer has no associated tree-sitter language".to_string()),
        ("buffer", Some(lang)) => match editor.syntax_tree_sexp() {
            Some(sexp) => Ok(json!({
                "scope": "buffer",
                "language": lang.id(),
                "buffer_name": editor.buffers[buf_idx].name.clone(),
                "sexp": sexp,
            })
            .to_string()),
            None => Err("Failed to parse buffer".to_string()),
        },
        ("cursor", Some(lang)) => match editor.syntax_node_kind_at_cursor() {
            Some(kind) => Ok(json!({
                "scope": "cursor",
                "language": lang.id(),
                "buffer_name": editor.buffers[buf_idx].name.clone(),
                "node_kind": kind,
            })
            .to_string()),
            None => Err("No node at cursor".to_string()),
        },
        (other, _) => Err(format!(
            "Unknown scope '{}': expected 'buffer' or 'cursor'",
            other
        )),
    };

    if restore_needed {
        editor.window_mgr.focused_window_mut().buffer_idx = original_idx;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::Buffer;
    use std::path::PathBuf;

    fn ed_with_rust_source(src: &str) -> Editor {
        let mut b = Buffer::new();
        b.set_file_path(PathBuf::from("/tmp/example.rs"));
        let mut ed = Editor::with_buffer(b);
        for ch in src.chars() {
            let win = ed.window_mgr.focused_window_mut();
            ed.buffers[0].insert_char(win, ch);
        }
        // Edits via insert_char bypass record_edit, so make sure the syntax
        // cache is fresh.
        ed.syntax.invalidate(0);
        ed
    }

    #[test]
    fn syntax_tree_buffer_scope_returns_sexp() {
        let mut ed = ed_with_rust_source("fn main() {}");
        let out = execute_syntax_tree(&mut ed, &json!({})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["scope"], "buffer");
        assert_eq!(v["language"], "rust");
        let sexp = v["sexp"].as_str().unwrap();
        assert!(sexp.contains("function_item"), "sexp was: {}", sexp);
    }

    #[test]
    fn syntax_tree_cursor_scope_returns_node_kind() {
        let mut ed = ed_with_rust_source("fn main() {}");
        // Cursor starts at (0,0) — on the 'fn' keyword.
        let out =
            execute_syntax_tree(&mut ed, &json!({"scope": "cursor"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["scope"], "cursor");
        assert!(v["node_kind"].is_string());
    }

    #[test]
    fn syntax_tree_without_language_returns_error() {
        let mut ed = Editor::new();
        // Scratch buffer — no language attached.
        let err = execute_syntax_tree(&mut ed, &json!({})).unwrap_err();
        assert!(
            err.contains("no associated tree-sitter language"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn syntax_tree_bad_scope_returns_error() {
        let mut ed = ed_with_rust_source("fn main() {}");
        let err = execute_syntax_tree(&mut ed, &json!({"scope": "bogus"}))
            .unwrap_err();
        assert!(err.contains("Unknown scope"));
    }
}
