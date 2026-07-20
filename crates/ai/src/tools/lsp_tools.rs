use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// LSP tool definitions: definition, references, hover, diagnostics, symbols, syntax tree.
pub(super) fn lsp_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefBuilder::new(
            "lsp_definition",
            "Go to definition of the symbol at the given position. Returns JSON with locations (uri, line, character). Requires an LSP server for the file's language. Positions are 1-indexed.",
        )
        .prop("line", "integer", "1-indexed line number (default: cursor line)")
        .prop("character", "integer", "1-indexed column (default: cursor column)")
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_references",
            "Find all references to the symbol at the given position. Returns JSON array of locations. Positions are 1-indexed.",
        )
        .prop("line", "integer", "1-indexed line number (default: cursor line)")
        .prop("character", "integer", "1-indexed column (default: cursor column)")
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_hover",
            "Get hover information (type signature, docs) for the symbol at the given position. Returns rendered markdown. Positions are 1-indexed.",
        )
        .prop("line", "integer", "1-indexed line number (default: cursor line)")
        .prop("character", "integer", "1-indexed column (default: cursor column)")
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_diagnostics",
            "Get LSP diagnostics (errors, warnings) for a buffer. Returns JSON array of {line, character, severity, message, source}. Lines are 1-indexed.",
        )
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_workspace_symbol",
            "Search for symbols across the workspace by name. Returns JSON array of {name, kind, location}.",
        )
        .prop("query", "string", "Symbol name or prefix to search for")
        .required(["query"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_document_symbols",
            "List all symbols in a document. Returns hierarchical JSON of {name, kind, range, children}.",
        )
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "syntax_tree",
            "Return the tree-sitter syntax tree for a buffer. Useful for understanding code structure before editing. `scope='buffer'` returns the full root S-expression; `scope='cursor'` returns only the named-node kind at the current cursor position.",
        )
        .prop_enum(
            "scope",
            "string",
            "'buffer' (default) returns the full tree; 'cursor' returns the node at the cursor.",
            ["buffer", "cursor"],
        )
        .prop("buffer_name", "string", "Override the active buffer by name.")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "lsp_rename",
            "Rename the symbol at the given position across the workspace. Requires an LSP server. Positions are 1-indexed.",
        )
        .prop("new_name", "string", "The new name for the symbol")
        .prop("line", "integer", "1-indexed line number (default: cursor line)")
        .prop("character", "integer", "1-indexed column (default: cursor column)")
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .required(["new_name"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "lsp_format",
            "Format a buffer (or a range of lines) via LSP. Without start_line/end_line, formats the entire buffer. Positions are 1-indexed.",
        )
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .prop("start_line", "integer", "1-indexed start line for range format (optional)")
        .prop("end_line", "integer", "1-indexed end line for range format (optional)")
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "lsp_code_action",
            "List available code actions at the given position. Returns JSON array of {title, kind}. Use with an index parameter to apply a specific action.",
        )
        .prop("line", "integer", "1-indexed line number (default: cursor line)")
        .prop("character", "integer", "1-indexed column (default: cursor column)")
        .prop("buffer_name", "string", "Buffer name (default: active buffer)")
        .prop(
            "apply_index",
            "integer",
            "0-indexed action to apply immediately (optional — omit to list actions)",
        )
        .permission(PermissionTier::Write)
        .build(),
    ]
}
