mod buffer;
mod dap;
mod editor_tools;
mod file;
mod git;
mod help;
mod introspect;
mod kb;
pub(crate) mod lsp;
mod project;
mod shell;
mod syntax;

pub use buffer::{
    execute_buffer_read, execute_buffer_write, execute_cursor_info, execute_file_read,
    execute_list_buffers,
};
pub use dap::{
    execute_dap_continue, execute_dap_disconnect, execute_dap_evaluate,
    execute_dap_expand_variable, execute_dap_inspect_variable, execute_dap_list_variables,
    execute_dap_output, execute_dap_remove_breakpoint, execute_dap_select_frame,
    execute_dap_select_thread, execute_dap_set_breakpoint, execute_dap_start, execute_dap_step,
};
pub use editor_tools::{
    execute_command_list, execute_debug_state, execute_editor_state, execute_event_recording,
    execute_get_option, execute_mouse_event, execute_org_cycle, execute_org_open_link,
    execute_org_todo_cycle, execute_render_inspect, execute_set_option, execute_shell_scrollback,
    execute_theme_inspect, execute_trigger_hook, execute_visual_buffer_add_circle,
    execute_visual_buffer_add_line, execute_visual_buffer_add_rect, execute_visual_buffer_add_text,
    execute_visual_buffer_clear, execute_window_layout,
};
pub use file::{
    execute_ai_load, execute_ai_save, execute_close_buffer, execute_create_file, execute_open_file,
    execute_rename_file, execute_switch_buffer,
};
pub use git::{
    execute_git_checkout, execute_git_commit, execute_git_diff, execute_git_log, execute_git_pull,
    execute_git_push, execute_git_stage, execute_git_status, execute_git_unstage,
};
pub use help::execute_help_open;
pub use introspect::execute_introspect;
pub use kb::{
    execute_kb_get, execute_kb_graph, execute_kb_links_from, execute_kb_links_to, execute_kb_list,
    execute_kb_search,
};
pub use lsp::execute_lsp_diagnostics;
pub use project::{
    execute_create_plan, execute_project_files, execute_project_info, execute_project_search,
    execute_save_memory, execute_switch_project, execute_update_plan,
};
pub use shell::{execute_shell_list, execute_shell_read_output, execute_shell_send_input};
pub use syntax::execute_syntax_tree;

use mae_core::Editor;

/// Resolve a buffer reference: if `buffer_name` is provided, find that buffer;
/// otherwise return the AI target buffer index (if set) or the active buffer index.
pub fn resolve_buffer_idx(editor: &Editor, args: &serde_json::Value) -> Result<usize, String> {
    if let Some(name) = args.get("buffer_name").and_then(|v| v.as_str()) {
        editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))
    } else {
        Ok(editor
            .ai_target_buffer_idx
            .unwrap_or_else(|| editor.active_buffer_idx()))
    }
}
