use mae_core::Editor;

use crate::tool_impls::{
    execute_help_open, execute_kb_get, execute_kb_graph, execute_kb_links_from,
    execute_kb_links_to, execute_kb_list, execute_kb_search,
};
use crate::types::ToolCall;

/// Dispatch knowledge base and help tools.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "kb_get" => execute_kb_get(editor, &call.arguments),
        "kb_search" => execute_kb_search(editor, &call.arguments),
        "kb_list" => execute_kb_list(editor, &call.arguments),
        "kb_links_from" => execute_kb_links_from(editor, &call.arguments),
        "kb_links_to" => execute_kb_links_to(editor, &call.arguments),
        "kb_graph" => execute_kb_graph(editor, &call.arguments),
        "help_open" => execute_help_open(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
