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
            description: "Search all knowledge base nodes (MAE manual + user + federated). Case-insensitive over titles, ids, bodies, tags, and aliases. Returns ids in relevance order. Falls back to fuzzy scoring when no substring matches are found. Empty query returns all ids.".into(),
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
            description: "Look up MAE manual content for your own reasoning (searches builtin nodes first, falls back to user KB). Does not open a visible buffer. To show help to the user, suggest `:help <topic>`. Falls back to the `index` node if the id isn't found.".into(),
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
        // --- Babel tools ---
        ToolDefinition {
            name: "babel_execute".into(),
            description: "Execute the org-babel source block at the cursor (or by name). Inserts #+RESULTS: below the block.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "block_name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional #+name of the block to execute (default: block at cursor)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "babel_tangle".into(),
            description: "Tangle the current org buffer — write all :tangle blocks to their target files.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "org_export".into(),
            description: "Export the current org buffer to a file. Writes alongside the source.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "format".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Export format".into(),
                        enum_values: Some(vec!["html".into(), "markdown".into()]),
                    },
                )]),
                required: vec!["format".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "kb_health".into(),
            description: "Compute KB health report: orphan nodes (no links in or out), broken links (references to missing nodes), namespace counts, total stats.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- KB federation tools ---
        ToolDefinition {
            name: "kb_instances".into(),
            description: "List all registered KB instances (name, UUID, node count, enabled).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_register".into(),
            description: "Register an org-roam directory as a federated KB instance. Recursively imports all .org files with :ID: properties. Returns import stats and health metrics (orphans, broken links, namespace distribution).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Display name for the KB instance (e.g. 'RoamNotes', 'Work')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "path".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Path to the org directory (supports ~ expansion)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["name".into(), "path".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "kb_unregister".into(),
            description: "Unregister a federated KB instance by name or UUID. Removes it from the registry and frees memory.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name or UUID of the instance to unregister".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["name".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_reimport".into(),
            description: "Re-import a registered KB instance from its org directory. Use after editing org files to refresh the graph. Returns updated import stats and health metrics.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name or UUID of the instance to reimport".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["name".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        // --- KB CRUD tools ---
        ToolDefinition {
            name: "kb_create".into(),
            description: "Create a new node in the local knowledge base. Cannot overwrite MAE manual (builtin) nodes.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Node id (e.g. 'user:my-note', 'concept:my-concept')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "title".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Human-readable title".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "body".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Markdown body (may contain [[link]] markers)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "kind".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Node kind (default: note)".into(),
                            enum_values: Some(vec!["note".into(), "concept".into(), "command".into(), "key".into(), "project".into()]),
                        },
                    ),
                ]),
                required: vec!["id".into(), "title".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_update".into(),
            description: "Update fields on an existing KB node. Cannot modify MAE manual (builtin) nodes. Only provided fields are changed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Node id to update".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "title".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "New title (optional)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "body".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "New body (optional)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "tags".into(),
                        ToolProperty {
                            prop_type: "array".into(),
                            description: "New tags array (optional, replaces existing)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_delete".into(),
            description: "Delete a node from the local knowledge base. Cannot delete MAE manual (builtin) nodes.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Node id to delete".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_search_context".into(),
            description: "RAG-optimized KB search: returns top-K nodes with excerpts for AI reasoning context. Searches local + federated KBs. Use this instead of kb_search + kb_get loops.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "query".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Search query (case-insensitive substring, fuzzy fallback)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max results (default 5, max 20)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["query".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- KB sharing tools ---
        ToolDefinition {
            name: "kb_share".into(),
            description: "Share a knowledge base for collaborative editing via the connected state server. Shares all nodes in the KB instance.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name of the KB instance to share (default: 'default' = primary KB)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_join".into(),
            description: "Join a shared KB from the connected state server. Downloads all nodes and enables continuous sync.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "KB identifier on the server (e.g. 'default', 'work-notes')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["kb_id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_leave".into(),
            description: "Leave (unsubscribe from) a shared KB. Local copy is preserved but sync stops.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "KB identifier to leave".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["kb_id".into()],
            },
            permission: Some(PermissionTier::Write),
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
