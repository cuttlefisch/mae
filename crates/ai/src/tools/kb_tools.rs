use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// Knowledge base tool definitions: get, search, list, links, graph, help, org.
pub(super) fn kb_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // ---- Knowledge base (shared with :help) ----
        //
        // The KB is the source of truth for command/concept/key
        // documentation. The same nodes the human reads via `:help`
        // are queryable here — the agent is a peer reader.
        ToolDefBuilder::new(
            "kb_get",
            "Fetch a knowledge-base node by id. Returns JSON with title, kind, body (may contain [[link]] markers), tags, outgoing links, and incoming backlinks. IDs are namespaced like 'cmd:<name>', 'concept:<slug>', 'key:<context>', or 'index'. WARNING: Linkage is high; pull atomic info and avoid walking the entire graph.",
        )
        .prop("id", "string", "Node id, e.g. 'index', 'concept:buffer', 'cmd:save'")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_search",
            "Search all knowledge base nodes (MAE manual + user + federated). Orderless, field-weighted relevance ranking over titles, ids, bodies, tags, and aliases (multi-word queries are AND-matched, order-independent). Returns an array of objects {id, title, kind, instance, excerpt} in relevance order; `instance` is null for local nodes. Returns up to `limit` results (default kb_search_max_results); use kb_list to enumerate every node id.",
        )
        .prop("query", "string", "Search terms (case-insensitive, order-independent)")
        .prop(
            "scope",
            "string",
            "Which KBs to search: 'all' (default), 'local' (primary only), 'remote' (shared/collaborative instances only), or a specific instance name.",
        )
        .prop("limit", "integer", "Max results to return (default: kb_search_max_results).")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_list",
            "List all KB node ids, sorted. Optional `prefix` filters to a namespace (e.g. prefix='cmd:' returns all command docs).",
        )
        .prop("prefix", "string", "Optional namespace prefix, e.g. 'cmd:', 'concept:'")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_links_from",
            "Outgoing links from a node — the targets of its body's [[link]] markers, in document order (deduplicated). Errors if the node doesn't exist.",
        )
        .prop("id", "string", "Source node id")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_links_to",
            "Incoming links — ids of all KB nodes whose body references this target. Works even if the target node doesn't exist yet (dangling backlinks).",
        )
        .prop("id", "string", "Target node id (may be dangling)")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph",
            "BFS neighborhood around a seed node up to `depth` hops (default 1, max 3). Returns {root, depth, nodes: [{id, title, kind, hop, missing?}], edges: [{src, dst}]}. Use this to orient yourself in the KB before navigating — the local graph tells you which related topics the user might want to explore next.",
        )
        .prop("id", "string", "Seed node id")
        .prop("depth", "integer", "Hop radius (default 1, clamped to 3)")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_related",
            "Find nodes structurally related to a seed node — distinct from kb_search (which matches text). Combines graph signals (direct links, co-citation, bibliographic coupling) and shared tags. Returns [{id, title, kind, score}] sorted by relatedness. Use this to suggest \"see also\" topics or discover adjacent concepts the text search wouldn't surface. Relatedness is computed within the node's own KB instance.",
        )
        .prop("id", "string", "Seed node id")
        .prop("limit", "integer", "Max related nodes to return (default 10)")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Native KB graph view (Part C Phase 1) ---
        ToolDefBuilder::new(
            "kb_graph_view_open",
            "Open the native KB graph view — a force-directed local ego-network panel around a KB node, distinct from kb_graph (which just answers a raw BFS query). Centers on `id` (default: whichever KB node the *KB* buffer is currently showing, else \"index\") at `depth` hops (default: the kb_graph_default_depth option). Reuses the existing graph window if already open.",
        )
        .prop("id", "string", "KB node id to center the graph on")
        .prop("depth", "integer", "Hop radius (default: kb_graph_default_depth option)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new("kb_graph_view_close", "Close the native KB graph view, if open.")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new(
            "kb_graph_view_refresh",
            "Refresh the native KB graph view in place (same center node/depth, freshly re-extracted data) if it's open. Never re-splits or steals focus.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_set_depth",
            "Change the native KB graph view's hop radius and refresh in place.",
        )
        .prop("depth", "integer", "New hop radius")
        .required(["depth"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_navigate",
            "Move the native KB graph view's node selection toward a direction.",
        )
        .prop_enum(
            "direction",
            "string",
            "Direction to move the selection",
            ["up", "down", "left", "right"],
        )
        .required(["direction"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_select_current",
            "Navigate the graph view's captured companion window (the last non-graph window focused) to the currently-selected node's KB buffer — NOT the graph window itself.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_zoom_to",
            "Set the native KB graph view's zoom to an explicit level (0.1-10.0, clamped) — the AI-appropriate equivalent of the mouse wheel's pixel-focus-based zoom, which has no meaningful non-pointer input. Applies to the focused window if it's showing the graph, else the first window found showing it.",
        )
        .prop("zoom", "number", "Target zoom level (0.1-10.0; out-of-range values are clamped)")
        .required(["zoom"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_set_pinned",
            "Pin or unpin a graph node by KB id — the AI-appropriate equivalent of drag-to-pin, no drag gesture needed. Optionally repositions it to (x, y) in scene coordinates; omit both to leave it wherever it currently is. Reflattens every window showing the graph (shared topology, not per-window state).",
        )
        .prop("id", "string", "KB node id to pin/unpin")
        .prop("pinned", "boolean", "true to pin, false to unpin")
        .prop(
            "x",
            "number",
            "Optional new scene x-position (must be given together with y, or omitted)",
        )
        .prop(
            "y",
            "number",
            "Optional new scene y-position (must be given together with x, or omitted)",
        )
        .required(["id", "pinned"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_toggle_overlay",
            "Toggle the native KB graph view between its normal tiled split-window pane and a full-frame modal overlay with a dimmed background, so the graph can be inspected without the tiled pane's size constraints. No-op if no graph view is open. Returns the new overlay state (true = overlay active).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_graph_view_state",
            "Structured introspection snapshot of the open native KB graph view: which node is hovered, which is selected, every node currently rendered (the ego-network), and every edge/link shown between them. Returns null if no graph view is open. Same data both the human's visual rendering and the (kb-graph-view-state) Scheme primitive read from — use this to reason about or narrate what the graph currently looks like.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- KB-link hover preview (Part D) ---
        ToolDefBuilder::new(
            "kb_preview_show",
            "Show the KB-link hover preview popup for a node id, anchored at the current cursor position — same popup shown by the human's cursor-hover/K-keybinding trigger. `id` doesn't need to be the target of a link under the cursor. Scoped to KB-view-mode buffers. Returns the popup's rendered content (title + noise-stripped, truncated body).",
        )
        .prop("id", "string", "KB node id to preview")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_preview_dismiss",
            "Dismiss the KB-link hover preview popup, if showing.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "help_open",
            "Look up MAE manual content for your own reasoning (searches builtin nodes first, falls back to user KB). Does not open a visible buffer. To show help to the user, suggest `:help <topic>`. Falls back to the `index` node if the id isn't found.",
        )
        .prop("id", "string", "Node id to open, e.g. 'index', 'concept:buffer', 'cmd:save'")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Babel tools ---
        ToolDefBuilder::new(
            "babel_execute",
            "Execute the org-babel source block at the cursor (or by name). Inserts #+RESULTS: below the block.",
        )
        .prop(
            "block_name",
            "string",
            "Optional #+name of the block to execute (default: block at cursor)",
        )
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "babel_tangle",
            "Tangle the current org buffer — write all :tangle blocks to their target files.",
        )
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "org_export",
            "Export the current org buffer to a file. Writes alongside the source.",
        )
        .prop_enum("format", "string", "Export format", ["html", "markdown"])
        .required(["format"])
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "kb_health",
            "Compute KB health report: orphan nodes (no links in or out), broken links (references to missing nodes), namespace counts, total stats.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_sync_status",
            "Per-federated-instance sync/freshness diagnostics: whether kb_notes_dir resolves to a registered instance, whether that instance's filesystem watcher is actually attached (not just whether one was ever expected), any watcher attach error, and seconds since its last drain. Use this to diagnose 'why didn't another mae process see my new/changed node'.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_id_audit",
            "Detect ghost/stale node ids that no longer match reality: either an id no longer produced by its (still-existing) source file's current content (an in-place :ID: edit/rename), or a node whose source_file has been deleted/renamed entirely. Re-parses/stats each distinct source file on demand — more expensive than kb_health, call when investigating id-rename or duplicate-node symptoms, not routinely.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- KB federation tools ---
        ToolDefBuilder::new(
            "kb_instances",
            "List all registered KB instances (name, UUID, node count, enabled).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_register",
            "Register an org-roam directory as a federated KB instance. Recursively imports all .org files with :ID: properties. Returns import stats and health metrics (orphans, broken links, namespace distribution).",
        )
        .prop("name", "string", "Display name for the KB instance (e.g. 'RoamNotes', 'Work')")
        .prop("path", "string", "Path to the org directory (supports ~ expansion)")
        .required(["name", "path"])
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "kb_unregister",
            "Unregister a federated KB instance by name or UUID. Removes it from the registry and frees memory.",
        )
        .prop("name", "string", "Name or UUID of the instance to unregister")
        .required(["name"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_reimport",
            "Re-import a registered KB instance from its org directory. Use after editing org files to refresh the graph. Returns updated import stats and health metrics.",
        )
        .prop("name", "string", "Name or UUID of the instance to reimport")
        .prop_enum(
            "mode",
            "string",
            "Ingest mode (default: full). \"incremental\" only re-parses files whose content hash changed since the last import — it will NOT pick up a per-node metadata fix (e.g. a newly-added source_file stamp) for files whose content is unchanged. Use \"full\" (the default) after any ingestion-logic change, not just after editing org files.",
            ["full", "incremental"],
        )
        .required(["name"])
        .permission(PermissionTier::Shell)
        .build(),
        // --- KB CRUD tools ---
        ToolDefBuilder::new(
            "kb_create",
            "Create a new node in the local knowledge base. Cannot overwrite MAE manual (builtin) nodes.",
        )
        .prop("id", "string", "Node id (e.g. 'user:my-note', 'concept:my-concept')")
        .prop("title", "string", "Human-readable title")
        .prop("body", "string", "Markdown body (may contain [[link]] markers)")
        .prop_enum(
            "kind",
            "string",
            "Node kind (default: note)",
            ["note", "concept", "command", "key", "project"],
        )
        .required(["id", "title"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_update",
            "Update fields on an existing KB node. Cannot modify MAE manual (builtin) nodes. Only provided fields are changed.",
        )
        .prop("id", "string", "Node id to update")
        .prop("title", "string", "New title (optional)")
        .prop("body", "string", "New body (optional)")
        .prop("tags", "array", "New tags array (optional, replaces existing)")
        .required(["id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_delete",
            "Delete a node from the local knowledge base. Cannot delete MAE manual (builtin) nodes.",
        )
        .prop("id", "string", "Node id to delete")
        .required(["id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_search_context",
            "RAG-optimized KB search: returns top-K nodes with excerpts for AI reasoning context. Searches local + federated KBs. Use this instead of kb_search + kb_get loops.",
        )
        .prop("query", "string", "Search query (case-insensitive substring, fuzzy fallback)")
        .prop("limit", "integer", "Max results (default 5, max 20)")
        .required(["query"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- KB graph-native tools (CozoDB backend) ---
        ToolDefBuilder::new(
            "kb_shortest_path",
            "Find the shortest path between two KB nodes via link graph (BFS). Returns an ordered list of node IDs from source to target. Requires CozoDB backend; returns error on SQLite.",
        )
        .prop("from", "string", "Source node id")
        .prop("to", "string", "Target node id")
        .required(["from", "to"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_neighborhood",
            "Graph neighborhood around a seed node with typed edges (from the persistent store). Returns {nodes: [[id, title]], edges: [[src, dst, rel_type]]}. Requires CozoDB backend for typed edges; falls back to in-memory BFS on SQLite.",
        )
        .prop("id", "string", "Seed node id")
        .prop("depth", "integer", "Hop radius (default 2, max 5)")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_add_link",
            "Add a typed relationship between two KB nodes. Relationship types: implements, extends, contradicts, explains, references, supersedes, part_of, related_to. Weight defaults to 1.0. Requires CozoDB backend.",
        )
        .prop("src", "string", "Source node id")
        .prop("dst", "string", "Target node id")
        .prop_enum(
            "rel_type",
            "string",
            "Relationship type",
            [
                "implements",
                "extends",
                "contradicts",
                "explains",
                "references",
                "supersedes",
                "part_of",
                "related_to",
            ],
        )
        .prop("weight", "number", "Edge weight (default 1.0)")
        .required(["src", "dst", "rel_type"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_raw_query",
            "Execute a raw query on the KB store backend. CozoDB: Datalog syntax. SQLite: SQL. Returns {headers: [...], rows: [[...]]}. Use with caution — no schema validation.",
        )
        .prop("query", "string", "Query string (Datalog for CozoDB, SQL for SQLite)")
        .required(["query"])
        .permission(PermissionTier::Privileged)
        .build(),
        // --- KB sharing tools ---
        ToolDefBuilder::new(
            "kb_sharing_status",
            "Introspect this peer's KB-sharing state: every shared/joined KB with its members + roles, join policy, pending requests, your own role and authorization epoch, and live sync status. Read-only; reflects this peer's local replica (the daemon is authoritative). Call this BEFORE managing membership so you know who is a member and what the fingerprints are.",
        )
        .prop(
            "kb_id",
            "string",
            "Optional: scope to a single KB by id/name (e.g. 'collabtest'). Omit for all shared/joined KBs.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "daemon_status",
            "Introspect daemon state + per-feature availability (ADR-035 capability model): the configured daemon_mode, whether a daemon is present/connected/hosting, and for each daemon-dependent feature its requirement (none|recommends|requires) and current availability (available|degraded|unavailable) with the reason + how-to-fix. Read-only. Call this BEFORE a daemon-dependent action (e.g. P2P KB sharing, continuous shared sync) to know whether it will work and, if not, exactly what to tell the user to do.",
        )
        .prop(
            "feature",
            "string",
            "Optional: scope to one feature by id (e.g. 'p2p-sharing', 'continuous-sync', 'kb-hosting', 'mesh-membership', 'multi-frontend-sharing', 'cross-session-persistence'). Omit for the full snapshot of all features.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_share",
            "Share a knowledge base for collaborative editing via the connected daemon. Shares all nodes in the named KB instance (NOT the active/default KB unless you name it).",
        )
        .prop(
            "kb_id",
            "string",
            "Name of the KB instance to share, e.g. 'collabtest' (default: 'default' = primary KB). Alias: kb_name.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_share_p2p",
            "Mint a shareable P2P join ticket (a 'magnet link', mae://join/…) for a KB so a remote peer can join the daemon mesh with no central server. Returns the ticket string in the result; hand it to a collaborator who runs kb_join / `kb-join <ticket>`. Requires the daemon to have P2P enabled (`mae setup-collab --p2p`).",
        )
        .prop(
            "kb_id",
            "string",
            "KB instance to share (default: the active/primary KB). Alias: kb_name.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_join_p2p",
            "Queue a P2P join from a 'magnet link' ticket (mae://join/…) you received from a KB owner. The daemon's background dialer then connects to the owner by node-id and pulls the KB once the owner approves your join. Requires the daemon to have P2P enabled (`mae setup-collab --p2p`).",
        )
        .prop("ticket", "string", "The mae://join/… ticket shared by the KB owner.")
        .required(["ticket"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_join",
            "Join a shared KB from the connected daemon. Downloads all nodes and enables continuous sync.",
        )
        .prop("kb_id", "string", "KB identifier on the server (e.g. 'default', 'work-notes')")
        .required(["kb_id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_leave",
            "Leave (unsubscribe from) a shared KB. Local copy is preserved but sync stops.",
        )
        .prop("kb_id", "string", "KB identifier to leave")
        .required(["kb_id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_add_member",
            "Add a peer to a shared KB's membership, or change their role (owner-only, ADR-018). The peer is identified by its collab identity fingerprint. Controls who may join and write.",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop("member", "string", "Peer's collab identity fingerprint, e.g. 'SHA256:9xLh0...'")
        .prop_enum(
            "role",
            "string",
            "Role for the peer (default 'editor'): 'viewer' = read-only, 'editor' = read+write, 'owner' = full control.",
            ["viewer", "editor", "owner"],
        )
        .required(["kb_id", "member"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_remove_member",
            "Remove a peer from a shared KB's membership (owner-only, ADR-018). The peer can no longer join or write; their local copy is unaffected.",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop("member", "string", "Peer's collab identity fingerprint to remove")
        .required(["kb_id", "member"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_block_member",
            "Locally block a principal on a KB (ADR-039 A2 self-protection deny-list). This is a LOCAL-ONLY override on THIS daemon — never propagated to peers (distinct from kb_remove_member, which is a global membership removal). Use it to stop trusting a principal you cannot get globally removed (e.g. you lack quorum). It is NOT owner-gated — you may block even the owner. The blocked principal is fenced at every membership check (access + content).",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop("member", "string", "Principal's collab identity fingerprint to block locally")
        .required(["kb_id", "member"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_unblock_member",
            "Remove a principal from a KB's LOCAL self-protection blocklist (ADR-039 A2), restoring this daemon's trust in them (subject to their normal derived membership). The inverse of kb_block_member.",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop("member", "string", "Principal's collab identity fingerprint to unblock")
        .required(["kb_id", "member"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_approve",
            "Approve a pending join request on a shared KB, granting the peer membership at a role (owner-only, ADR-018). Under the 'invite' policy, non-members' joins become pending until approved. Use kb_sharing_status first to read the pending fingerprints.",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop("member", "string", "Pending peer's collab identity fingerprint to approve")
        .prop_enum("role", "string", "Role to grant (default 'editor')", ["owner", "editor", "viewer"])
        .required(["kb_id", "member"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_set_policy",
            "Set a shared KB's join policy (owner-only, ADR-018): 'restrictive' (only explicitly-added members), 'invite' (joins become pending → approve), or 'permissive' (any authenticated peer auto-joins as viewer).",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop_enum("policy", "string", "Join policy", ["restrictive", "invite", "permissive"])
        .required(["kb_id", "policy"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_set_encryption",
            "Enable E2E content encryption on an owned KB (owner-only, one-way; ADR-037). The owner generates a per-KB content key, wraps it to each member through the signed membership log, and the daemon/relay stays key-blind (cannot read content). Restricted to single-owner KBs.",
        )
        .prop("kb_id", "string", "KB identifier (e.g. 'collabtest')")
        .prop_enum("mode", "string", "Encryption mode (only 'e2e'; one-way)", ["e2e"])
        .required(["kb_id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_set_role",
            "Set a KB node's molecular-note role: source (raw external material), atom (established knowledge in your own words), molecule (personal synthesis/insight), or hub (organized map-of-content linking related notes). Stamped as a :role: PROPERTIES-drawer field, orthogonal to the node's existing kind (MAE's own doc taxonomy) -- a node can be both kind=concept and role=atom at once. Freely overwritable as understanding matures.",
        )
        .prop("id", "string", "KB node id")
        .prop_enum(
            "role",
            "string",
            "Molecular-note role",
            ["source", "atom", "molecule", "hub"],
        )
        .required(["id", "role"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_set_ai_residency",
            "Set a KB's AI-residency policy (ADR-048): 'open' (any AI provider may read/write this KB, default) or 'local_models_only' (only a locally-classified provider such as Ollama may — enforced at tool dispatch; a hosted/cloud provider like Claude/OpenAI/Gemini/DeepSeek is denied). A plain, freely-toggleable local setting — not the anti-downgrade signed policy used for shared-KB peer trust.",
        )
        .prop("kb", "string", "KB instance name/UUID, or \"primary\" for the primary/local KB")
        .prop_enum("policy", "string", "AI-residency policy", ["open", "local_models_only"])
        .required(["kb", "policy"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Graph KB tools (v0.12.0) ---
        ToolDefBuilder::new(
            "kb_agenda",
            "Query KB nodes using agenda-style filters. Returns matching nodes as JSON array. Filters: todo (by state), priority (minimum char), tag, stale (days), orphan (no links), dead_end (no outgoing), missing_role (no :role: property set), weakly_linked (fewer than N outgoing links), custom (raw Datalog).",
        )
        .prop_enum(
            "filter",
            "string",
            "Filter type: todo, priority, tag, stale, orphan, dead_end, missing_role, weakly_linked, custom",
            [
                "todo",
                "priority",
                "tag",
                "stale",
                "orphan",
                "dead_end",
                "missing_role",
                "weakly_linked",
                "custom",
            ],
        )
        .prop(
            "value",
            "string",
            "Filter value: todo state (e.g. 'TODO'), priority char (e.g. 'A'), tag name, days for stale, N for weakly_linked, or Datalog query for custom",
        )
        .required(["filter"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_history",
            "Get version history for a KB node. Returns array of {version, title, change_summary, content_hash, created_at}. Requires CozoDB backend.",
        )
        .prop("id", "string", "Node ID to get history for")
        .prop("limit", "integer", "Max versions to return (default 10)")
        .required(["id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_restore",
            "Restore a KB node to a previous version. Creates a pre-restore snapshot first. Verifies SHA-256 content hash integrity before applying.",
        )
        .prop("id", "string", "Node ID to restore")
        .prop("version", "integer", "Version number to restore to")
        .required(["id", "version"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "kb_view_query",
            "Execute a pre-defined KB view by ID (e.g. view:kanban, view:backlog, view:sprint, view:timeline, view:agenda, view:orphans). Runs the view's stored Datalog query and returns results.",
        )
        .prop("view_id", "string", "View node ID (e.g. 'view:kanban')")
        .required(["view_id"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "kb_vector_search",
            "Semantic (vector) KB search — the similarity-by-meaning modality alongside kb_search (lexical) and kb_related (graph). Currently UNAVAILABLE: no embedding provider is configured, so it returns a clear error pointing you to kb_search / kb_related. Shares their scope/limit contract for when embeddings are wired.",
        )
        .prop(
            "query",
            "string",
            "Text query (will be embedded when an embedding provider is configured)",
        )
        .prop(
            "scope",
            "string",
            "Which KBs to search: 'all' (default), 'local', 'remote', or an instance name (same as kb_search)",
        )
        .prop("limit", "integer", "Max results (default: kb_search_max_results)")
        .required(["query"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Org tools ---
        ToolDefBuilder::new("org_cycle", "Toggle visibility (folding) of the Org heading at the cursor.")
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("org_todo_cycle", "Cycle the TODO state of the Org heading at the cursor.")
            .prop(
                "forward",
                "boolean",
                "true to cycle forward (TODO->DONE), false for backward",
            )
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new(
            "org_open_link",
            "Open the Org link under the cursor (internal jump or external URL).",
        )
        .permission(PermissionTier::Write)
        .build(),
    ]
}
