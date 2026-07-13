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
            description: "Search all knowledge base nodes (MAE manual + user + federated). Orderless, field-weighted relevance ranking over titles, ids, bodies, tags, and aliases (multi-word queries are AND-matched, order-independent). Returns an array of objects {id, title, kind, instance, excerpt} in relevance order; `instance` is null for local nodes. Returns up to `limit` results (default kb_search_max_results); use kb_list to enumerate every node id.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "query".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Search terms (case-insensitive, order-independent)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Which KBs to search: 'all' (default), 'local' (primary only), 'remote' (shared/collaborative instances only), or a specific instance name.".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max results to return (default: kb_search_max_results).".into(),
                            enum_values: None,
                        },
                    ),
                ]),
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
            name: "kb_related".into(),
            description: "Find nodes structurally related to a seed node — distinct from kb_search (which matches text). Combines graph signals (direct links, co-citation, bibliographic coupling) and shared tags. Returns [{id, title, kind, score}] sorted by relatedness. Use this to suggest \"see also\" topics or discover adjacent concepts the text search wouldn't surface. Relatedness is computed within the node's own KB instance.".into(),
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
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max related nodes to return (default 10)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Native KB graph view (Part C Phase 1) ---
        ToolDefinition {
            name: "kb_graph_view_open".into(),
            description: "Open the native KB graph view — a force-directed local ego-network panel around a KB node, distinct from kb_graph (which just answers a raw BFS query). Centers on `id` (default: whichever KB node the *KB* buffer is currently showing, else \"index\") at `depth` hops (default: the kb_graph_default_depth option). Reuses the existing graph window if already open.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB node id to center the graph on".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "depth".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Hop radius (default: kb_graph_default_depth option)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_close".into(),
            description: "Close the native KB graph view, if open.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_refresh".into(),
            description: "Refresh the native KB graph view in place (same center node/depth, freshly re-extracted data) if it's open. Never re-splits or steals focus.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_set_depth".into(),
            description: "Change the native KB graph view's hop radius and refresh in place.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "depth".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "New hop radius".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["depth".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_navigate".into(),
            description: "Move the native KB graph view's node selection toward a direction.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "direction".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Direction to move the selection".into(),
                        enum_values: Some(vec![
                            "up".into(),
                            "down".into(),
                            "left".into(),
                            "right".into(),
                        ]),
                    },
                )]),
                required: vec!["direction".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_select_current".into(),
            description: "Navigate the graph view's captured companion window (the last non-graph window focused) to the currently-selected node's KB buffer — NOT the graph window itself.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph_view_state".into(),
            description: "Structured introspection snapshot of the open native KB graph view: which node is hovered, which is selected, every node currently rendered (the ego-network), and every edge/link shown between them. Returns null if no graph view is open. Same data both the human's visual rendering and the (kb-graph-view-state) Scheme primitive read from — use this to reason about or narrate what the graph currently looks like.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- KB-link hover preview (Part D) ---
        ToolDefinition {
            name: "kb_preview_show".into(),
            description: "Show the KB-link hover preview popup for a node id, anchored at the current cursor position — same popup shown by the human's cursor-hover/K-keybinding trigger. `id` doesn't need to be the target of a link under the cursor. Scoped to KB-view-mode buffers. Returns the popup's rendered content (title + noise-stripped, truncated body).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "KB node id to preview".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_preview_dismiss".into(),
            description: "Dismiss the KB-link hover preview popup, if showing.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
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
        ToolDefinition {
            name: "kb_sync_status".into(),
            description: "Per-federated-instance sync/freshness diagnostics: whether kb_notes_dir resolves to a registered instance, whether that instance's filesystem watcher is actually attached (not just whether one was ever expected), any watcher attach error, and seconds since its last drain. Use this to diagnose 'why didn't another mae process see my new/changed node'.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_id_audit".into(),
            description: "Detect ghost/stale node ids that no longer match reality: either an id no longer produced by its (still-existing) source file's current content (an in-place :ID: edit/rename), or a node whose source_file has been deleted/renamed entirely. Re-parses/stats each distinct source file on demand — more expensive than kb_health, call when investigating id-rename or duplicate-node symptoms, not routinely.".into(),
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
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Name or UUID of the instance to reimport".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "mode".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Ingest mode (default: full). \"incremental\" only \
                                re-parses files whose content hash changed since the last \
                                import — it will NOT pick up a per-node metadata fix (e.g. a \
                                newly-added source_file stamp) for files whose content is \
                                unchanged. Use \"full\" (the default) after any ingestion-logic \
                                change, not just after editing org files."
                                .into(),
                            enum_values: Some(vec!["full".into(), "incremental".into()]),
                        },
                    ),
                ]),
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
        // --- KB graph-native tools (CozoDB backend) ---
        ToolDefinition {
            name: "kb_shortest_path".into(),
            description: "Find the shortest path between two KB nodes via link graph (BFS). Returns an ordered list of node IDs from source to target. Requires CozoDB backend; returns error on SQLite.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "from".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Source node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "to".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Target node id".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["from".into(), "to".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_neighborhood".into(),
            description: "Graph neighborhood around a seed node with typed edges (from the persistent store). Returns {nodes: [[id, title]], edges: [[src, dst, rel_type]]}. Requires CozoDB backend for typed edges; falls back to in-memory BFS on SQLite.".into(),
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
                            description: "Hop radius (default 2, max 5)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_add_link".into(),
            description: "Add a typed relationship between two KB nodes. Relationship types: implements, extends, contradicts, explains, references, supersedes, part_of, related_to. Weight defaults to 1.0. Requires CozoDB backend.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "src".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Source node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "dst".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Target node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "rel_type".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Relationship type".into(),
                            enum_values: Some(vec![
                                "implements".into(),
                                "extends".into(),
                                "contradicts".into(),
                                "explains".into(),
                                "references".into(),
                                "supersedes".into(),
                                "part_of".into(),
                                "related_to".into(),
                            ]),
                        },
                    ),
                    (
                        "weight".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Edge weight (default 1.0)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["src".into(), "dst".into(), "rel_type".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_raw_query".into(),
            description: "Execute a raw query on the KB store backend. CozoDB: Datalog syntax. SQLite: SQL. Returns {headers: [...], rows: [[...]]}. Use with caution — no schema validation.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "query".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Query string (Datalog for CozoDB, SQL for SQLite)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["query".into()],
            },
            permission: Some(PermissionTier::Privileged),
        },
        // --- KB sharing tools ---
        ToolDefinition {
            name: "kb_sharing_status".into(),
            description: "Introspect this peer's KB-sharing state: every shared/joined KB with its members + roles, join policy, pending requests, your own role and authorization epoch, and live sync status. Read-only; reflects this peer's local replica (the daemon is authoritative). Call this BEFORE managing membership so you know who is a member and what the fingerprints are.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional: scope to a single KB by id/name (e.g. 'collabtest'). Omit for all shared/joined KBs.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "daemon_status".into(),
            description: "Introspect daemon state + per-feature availability (ADR-035 capability model): the configured daemon_mode, whether a daemon is present/connected/hosting, and for each daemon-dependent feature its requirement (none|recommends|requires) and current availability (available|degraded|unavailable) with the reason + how-to-fix. Read-only. Call this BEFORE a daemon-dependent action (e.g. P2P KB sharing, continuous shared sync) to know whether it will work and, if not, exactly what to tell the user to do.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "feature".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional: scope to one feature by id (e.g. 'p2p-sharing', 'continuous-sync', 'kb-hosting', 'mesh-membership', 'multi-frontend-sharing', 'cross-session-persistence'). Omit for the full snapshot of all features.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_share".into(),
            description: "Share a knowledge base for collaborative editing via the connected daemon. Shares all nodes in the named KB instance (NOT the active/default KB unless you name it).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name of the KB instance to share, e.g. 'collabtest' (default: 'default' = primary KB). Alias: kb_name.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_share_p2p".into(),
            description: "Mint a shareable P2P join ticket (a 'magnet link', mae://join/…) for a KB so a remote peer can join the daemon mesh with no central server. Returns the ticket string in the result; hand it to a collaborator who runs kb_join / `kb-join <ticket>`. Requires the daemon to have P2P enabled (`mae setup-collab --p2p`).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "kb_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "KB instance to share (default: the active/primary KB). Alias: kb_name.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_join_p2p".into(),
            description: "Queue a P2P join from a 'magnet link' ticket (mae://join/…) you received from a KB owner. The daemon's background dialer then connects to the owner by node-id and pulls the KB once the owner approves your join. Requires the daemon to have P2P enabled (`mae setup-collab --p2p`).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "ticket".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "The mae://join/… ticket shared by the KB owner.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["ticket".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_join".into(),
            description: "Join a shared KB from the connected daemon. Downloads all nodes and enables continuous sync.".into(),
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
        ToolDefinition {
            name: "kb_add_member".into(),
            description: "Add a peer to a shared KB's membership, or change their role (owner-only, ADR-018). The peer is identified by its collab identity fingerprint. Controls who may join and write.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "member".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Peer's collab identity fingerprint, e.g. 'SHA256:9xLh0...'".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "role".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Role for the peer (default 'editor'): 'viewer' = read-only, 'editor' = read+write, 'owner' = full control.".into(),
                            enum_values: Some(vec![
                                "viewer".into(),
                                "editor".into(),
                                "owner".into(),
                            ]),
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "member".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_remove_member".into(),
            description: "Remove a peer from a shared KB's membership (owner-only, ADR-018). The peer can no longer join or write; their local copy is unaffected.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "member".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Peer's collab identity fingerprint to remove".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "member".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_block_member".into(),
            description: "Locally block a principal on a KB (ADR-039 A2 self-protection deny-list). This is a LOCAL-ONLY override on THIS daemon — never propagated to peers (distinct from kb_remove_member, which is a global membership removal). Use it to stop trusting a principal you cannot get globally removed (e.g. you lack quorum). It is NOT owner-gated — you may block even the owner. The blocked principal is fenced at every membership check (access + content).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "member".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Principal's collab identity fingerprint to block locally".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "member".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_unblock_member".into(),
            description: "Remove a principal from a KB's LOCAL self-protection blocklist (ADR-039 A2), restoring this daemon's trust in them (subject to their normal derived membership). The inverse of kb_block_member.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "member".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Principal's collab identity fingerprint to unblock".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "member".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_approve".into(),
            description: "Approve a pending join request on a shared KB, granting the peer membership at a role (owner-only, ADR-018). Under the 'invite' policy, non-members' joins become pending until approved. Use kb_sharing_status first to read the pending fingerprints.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "member".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Pending peer's collab identity fingerprint to approve".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "role".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Role to grant (default 'editor')".into(),
                            enum_values: Some(vec!["owner".into(), "editor".into(), "viewer".into()]),
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "member".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_set_policy".into(),
            description: "Set a shared KB's join policy (owner-only, ADR-018): 'restrictive' (only explicitly-added members), 'invite' (joins become pending → approve), or 'permissive' (any authenticated peer auto-joins as viewer).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "policy".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Join policy".into(),
                            enum_values: Some(vec![
                                "restrictive".into(),
                                "invite".into(),
                                "permissive".into(),
                            ]),
                        },
                    ),
                ]),
                required: vec!["kb_id".into(), "policy".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_set_encryption".into(),
            description: "Enable E2E content encryption on an owned KB (owner-only, one-way; ADR-037). The owner generates a per-KB content key, wraps it to each member through the signed membership log, and the daemon/relay stays key-blind (cannot read content). Restricted to single-owner KBs.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB identifier (e.g. 'collabtest')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "mode".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Encryption mode (only 'e2e'; one-way)".into(),
                            enum_values: Some(vec!["e2e".into()]),
                        },
                    ),
                ]),
                required: vec!["kb_id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_set_role".into(),
            description: "Set a KB node's molecular-note role: source (raw external material), atom (established knowledge in your own words), molecule (personal synthesis/insight), or hub (organized map-of-content linking related notes). Stamped as a :role: PROPERTIES-drawer field, orthogonal to the node's existing kind (MAE's own doc taxonomy) -- a node can be both kind=concept and role=atom at once. Freely overwritable as understanding matures.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "role".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Molecular-note role".into(),
                            enum_values: Some(vec![
                                "source".into(),
                                "atom".into(),
                                "molecule".into(),
                                "hub".into(),
                            ]),
                        },
                    ),
                ]),
                required: vec!["id".into(), "role".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_set_ai_residency".into(),
            description: "Set a KB's AI-residency policy (ADR-048): 'open' (any AI provider may read/write this KB, default) or 'local_models_only' (only a locally-classified provider such as Ollama may — enforced at tool dispatch; a hosted/cloud provider like Claude/OpenAI/Gemini/DeepSeek is denied). A plain, freely-toggleable local setting — not the anti-downgrade signed policy used for shared-KB peer trust.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "kb".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "KB instance name/UUID, or \"primary\" for the primary/local KB".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "policy".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "AI-residency policy".into(),
                            enum_values: Some(vec!["open".into(), "local_models_only".into()]),
                        },
                    ),
                ]),
                required: vec!["kb".into(), "policy".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Graph KB tools (v0.12.0) ---
        ToolDefinition {
            name: "kb_agenda".into(),
            description: "Query KB nodes using agenda-style filters. Returns matching nodes as JSON array. Filters: todo (by state), priority (minimum char), tag, stale (days), orphan (no links), dead_end (no outgoing), missing_role (no :role: property set), weakly_linked (fewer than N outgoing links), custom (raw Datalog).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "filter".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Filter type: todo, priority, tag, stale, orphan, dead_end, missing_role, weakly_linked, custom".into(),
                            enum_values: Some(vec![
                                "todo".into(), "priority".into(), "tag".into(),
                                "stale".into(), "orphan".into(), "dead_end".into(),
                                "missing_role".into(), "weakly_linked".into(), "custom".into(),
                            ]),
                        },
                    ),
                    (
                        "value".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Filter value: todo state (e.g. 'TODO'), priority char (e.g. 'A'), tag name, days for stale, N for weakly_linked, or Datalog query for custom".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["filter".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_history".into(),
            description: "Get version history for a KB node. Returns array of {version, title, change_summary, content_hash, created_at}. Requires CozoDB backend.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Node ID to get history for".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max versions to return (default 10)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_restore".into(),
            description: "Restore a KB node to a previous version. Creates a pre-restore snapshot first. Verifies SHA-256 content hash integrity before applying.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Node ID to restore".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "version".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Version number to restore to".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into(), "version".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "kb_view_query".into(),
            description: "Execute a pre-defined KB view by ID (e.g. view:kanban, view:backlog, view:sprint, view:timeline, view:agenda, view:orphans). Runs the view's stored Datalog query and returns results.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "view_id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "View node ID (e.g. 'view:kanban')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["view_id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_vector_search".into(),
            description: "Semantic (vector) KB search — the similarity-by-meaning modality alongside kb_search (lexical) and kb_related (graph). Currently UNAVAILABLE: no embedding provider is configured, so it returns a clear error pointing you to kb_search / kb_related. Shares their scope/limit contract for when embeddings are wired.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "query".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Text query (will be embedded when an embedding provider is configured)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Which KBs to search: 'all' (default), 'local', 'remote', or an instance name (same as kb_search)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max results (default: kb_search_max_results)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["query".into()],
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
