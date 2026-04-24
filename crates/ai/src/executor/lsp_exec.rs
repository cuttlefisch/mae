use mae_core::Editor;

use crate::tool_impls::execute_lsp_diagnostics;
use crate::types::ToolCall;

/// Dispatch synchronous LSP tools (diagnostics).
/// Deferred LSP tools (definition, references, hover, symbols) are handled
/// directly in `execute_tool()` in mod.rs before reaching this dispatcher.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "lsp_diagnostics" => execute_lsp_diagnostics(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
