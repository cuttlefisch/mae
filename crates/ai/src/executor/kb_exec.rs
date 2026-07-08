use mae_core::Editor;

use crate::tool_impls::kb::record_kb_visit;
use crate::tool_impls::{
    execute_help_open, execute_kb_add_link, execute_kb_agenda, execute_kb_create,
    execute_kb_delete, execute_kb_get, execute_kb_graph, execute_kb_health, execute_kb_history,
    execute_kb_id_audit, execute_kb_links_from, execute_kb_links_to, execute_kb_list,
    execute_kb_neighborhood, execute_kb_raw_query, execute_kb_register, execute_kb_reimport,
    execute_kb_related, execute_kb_restore, execute_kb_search, execute_kb_search_context,
    execute_kb_shortest_path, execute_kb_sync_status, execute_kb_unregister, execute_kb_update,
    execute_kb_vector_search, execute_kb_view_query,
};
use crate::types::ToolCall;

/// Dispatch knowledge base and help tools.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
/// Records visited IDs for kb_get/links_from/links_to to detect manual
/// graph traversal loops and steer the AI toward kb_graph.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "kb_get" => {
            let r = execute_kb_get(editor, &call.arguments);
            if let Some(id) = call.arguments.get("id").and_then(|v| v.as_str()) {
                record_kb_visit(editor, id);
            }
            r
        }
        "kb_search" => execute_kb_search(editor, &call.arguments),
        "kb_list" => execute_kb_list(editor, &call.arguments),
        "kb_links_from" => {
            let r = execute_kb_links_from(editor, &call.arguments);
            if let Some(id) = call.arguments.get("id").and_then(|v| v.as_str()) {
                record_kb_visit(editor, id);
            }
            r
        }
        "kb_links_to" => {
            let r = execute_kb_links_to(editor, &call.arguments);
            if let Some(id) = call.arguments.get("id").and_then(|v| v.as_str()) {
                record_kb_visit(editor, id);
            }
            r
        }
        "kb_graph" => execute_kb_graph(editor, &call.arguments),
        "kb_related" => {
            let r = execute_kb_related(editor, &call.arguments);
            if let Some(id) = call.arguments.get("id").and_then(|v| v.as_str()) {
                record_kb_visit(editor, id);
            }
            r
        }
        "kb_health" => execute_kb_health(editor),
        "kb_id_audit" => execute_kb_id_audit(editor),
        "kb_sync_status" => execute_kb_sync_status(editor),
        "kb_create" => execute_kb_create(editor, &call.arguments),
        "kb_update" => execute_kb_update(editor, &call.arguments),
        "kb_delete" => execute_kb_delete(editor, &call.arguments),
        "kb_register" => execute_kb_register(editor, &call.arguments),
        "kb_unregister" => execute_kb_unregister(editor, &call.arguments),
        "kb_reimport" => execute_kb_reimport(editor, &call.arguments),
        "kb_search_context" => execute_kb_search_context(editor, &call.arguments),
        "kb_shortest_path" => execute_kb_shortest_path(editor, &call.arguments),
        "kb_neighborhood" => execute_kb_neighborhood(editor, &call.arguments),
        "kb_add_link" => execute_kb_add_link(editor, &call.arguments),
        "kb_raw_query" => execute_kb_raw_query(editor, &call.arguments),
        "kb_agenda" => execute_kb_agenda(editor, &call.arguments),
        "kb_history" => execute_kb_history(editor, &call.arguments),
        "kb_restore" => execute_kb_restore(editor, &call.arguments),
        "kb_view_query" => execute_kb_view_query(editor, &call.arguments),
        "kb_vector_search" => execute_kb_vector_search(editor, &call.arguments),
        "help_open" => execute_help_open(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
