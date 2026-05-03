use std::collections::HashMap;

use crate::types::*;

/// Knowledge base tool definitions: get, search, list, links, graph, help, org.
pub(super) fn kb_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // ---- Knowledge base (shared with :help) ----
        //
        // The KB is the source of truth for command/concept/key
        // documentation. The same nodes the human reads via `:help`
        // are queryable here — the agent is a peer reader.
        ToolDefinition {
            name: "kb_get".into(),
            description: "Fetch a knowledge-base node by id. Returns JSON with title, kind, body (may contain [[link]] markers), tags, outgoing links, and incoming backlinks. IDs are namespaced like 'cmd:<name>', 'concept:<slug>', 'key:<context>', or 'index'. WARNING: Linkage is high; pull atomic info and avoid walking the entire graph.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Node id, e.g. 'index', 'concept:buffer', 'cmd:save'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_search".into(),
            description: "Case-insensitive substring search over KB node titles, ids, bodies, and tags. Returns ids in relevance order (title/id matches before body matches). Empty query returns all ids.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "query".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Substring to search for (case-insensitive)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_list".into(),
            description: "List all KB node ids, sorted. Optional `prefix` filters to a namespace (e.g. prefix='cmd:' returns all command docs).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "prefix".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional namespace prefix, e.g. 'cmd:', 'concept:'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_links_from".into(),
            description: "Outgoing links from a node — the targets of its body's [[link]] markers, in document order (deduplicated). Errors if the node doesn't exist.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Source node id".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_links_to".into(),
            description: "Incoming links — ids of all KB nodes whose body references this target. Works even if the target node doesn't exist yet (dangling backlinks).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Target node id (may be dangling)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph".into(),
            description: "BFS neighborhood around a seed node up to `depth` hops (default 1, max 3). Returns {root, depth, nodes: [{id, title, kind, hop, missing?}], edges: [{src, dst}]}. Use this to orient yourself in the KB before navigating — the local graph tells you which related topics the user might want to explore next.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Seed node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "depth".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Hop radius (default 1, clamped to 3)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "help_open".into(),
            description: "Returns help content for the agent's context without opening a visible buffer. Use this to look up KB documentation for your own reasoning. To show help to the user, suggest they run `:help <topic>`. Falls back to the `index` node if the id isn't found.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Node id to open, e.g. 'index', 'concept:buffer', 'cmd:save'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Org tools ---
        ToolDefinition {
            name: "org_cycle".into(),
            description: "Toggle visibility (folding) of the Org heading at the cursor.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "org_todo_cycle".into(),
            description: "Cycle the TODO state of the Org heading at the cursor.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "forward".into(),
                    ToolProperty {
                        prop_type: "boolean".into(),
                        description: "true to cycle forward (TODO->DONE), false for backward".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "org_open_link".into(),
            description: "Open the Org link under the cursor (internal jump or external URL).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
    ]
}
