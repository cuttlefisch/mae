use mae_core::Editor;

use crate::tool_impls::{
    execute_dap_continue, execute_dap_disconnect, execute_dap_evaluate,
    execute_dap_expand_variable, execute_dap_inspect_variable, execute_dap_list_variables,
    execute_dap_output, execute_dap_remove_breakpoint, execute_dap_select_frame,
    execute_dap_select_thread, execute_dap_set_breakpoint, execute_dap_start, execute_dap_step,
};
use crate::types::ToolCall;

/// Dispatch DAP (Debug Adapter Protocol) tools.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "dap_start" => execute_dap_start(editor, &call.arguments),
        "dap_set_breakpoint" => execute_dap_set_breakpoint(editor, &call.arguments),
        "dap_continue" => execute_dap_continue(editor),
        "dap_step" => execute_dap_step(editor, &call.arguments),
        "dap_inspect_variable" => execute_dap_inspect_variable(editor, &call.arguments),
        "dap_remove_breakpoint" => execute_dap_remove_breakpoint(editor, &call.arguments),
        "dap_list_variables" => execute_dap_list_variables(editor),
        "dap_expand_variable" => execute_dap_expand_variable(editor, &call.arguments),
        "dap_select_frame" => execute_dap_select_frame(editor, &call.arguments),
        "dap_select_thread" => execute_dap_select_thread(editor, &call.arguments),
        "dap_output" => execute_dap_output(editor, &call.arguments),
        "dap_evaluate" => execute_dap_evaluate(editor, &call.arguments),
        "dap_disconnect" => execute_dap_disconnect(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
