use std::collections::HashMap;

use crate::types::*;

/// LSP tool definitions: definition, references, hover, diagnostics, symbols, syntax tree.
pub(super) fn lsp_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "lsp_definition".into(),
            description: "Go to definition of the symbol at the given position. Returns JSON with locations (uri, line, character). Requires an LSP server for the file's language. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_references".into(),
            description: "Find all references to the symbol at the given position. Returns JSON array of locations. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_hover".into(),
            description: "Get hover information (type signature, docs) for the symbol at the given position. Returns rendered markdown. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_diagnostics".into(),
            description: "Get LSP diagnostics (errors, warnings) for a buffer. Returns JSON array of {line, character, severity, message, source}. Lines are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "buffer_name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Buffer name (default: active buffer)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_workspace_symbol".into(),
            description: "Search for symbols across the workspace by name. Returns JSON array of {name, kind, location}.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "query".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Symbol name or prefix to search for".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["query".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_document_symbols".into(),
            description: "List all symbols in a document. Returns hierarchical JSON of {name, kind, range, children}.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "buffer_name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Buffer name (default: active buffer)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "syntax_tree".into(),
            description: "Return the tree-sitter syntax tree for a buffer. Useful for understanding code structure before editing. `scope='buffer'` returns the full root S-expression; `scope='cursor'` returns only the named-node kind at the current cursor position.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "'buffer' (default) returns the full tree; 'cursor' returns the node at the cursor.".into(),
                            enum_values: Some(vec!["buffer".into(), "cursor".into()]),
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Override the active buffer by name.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
    ]
}
