use mae_core::Editor;

use crate::tool_impls::{
    execute_ai_load, execute_ai_save, execute_create_plan, execute_event_recording,
    execute_introspect, execute_mouse_event, execute_org_cycle, execute_org_open_link,
    execute_org_todo_cycle, execute_render_inspect, execute_save_memory, execute_theme_inspect,
    execute_trigger_hook, execute_update_plan, execute_visual_buffer_add_circle,
    execute_visual_buffer_add_line, execute_visual_buffer_add_rect, execute_visual_buffer_add_text,
    execute_visual_buffer_clear,
};
use crate::types::ToolCall;

/// Dispatch AI-specific, visual, org, and introspection tools.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "ai_save" => execute_ai_save(editor, &call.arguments),
        "ai_load" => execute_ai_load(editor, &call.arguments),
        "save_memory" => execute_save_memory(&call.arguments),
        "create_plan" => execute_create_plan(&call.arguments),
        "update_plan" => execute_update_plan(&call.arguments),
        "mouse_event" => execute_mouse_event(editor, &call.arguments),
        "render_inspect" => execute_render_inspect(editor, &call.arguments),
        "introspect" => execute_introspect(editor, &call.arguments),
        "trigger_hook" => execute_trigger_hook(editor, &call.arguments),
        "theme_inspect" => execute_theme_inspect(editor, &call.arguments),
        "visual_buffer_add_rect" => execute_visual_buffer_add_rect(editor, &call.arguments),
        "visual_buffer_add_line" => execute_visual_buffer_add_line(editor, &call.arguments),
        "visual_buffer_add_circle" => execute_visual_buffer_add_circle(editor, &call.arguments),
        "visual_buffer_add_text" => execute_visual_buffer_add_text(editor, &call.arguments),
        "visual_buffer_clear" => execute_visual_buffer_clear(editor),
        "org_cycle" => execute_org_cycle(editor),
        "org_todo_cycle" => execute_org_todo_cycle(editor, &call.arguments),
        "org_open_link" => execute_org_open_link(editor),
        "event_recording" => execute_event_recording(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
