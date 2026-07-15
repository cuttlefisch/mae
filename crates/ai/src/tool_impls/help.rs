//! Help-buffer tool implementations.
//!
//! `help_open` returns help content invisibly for the agent's context
//! without opening a visible buffer. To show help to the user, suggest
//! `:help <topic>`.

use mae_core::Editor;

/// Return help content for the agent's context without any window changes.
/// The agent can read KB content and reason about it, but the user's layout
/// is never disrupted. To show the user help, suggest `:help <topic>`.
pub fn execute_help_open(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    // `kb_get_node_anywhere` checks the query layer first (when present)
    // and falls through to the in-memory KB when it misses — the single
    // shared existence/lookup used across crates so a query-layer
    // projection lagging behind the in-memory/CRDT truth (ADR-029) can
    // never make this tool report a real node as missing. This used to be
    // a third, independent copy of that same query-layer-then-in-memory
    // logic (missing the fallback), which is exactly how that bug slipped
    // through here even after being fixed in `mae-core`.
    let requested = editor.kb_get_node_anywhere(id);
    let (target, body, header) = match requested {
        Some(node) => (id.to_string(), node.body, format!("Help: {}\n\n", id)),
        None => {
            let body = editor
                .kb_get_node_anywhere("index")
                .map(|n| n.body)
                .unwrap_or_else(|| "Node not found.".to_string());
            (
                "index".to_string(),
                body,
                format!("No KB node '{}' -- showing 'index' instead.\n\n", id),
            )
        }
    };
    Ok(format!(
        "{}{}\n\n(Returned invisibly. To show the user, suggest `:help {}`)",
        header, body, target
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_open_returns_content_invisibly() {
        let mut editor = Editor::new();
        let initial_buf_count = editor.buffers.len();
        let initial_active = editor.active_buffer_idx();
        let result = execute_help_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(result.contains("index"));
        assert!(result.contains("Returned invisibly"));
        // No new buffer created, no window change.
        assert_eq!(editor.buffers.len(), initial_buf_count);
        assert_eq!(editor.active_buffer_idx(), initial_active);
    }

    #[test]
    fn help_open_concept_node() {
        let mut editor = Editor::new();
        let result =
            execute_help_open(&mut editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        assert!(result.contains("concept:buffer"));
        assert!(result.contains("Returned invisibly"));
    }

    #[test]
    fn help_open_missing_falls_back_to_index() {
        let mut editor = Editor::new();
        let result =
            execute_help_open(&mut editor, &serde_json::json!({"id": "no:such:node"})).unwrap();
        assert!(result.contains("No KB node"));
        assert!(result.contains("index"));
    }

    #[test]
    fn help_open_missing_id_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_help_open(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("id"));
    }

    #[test]
    fn help_open_finds_a_node_via_in_memory_fallback_when_the_query_layer_lags_behind() {
        // Direct regression test for the reported bug at the exact layer
        // the user's MCP client calls: an empty query layer (simulating a
        // CozoDB projection — ADR-029 — that hasn't caught up to the
        // in-memory KB yet) must not make `help_open` report a real node
        // as missing. This was a THIRD, independent copy of the same
        // query-layer-then-in-memory fallback logic (missing the
        // fallback) — fixed once in `mae-core`'s `kb_contains_any` and
        // again here, both now routed through the shared
        // `Editor::kb_get_node_anywhere`.
        use std::sync::Arc;

        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "scheme:gc-collect!",
            "Scheme: gc-collect!",
            mae_kb::NodeKind::Concept,
            "body text",
        ));
        let empty_layer: Arc<dyn mae_kb::query::KbQueryLayer> = Arc::new(
            mae_kb::query::InMemoryQueryLayer::new(mae_kb::KnowledgeBase::new()),
        );
        editor.kb.set_daemon_query_layer(Some(empty_layer));
        assert!(!editor
            .kb
            .query_layer()
            .unwrap()
            .contains("scheme:gc-collect!"));

        let result = execute_help_open(
            &mut editor,
            &serde_json::json!({"id": "scheme:gc-collect!"}),
        )
        .unwrap();

        assert!(
            !result.contains("No KB node"),
            "must not report a real node as missing: {result}"
        );
        assert!(result.contains("Help: scheme:gc-collect!"));
        assert!(result.contains("body text"));
    }
}
