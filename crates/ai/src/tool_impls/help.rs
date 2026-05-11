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
    let target = if editor.kb.contains(id) {
        id.to_string()
    } else {
        "index".to_string()
    };
    let content = editor
        .kb
        .get(&target)
        .map(|node| node.body.clone())
        .unwrap_or_else(|| "Node not found.".to_string());
    let header = if editor.kb.contains(id) {
        format!("Help: {}\n\n", target)
    } else {
        format!("No KB node '{}' -- showing 'index' instead.\n\n", id)
    };
    Ok(format!(
        "{}{}\n\n(Returned invisibly. To show the user, suggest `:help {}`)",
        header, content, target
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
}
