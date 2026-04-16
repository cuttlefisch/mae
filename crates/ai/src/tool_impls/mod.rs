mod buffer;
mod editor_tools;
mod file;
mod project;

pub use buffer::{
    execute_buffer_read, execute_buffer_write, execute_cursor_info, execute_file_read,
    execute_list_buffers,
};
pub use editor_tools::{
    execute_command_list, execute_debug_state, execute_editor_state, execute_window_layout,
};
pub use file::{
    execute_close_buffer, execute_create_file, execute_open_file, execute_switch_buffer,
};
pub use project::{execute_project_files, execute_project_search};

use mae_core::Editor;

/// Resolve a buffer reference: if `buffer_name` is provided, find that buffer;
/// otherwise return the active buffer index.
pub fn resolve_buffer_idx(editor: &Editor, args: &serde_json::Value) -> Result<usize, String> {
    if let Some(name) = args.get("buffer_name").and_then(|v| v.as_str()) {
        editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))
    } else {
        Ok(editor.active_buffer_idx())
    }
}
