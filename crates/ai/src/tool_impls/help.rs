//! Help-buffer tool implementations.
//!
//! `help_open` mirrors the human `:help <node>` command: it opens the
//! *Help* buffer on a KB node so the user can see what the agent is
//! referencing. Because the human and agent read the same KB, the agent
//! pointing the user at a help page is a cheap, high-signal UX move.

use mae_core::Editor;

/// Open the *Help* buffer focused on a KB node. If the node id is missing,
/// the editor falls back to the `index` node and records a status message
/// — we surface that fallback in the tool result so the agent knows.
pub fn execute_help_open(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: id".to_string())?;
    let existed = editor.kb.contains(id);
    editor.open_help_at(id);
    let view = editor
        .help_view()
        .ok_or_else(|| "Failed to open help buffer".to_string())?;
    let msg = if existed {
        format!("Opened help buffer on '{}'", view.current)
    } else {
        format!(
            "No KB node '{}' — opened help buffer on '{}' instead",
            id, view.current
        )
    };
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::buffer::BufferKind;

    #[test]
    fn help_open_creates_help_buffer() {
        let mut editor = Editor::new();
        let result = execute_help_open(&mut editor, &serde_json::json!({"id": "index"})).unwrap();
        assert!(result.contains("index"));
        assert_eq!(editor.active_buffer().kind, BufferKind::Help);
        assert_eq!(editor.help_view().unwrap().current, "index");
    }

    #[test]
    fn help_open_concept_node() {
        let mut editor = Editor::new();
        let result =
            execute_help_open(&mut editor, &serde_json::json!({"id": "concept:buffer"})).unwrap();
        assert!(result.contains("concept:buffer"));
        assert_eq!(editor.help_view().unwrap().current, "concept:buffer");
    }

    #[test]
    fn help_open_missing_falls_back_to_index() {
        let mut editor = Editor::new();
        let result =
            execute_help_open(&mut editor, &serde_json::json!({"id": "no:such:node"})).unwrap();
        assert!(result.contains("No KB node"));
        assert!(result.contains("index"));
        assert_eq!(editor.help_view().unwrap().current, "index");
    }

    #[test]
    fn help_open_missing_id_arg_is_error() {
        let mut editor = Editor::new();
        let err = execute_help_open(&mut editor, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("id"));
    }
}
