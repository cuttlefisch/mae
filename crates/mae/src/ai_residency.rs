//! AI-residency gate (ADR-048): prevents a KB flagged `LocalModelsOnly` from
//! having its content read/written by a hosted/cloud AI provider — only a
//! locally-classified provider (Ollama) may touch it.
//!
//! Two call sites enforce this, both funneling through [`check_kb_residency`]:
//! - `ai_event_handler::handle_ai_event` (embedded/`delegate()` sessions), keyed
//!   on the authoritative `editor.ai.provider` — MAE constructed that provider
//!   itself, it can't be lying.
//! - `ai_event_handler::handle_mcp_request` (external MCP clients), keyed on the
//!   PSK-authenticated `RequesterContext` threaded from `shared/mcp` — an
//!   unauthenticated client's self-declared provider is never trusted (see
//!   `shared/mcp/src/lib.rs`'s `initialize` handler).
//!
//! ## Classification, not a hand-maintained allowlist (#350/#351 follow-up)
//!
//! Every `kb_*`/`help_open` tool is explicitly classified by [`classify_kb_tool`]
//! into one [`ToolResidencyShape`]. This replaced an earlier design with two flat
//! `&[&str]` arrays (`SINGLE_TARGET_KB_TOOLS`/`FEDERATED_SCAN_KB_TOOLS`) that any
//! tool not listed in either silently fell through to `Allow` — the actual root
//! cause behind #350/#351 and nine other tools found ungated in the same audit
//! (including `kb_raw_query`, a full content bypass, and `kb_graph`, an
//! explicitly federated BFS walk). `check_kb_residency` now fails **closed** for
//! any `kb_*`/`help_open` name `classify_kb_tool` doesn't recognize, and
//! `every_kb_tool_and_help_open_is_explicitly_classified` (below) makes that
//! drift impossible to reintroduce silently: a new tool with no residency
//! review fails CI with a clear message instead of quietly defaulting to Allow.
//!
//! ## Scope note (v1, still true for [`ToolResidencyShape::UnscopedFederatedContent`])
//!
//! Tools in that bucket do not share a consistent per-result "which instance
//! did this hit come from" shape, so rather than risk a subtly-wrong per-tool
//! result filter, v1 conservatively denies the *entire* call whenever ANY
//! registered KB (or the primary) is `LocalModelsOnly` and the requester isn't
//! local. This is coarser than ADR-048's original "post-filter, don't fail the
//! whole call" design — a documented, honest simplification, not a silent gap.
//! [`ToolResidencyShape::ScopedFederatedScanFilterable`] tools (`kb_search`,
//! `kb_search_context`, `kb_vector_search`) are the escape hatch from that
//! coarseness: they accept a `scope` argument (or fall back to the
//! `kb_search_scope` option) that names exactly which KB(s) participate, so
//! `kb_federated_search_scoped(query, scope)` itself only ever includes KBs
//! within that resolved scope (the actual #351 fix — a call explicitly
//! scoped away from a restricted KB is never blocked by that KB's policy),
//! and each tool then post-filters its own materialized
//! `(Option<String>, Node)` results via
//! `mae_core::ai_residency::filter_residency_exempt` (#358) for the seed
//! exemption within whatever restricted KB genuinely IS in scope.
//!
//! ## Seed-content exemption (#358)
//!
//! `SingleTarget` and `ScopedFederatedScanFilterable`
//! tools exempt MAE's own seeded/built-in content (`Node::source ==
//! Some(NodeSource::Seed)`, stamped once at startup, identical on every
//! install, never sensitive) from `LocalModelsOnly` gating even when it lives
//! in a restricted KB — restricting `primary` to protect a user's own notes
//! must not also lock an AI agent out of MAE's own built-in help system. The
//! filter primitives (`is_residency_exempt`, `filter_residency_exempt`) live
//! in `mae_core::ai_residency` rather than here — a Rust crate-graph
//! constraint (the `mae` package has no `[lib]` target, so nothing in
//! `mae-ai`'s tool implementations can reach this file), not a conceptual
//! split. `SingleTarget` applies the exemption directly in
//! `resolve_restricted_label` (the node is already resolved there);
//! `ScopedFederatedScanFilterable` allows the call through unconditionally
//! and relies on the tool implementation (`execute_kb_agenda`/
//! `execute_kb_search`/`execute_kb_search_context`/`execute_kb_vector_search`
//! in `crates/ai/src/tool_impls/kb.rs`) to post-filter its own materialized
//! results — see the shape's doc comment.
//!
//! Three tool shapes stay structurally unable to apply this exemption, and
//! stay hard-denied on purpose, not as an unfinished TODO:
//! - `kb_raw_query`/`kb_view_query` (`PrimaryOnly`): arbitrary Datalog
//!   against the Cozo store has no schema-level per-row node-identity to
//!   inject a `source != 'seed'` predicate into.
//! - `kb_id_audit` (`UnscopedFederatedContent`): `detect_ghost_ids`/
//!   `detect_stale_nodes` only ever consider nodes with
//!   `source_file.is_some()`; seed nodes never get `source_file` set, so
//!   this tool can never surface seed content regardless of residency
//!   policy.
//! - `kb_graph_view_open`/`kb_graph_view_refresh` (`UnscopedFederatedContent`):
//!   their own responses are counts only (no per-node content at these two
//!   entry points).
//!
//! `kb_related`/`kb_graph` are now handled (#361, `SingleTargetFilterable`/
//! `UnscopedFederatedContentFilterable` above) — the shared-trait extension
//! (`GraphNeighbors`/`RelatedSource::describe` now also return
//! `is_seed_content`) turned out to be cheap: both backends already fetched
//! the full `Node` and discarded everything but title/kind. `kb_history`/
//! `kb_restore` needed no code change at all: they were already
//! `SingleTarget` with `"id"` in `TARGET_ARG_KEYS`, so `resolve_restricted_
//! label`'s existing exemption check already covers them (their result
//! shape is version metadata for the SAME id, so there is no other-node
//! traversal-leak vector these two tools even have).
//!
//! `kb_neighborhood`/`kb_health`/`kb_graph_view_state` are now handled too
//! (#361, `SingleTargetFilterable`/`UnscopedFederatedContentFilterable`
//! above) -- `kb_graph_view_state` threads a per-node `is_seed` flag from
//! `mae_kb::Node::source` all the way through `mae-canvas`'s `KbNodeInfo`/
//! `SceneNode` (a deliberate no-`mae-kb`-dependency leaf crate, so this is a
//! structural mirror field, same pattern as `NodeKind`) into
//! `GraphViewNodeState`, letting the AI's read of an already-open graph
//! buffer filter itself without restricting what the human sees on screen.
//!
//! `kb_links_from`/`kb_links_to`/`kb_shortest_path` are now handled too
//! (#366, "Bucket B" — `SingleTargetFilterable` above): unlike Bucket A,
//! neither backend's links-relation query ever touched a target's full
//! `Node`, so extending the exemption needed a genuine new per-target-id
//! lookup rather than a mechanical thread-the-field change. Reuses the
//! same `GraphNeighbors::describe()` backend `kb_graph`/`kb_related`
//! already have (`LinksBackend` in `crates/ai/src/tool_impls/kb.rs`),
//! accepting the per-result lookup cost rather than batching — these are
//! typically small, bounded result sets in practice (#366's own scoping
//! note). `kb_shortest_path`'s own `CozoKbStore` implementation is
//! currently a reachability check, not real path reconstruction (only
//! ever returns `[from, to]`, see that function's doc comment) — a
//! separate, pre-existing correctness quirk, not this fix's scope — so
//! filtering it re-resolves each returned id via `editor.kb.store`
//! (always primary, matching `kb_neighborhood`'s scope) and drops the
//! WHOLE path (not just the offending hop) if any id isn't seed-exempt,
//! since a partial path is not a meaningful result.
//!
//! `kb_list`'s CozoDB-backed path is the one piece of #366 NOT done here —
//! deliberately scoped out as a down payment (CLAUDE.md principle #15):
//! extending it needs a `KbQueryLayer::list_ids` trait signature change
//! (to carry `.source` per id) across all EIGHT implementors
//! (`CozoQueryLayer`, `FederatedQuery`, `InMemoryQueryLayer`,
//! `CachedQueryLayer`, `LruQueryLayer`, `RemoteHubQueryLayer`, plus two
//! test-only layers) — a much larger blast radius than the other three
//! tools for a precision improvement, not a security fix (`kb_list` stays
//! `UnscopedFederatedContent`: safe today, just coarser than necessary).
//! Tracked as a follow-up issue cross-linked from #366 rather than
//! silently dropped.

use mae_core::ai_residency::{is_local_provider, is_residency_exempt};
use mae_core::Editor;

/// Argument keys, across [`ToolResidencyShape::SingleTarget`] tools, that hold
/// a node id or an explicit KB instance name/uuid worth resolving.
const TARGET_ARG_KEYS: &[&str] = &["id", "src", "dst", "from", "to", "kb", "name"];

/// How a `kb_*`/`help_open` tool's content exposure relates to AI-residency
/// policy — see the module doc for why this replaced two hand-maintained
/// arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolResidencyShape {
    /// Resolves to exactly one (or two) specific node id(s)/KB name(s) in
    /// `arguments` — checked precisely via [`TARGET_ARG_KEYS`], with the
    /// seed-content exemption (#358) applied by `resolve_restricted_label`.
    SingleTarget,
    /// Same anchor-argument gate check as [`Self::SingleTarget`] (unchanged —
    /// still denies outright if the anchor id's own KB is restricted and the
    /// anchor isn't seed-exempt), but the tool ALSO traverses to OTHER nodes
    /// that can live in a different KB than the anchor (`kb_related`'s
    /// federated relatedness scan; `kb_neighborhood`'s same-KB-but-different-
    /// node BFS) — so the tool impl additionally post-filters its own
    /// multi-node result list via
    /// `mae_core::ai_residency::filter_residency_exempt_by` (#361). Without
    /// this, a permitted (open or seed) anchor could leak a *different*
    /// restricted KB's non-seed content reached via traversal, or (for
    /// same-KB neighbors) a non-seed sibling reachable from a seed anchor in
    /// the SAME restricted KB.
    SingleTargetFilterable,
    /// Only ever touches the primary store (`editor.kb.store`), never a
    /// federated instance, AND its result shape has no per-node identity to
    /// filter (arbitrary Datalog / a stored view's raw query) — checked
    /// against `primary_ai_residency` only, hard-denied outright.
    PrimaryOnly,
    /// Scans across multiple KB instances via a `scope` argument (or falls
    /// back to the `kb_search_scope` option) that names exactly which KB(s)
    /// participate — scope is resolved FIRST, then residency is checked only
    /// for KBs within that resolved scope (the #351 fix; see
    /// `any_restricted_kb_label_in_scope`), rather than every registered KB.
    /// Its results ARE real `(Option<String>, Node)` pairs — the gate allows
    /// the call through; `kb_search`/`kb_search_context`/`kb_vector_search`/
    /// `kb_agenda` call `mae_core::ai_residency::filter_residency_exempt` on
    /// their own materialized results (#358; `kb_vector_search` joined this
    /// shape in ADR-061 Phase F1 once un-stubbing it gave it real results to
    /// filter — previously a separate, now-removed `ScopedFederatedScan`
    /// shape existed for its old no-real-results-to-filter stub behavior,
    /// hard-denying outright instead of post-filtering. `kb_agenda` joined
    /// in the same integration pass that ported `kb-export-subgraph-html`
    /// upstream — see ADR-083 — once its own federation gap was fixed: it
    /// previously only ever queried `editor.kb.store` (primary), so its
    /// `PrimaryOnlyFilterable` classification was accurate at the time but
    /// became stale drift the moment it started scanning federated
    /// instances too; removed rather than kept as a now-empty variant
    /// (principle #15 — see `unclassified_kb_prefixed_tool_denied_conservatively`
    /// and this module's own dead-code discipline elsewhere in this file).
    ScopedFederatedScanFilterable,
    /// Scans across multiple KB instances with no way to exclude one —
    /// denied outright whenever ANY registered KB (or primary) is
    /// restricted (see the module doc's "Scope note").
    UnscopedFederatedContent,
    /// Same unscoped multi-instance scan as [`Self::UnscopedFederatedContent`],
    /// but its results ARE real per-node data the tool impl can post-filter —
    /// the gate allows the call through unconditionally;
    /// `execute_kb_graph`'s BFS walk (root node included, at hop 0) is
    /// filtered via `mae_core::ai_residency::filter_residency_exempt_by`
    /// (#361), the same pattern `ScopedFederatedScanFilterable` uses for
    /// `kb_search`.
    UnscopedFederatedContentFilterable,
    /// Meta/administrative only — no node titles/bodies/links/content ever
    /// leaves this tool (membership/policy/lifecycle actions, or pure
    /// view-state manipulation of an already-rendered scene). Never gated.
    NonContent,
    /// Resolves its one target from **editor state** (the `ai_guidance_kb`
    /// option), not from `arguments` — unlike [`Self::SingleTarget`], whose
    /// gate mechanism only ever inspects `TARGET_ARG_KEYS` in the call's
    /// arguments and would silently NOT gate a tool that takes no such
    /// argument at all. `kb_export_guidance` writes that KB's content to a
    /// plain file any subsequent agent (local or not) can read, so it must
    /// be gated exactly like a direct read of that KB would be — empty
    /// `ai_guidance_kb` (nothing configured) is always allowed.
    GuidanceKbTarget,
}

/// Explicit residency classification for every `kb_*`/`help_open` AI tool.
/// `None` means "not recognized" — [`check_kb_residency`] fails CLOSED for
/// that case rather than defaulting to Allow (see module doc). Every real
/// tool name must have an arm here; enforced by
/// `every_kb_tool_and_help_open_is_explicitly_classified`.
fn classify_kb_tool(tool_name: &str) -> Option<ToolResidencyShape> {
    use ToolResidencyShape::*;
    Some(match tool_name {
        // --- SingleTarget: resolves to one node id or KB instance name ---
        "kb_get" | "kb_update" | "kb_delete" | "kb_promote" | "kb_restore" | "kb_add_link"
        | "kb_history" | "kb_preview_show" | "kb_create" | "kb_set_role" | "kb_reimport"
        | "help_open" => SingleTarget,

        // --- SingleTarget (not Filterable): kb_export_subgraph_html's BFS
        // walk (mae_kb::KnowledgeBase::extract_subgraph) runs against ONE
        // already-resolved KnowledgeBase (primary, or exactly one federated
        // instance) and structurally never crosses into a different
        // instance -- it only ever follows edges within `self.nodes`. So
        // every node the export can possibly include is guaranteed to live
        // in the SAME KB `id` resolves to; gating on `id` alone (the
        // anchor-id check below) already covers the entire exported
        // subgraph, with no cross-instance leak possible for a
        // post-filter to catch. Unlike kb_graph/kb_neighborhood (federated
        // BFS, genuinely can cross instances -- SingleTargetFilterable).
        // The tool's optional `guidance_ids` argument independently resolves
        // EACH id across every registered store (may land in a DIFFERENT KB
        // than the seed) -- this used to be an unfiltered gap here, since
        // this SingleTarget shape only ever gates on the anchor `id`.
        // `execute_kb_export_subgraph_html` (crates/ai/src/tool_impls/
        // kb_export_html.rs) now closes it itself: a
        // SingleTargetFilterable-style post-filter (`mae_core::ai_residency::
        // filter_residency_exempt_by`, the same primitive `kb_links_from`'s
        // own per-target check already uses) drops any guidance id whose
        // owning KB is residency-restricted and the requester isn't a local
        // provider, and reports what was omitted in the tool's own returned
        // status string -- never silently. This gate still only needs to
        // cover the seed/anchor `id`.
        "kb_export_subgraph_html" => SingleTarget,

        // --- SingleTargetFilterable: same anchor-id gate check as
        // SingleTarget, PLUS the tool impl post-filters its own multi-node
        // traversal results (#361 -- see the shape's doc comment). #366
        // ("Bucket B") added kb_links_from/kb_links_to/kb_shortest_path to
        // this bucket -- kb_links_to moved here FROM UnscopedFederatedContent
        // (it has a well-defined anchor "id" argument after all, same as
        // links_from) now that its aggregated backlink sources are actually
        // filtered rather than the whole call being denied outright. ---
        "kb_related" | "kb_neighborhood" | "kb_links_from" | "kb_links_to"
        | "kb_shortest_path" => SingleTargetFilterable,

        // --- PrimaryOnly: implementation only ever reads editor.kb.store,
        // AND runs arbitrary Datalog with no per-row node-identity to
        // filter -- structurally incapable of the seed exemption (#358) ---
        "kb_raw_query" | "kb_view_query" => PrimaryOnly,

        // --- PrimaryOnly: ADR-061 Phase E, primary-KB-only by construction
        // (federated instances have no CozoKbStore handle in this process
        // today). Its own return value is just counts, but a failed node's
        // id can appear in the "errors" array -- real node-identity leakage,
        // not content, but enough that "no per-row filter" applies (like
        // kb_raw_query) rather than treating it as content-free. This is a
        // SEPARATE, layered check from `execute_kb_enrich`'s own internal
        // `residency_permits_provider` gate on the EMBEDDING provider
        // (ai_embedding_provider) -- that one governs which model processes
        // restricted content; this one governs which REQUESTER may even
        // call the tool and see its (node-id-bearing) output at all. ---
        "kb_enrich" => PrimaryOnly,

        // --- ScopedFederatedScanFilterable: scans across multiple KB
        // instances via an explicit `scope` argument (or a default option),
        // AND the tool impl post-filters its real (Option<String>, Node)
        // results for the seed exemption (#358). kb_vector_search joined
        // this shape in ADR-061 Phase F1/F2: un-stubbing it gave it real
        // RRF-fused (Option<String>, Node) results (mae_core::
        // ai_residency::filter_residency_exempt call in
        // execute_kb_vector_search), the same shape kb_search/
        // kb_search_context already have -- it is no longer the
        // no-real-results-yet ScopedFederatedScan case. kb_agenda joined
        // here (ADR-083) once its `execute_kb_agenda` implementation was
        // fixed to actually scan `editor.kb.registry.instances` matching
        // `scope` (mirroring `execute_kb_health`'s per-instance loop)
        // instead of only ever reading `editor.kb.store` (primary) --
        // previously classified `PrimaryOnlyFilterable`, now removed as a
        // shape with zero real tools in it (principle #15). ---
        "kb_agenda" | "kb_search" | "kb_search_context" | "kb_vector_search" => {
            ScopedFederatedScanFilterable
        }

        // --- UnscopedFederatedContent: genuinely scans multiple instances,
        // no scope argument to narrow it. kb_list stays here (its CozoDB
        // path isn't filterable yet -- #366's explicitly-scoped-out down
        // payment, see module doc); kb_links_to moved to
        // SingleTargetFilterable above (#366) ---
        "kb_graph_view_open" | "kb_graph_view_refresh" | "kb_list" | "kb_id_audit" => {
            UnscopedFederatedContent
        }

        // --- UnscopedFederatedContentFilterable: same unscoped multi-instance
        // scan, AND the tool impl post-filters its real per-node results
        // (root included, for kb_graph), per-KB report (for kb_health), or
        // already-open-graph-buffer state (for kb_graph_view_state) for the
        // seed exemption (#361) ---
        "kb_graph" | "kb_health" | "kb_graph_view_state" => UnscopedFederatedContentFilterable,

        // --- NonContent: pure view/camera-state manipulation of an
        // already-rendered graph scene (no new cross-KB content fetched by
        // these calls themselves) ---
        "kb_graph_view_close"
        | "kb_graph_view_navigate"
        | "kb_graph_view_select_current"
        | "kb_graph_view_zoom_to"
        | "kb_graph_view_set_pinned"
        | "kb_graph_view_toggle_overlay"
        | "kb_graph_view_set_depth"
        // --- NonContent: meta/admin, no titles/bodies/links (kb_instances
        // precedent; kb_sync_status's only leak is an org_dir path) ---
        | "kb_sync_status"
        | "kb_instances"
        | "kb_preview_dismiss"
        | "kb_register"
        | "kb_unregister"
        // --- NonContent: per-project provisioning lifecycle (ADR-058) — registers/
        // declines a project-scoped instance, never reads/returns node content ---
        | "kb_init_project"
        | "kb_decline_project_provisioning"
        // --- NonContent: sharing/membership/policy lifecycle actions —
        // mutate collaboration state, never read/return node content ---
        | "kb_sharing_status"
        | "kb_share"
        | "kb_share_p2p"
        | "kb_join"
        | "kb_join_p2p"
        | "kb_leave"
        | "kb_add_member"
        | "kb_remove_member"
        | "kb_block_member"
        | "kb_unblock_member"
        | "kb_approve"
        | "kb_set_policy"
        | "kb_set_encryption"
        | "kb_set_ai_residency" => NonContent,

        // --- GuidanceKbTarget: reads ai_guidance_kb's content, target
        // resolved from editor state rather than a tool argument ---
        "kb_export_guidance" => GuidanceKbTarget,

        _ => return None,
    })
}

/// Result of an AI-residency check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidencyDecision {
    Allow,
    Deny(String),
}

/// KB-reading Scheme primitives with no MCP-tool sibling for `classify_kb_tool`
/// to gate — the exact bypass #478 reported: `eval_scheme` is not `kb_*`-prefixed,
/// so `classify_kb_tool` never sees it, and an AI agent could call
/// `(eval_scheme "(kb-graph-view-state)")` (or any of these siblings) to read
/// fully unfiltered content from a `LocalModelsOnly`-restricted KB, bypassing
/// the MCP-layer gate entirely. `mae-scheme`'s `SharedState` has no requester
/// identity field at all (used identically for AI and human evals), so the
/// primitives themselves are structurally unable to self-gate — this is why
/// the fix lives here, at the `eval_scheme` call itself, not inside each
/// primitive (would mean duplicating this module's logic N times with no
/// requester context to gate on — CLAUDE.md principle #8).
///
/// Every one of these genuinely reads/exposes KB-derived data (node ids,
/// titles, link targets, raw body text, or block/member counts that confirm
/// node existence) — conservatively includes `kb-block-count`/`kb-pending`/
/// `kb-compose-meta` even though their exposure is narrower, since "fail
/// closed" is cheaper than auditing each one's exact leak surface.
const SENSITIVE_SCHEME_KB_PRIMITIVES: &[&str] = &[
    "kb-graph-view-state",
    "kb-preview-show",
    "kb-agenda",
    "kb-history",
    "kb-restore",
    "kb-raw-query",
    "kb-links-from",
    "kb-links-to",
    "kb-graph",
    "kb-neighborhood",
    "kb-related",
    "kb-shortest-path",
    "kb-links-typed",
    "kb-meta-members",
    "kb-get-block",
    "kb-block-count",
    "kb-pending",
    "kb-compose-meta",
];

/// If `code` references any [`SENSITIVE_SCHEME_KB_PRIMITIVES`] name, return it
/// (the first match, for the deny message). A plain substring scan, not a
/// real Scheme parse — deliberately: a false positive (the name appearing in
/// a string literal or comment that would never actually execute) is the
/// SAFE failure direction for a security gate, not a bug worth chasing down,
/// and arbitrary Scheme code can call a primitive conditionally, in a loop,
/// or build it dynamically, so there's no reliable static "will this actually
/// run" analysis to do instead.
fn scheme_code_references_sensitive_primitive(code: &str) -> Option<&'static str> {
    SENSITIVE_SCHEME_KB_PRIMITIVES
        .iter()
        .find(|name| code.contains(*name))
        .copied()
}

/// Check whether `requester_provider` may run `tool_name` with `arguments`,
/// given the KBs' current AI-residency policies. `requester_provider` is
/// `None` when the requester has no trusted provider identity at all (an
/// unauthenticated external MCP client) — treated the same as "not local".
pub fn check_kb_residency(
    editor: &Editor,
    tool_name: &str,
    arguments: &serde_json::Value,
    requester_provider: Option<&str>,
) -> ResidencyDecision {
    if requester_provider.is_some_and(is_local_provider) {
        return ResidencyDecision::Allow;
    }

    // #478: eval_scheme is not kb_*-prefixed, so classify_kb_tool below never
    // sees it and this gate would otherwise always Allow regardless of what
    // KB-reading Scheme primitives the queued code calls. Denies the WHOLE
    // call (not a post-hoc result filter, unlike the *Filterable MCP-tool
    // shapes above) whenever the code references a sensitive primitive AND
    // any registered KB is currently residency-restricted — coarser than the
    // equivalent direct MCP tool call (which can post-filter a typed result),
    // but arbitrary Scheme code has no equivalent post-hoc filtering point:
    // it could call a primitive conditionally, in a loop, or construct its
    // result programmatically, so there's no clean way to filter an opaque
    // return value the way a typed MCP response can be. Fails closed per
    // ADR-048, matching this module's existing UnscopedFederatedContent
    // precedent (deny outright when a shape has no way to scope/filter).
    if tool_name == "eval_scheme" {
        if let Some(code) = arguments.get("code").and_then(|v| v.as_str()) {
            if let Some(primitive) = scheme_code_references_sensitive_primitive(code) {
                if let Some(label) = any_restricted_kb_label(editor) {
                    return ResidencyDecision::Deny(format!(
                        "AI-residency policy: KB '{label}' is set to local_models_only, and \
                         this session's AI provider ({}) isn't a local model. This eval_scheme \
                         call references '{primitive}', a KB-reading Scheme primitive with no \
                         per-call residency filter — denied outright (not partially run) since \
                         arbitrary Scheme code has no reliable post-hoc result filter the way a \
                         direct MCP tool call does. Use the equivalent kb_* MCP tool instead (it \
                         can filter/scope its result), or switch to a local (Ollama) provider.",
                        requester_provider.unwrap_or("none/unauthenticated")
                    ));
                }
            }
        }
    }

    let Some(shape) = classify_kb_tool(tool_name) else {
        if tool_name.starts_with("kb_") || tool_name == "help_open" {
            // A kb_*/help_open tool this gate doesn't recognize at all --
            // fail CLOSED rather than silently ungate it. This is the exact
            // drift class #350/#351's investigation found nine instances
            // of; see `unclassified_kb_prefixed_tool_denied_conservatively`
            // and `every_kb_tool_and_help_open_is_explicitly_classified`.
            return ResidencyDecision::Deny(format!(
                "AI-residency policy: '{tool_name}' has no explicit residency classification \
                 yet -- denied conservatively rather than silently ungated. This is a gap in \
                 MAE itself, not a policy violation; please file an issue."
            ));
        }
        // Genuinely unrelated tool (buffer_read, git_status, ...) -- not
        // this gate's concern.
        return ResidencyDecision::Allow;
    };

    match shape {
        ToolResidencyShape::NonContent => ResidencyDecision::Allow,

        ToolResidencyShape::GuidanceKbTarget => {
            if editor.ai_guidance_kb.is_empty() {
                return ResidencyDecision::Allow;
            }
            if let Some(label) = resolve_restricted_label(editor, &editor.ai_guidance_kb) {
                return ResidencyDecision::Deny(format!(
                    "AI-residency policy: KB '{label}' is set to local_models_only, and this \
                     session's AI provider ({}) isn't a local model. kb_export_guidance would \
                     write that KB's content to a plain file any subsequent agent can read — \
                     switch to a local (Ollama) provider, or point ai_guidance_kb at an \
                     unrestricted KB.",
                    requester_provider.unwrap_or("none/unauthenticated")
                ));
            }
            ResidencyDecision::Allow
        }

        ToolResidencyShape::PrimaryOnly => {
            if editor.kb.registry.primary_ai_residency
                == mae_kb::federation::AiResidency::LocalModelsOnly
            {
                return ResidencyDecision::Deny(format!(
                    "AI-residency policy: KB 'primary' is set to local_models_only, and this \
                     session's AI provider ({}) isn't a local model.",
                    requester_provider.unwrap_or("none/unauthenticated")
                ));
            }
            ResidencyDecision::Allow
        }

        // The gate allows the call through unconditionally; execute_kb_search/
        // execute_kb_search_context/execute_kb_vector_search/execute_kb_agenda post-filter
        // their own materialized (Option<String>, Node) results via
        // mae_core::ai_residency::filter_residency_exempt (#358). Scope
        // narrowing (the #351 fix) happens naturally inside each tool's own
        // scope-resolution step (kb_federated_search_scoped(query, scope)
        // for kb_search/kb_search_context/kb_vector_search, an equivalent
        // per-instance registry loop for kb_agenda), not as a separate
        // gate-level pre-check.
        ToolResidencyShape::ScopedFederatedScanFilterable => ResidencyDecision::Allow,

        ToolResidencyShape::UnscopedFederatedContent => {
            if let Some(label) = any_restricted_kb_label(editor) {
                return ResidencyDecision::Deny(format!(
                    "AI-residency policy: KB '{label}' is set to local_models_only, and this \
                     session's AI provider ({}) isn't a local model. '{tool_name}' scans across \
                     all registered KBs with no way to exclude one, so it's blocked outright \
                     rather than silently omitting that KB's results -- use kb_get, or a \
                     scope-aware tool like kb_search, instead, or switch to a local (Ollama) \
                     provider.",
                    requester_provider.unwrap_or("none/unauthenticated")
                ));
            }
            ResidencyDecision::Allow
        }

        // Same unscoped scan as UnscopedFederatedContent, but the gate
        // allows the call through unconditionally; execute_kb_graph
        // post-filters its own materialized per-node BFS results (root
        // included) via mae_core::ai_residency::filter_residency_exempt_by
        // (#361).
        ToolResidencyShape::UnscopedFederatedContentFilterable => ResidencyDecision::Allow,

        // SingleTargetFilterable's gate check is identical to SingleTarget's
        // (same anchor-id resolution below) -- the extra post-filtering
        // (#361) happens entirely inside the tool impl, not here.
        ToolResidencyShape::SingleTarget | ToolResidencyShape::SingleTargetFilterable => {
            for key in TARGET_ARG_KEYS {
                let Some(value) = arguments.get(*key).and_then(|v| v.as_str()) else {
                    continue;
                };
                if let Some(label) = resolve_restricted_label(editor, value) {
                    return ResidencyDecision::Deny(format!(
                        "AI-residency policy: KB '{label}' is set to local_models_only, and \
                         this session's AI provider ({}) isn't a local model.",
                        requester_provider.unwrap_or("none/unauthenticated")
                    ));
                }
            }
            ResidencyDecision::Allow
        }
    }
}

/// If `value` names a `LocalModelsOnly`-restricted KB — either as a literal
/// instance name/UUID/"primary", or as a node id owned by one — return that
/// KB's display label. `None` means unrestricted (or not found at all; a
/// missing node/instance is the underlying tool's error to report, not this
/// gate's).
fn resolve_restricted_label(editor: &Editor, value: &str) -> Option<String> {
    // Literal KB reference first ("primary" or an instance name/uuid) — this is
    // how `kb_add_link`'s src/dst usually aren't KB names, but `kb`-style args
    // on other tools could be; cheap to check before falling back to node-id
    // resolution.
    if value.eq_ignore_ascii_case("primary") {
        if editor.kb.registry.primary_ai_residency
            == mae_kb::federation::AiResidency::LocalModelsOnly
        {
            return Some("primary".to_string());
        }
        return None;
    }
    if let Some(inst) = editor.kb.registry.find(value) {
        if inst.ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly {
            return Some(inst.name.clone());
        }
        return None;
    }

    // Fall through to node-id resolution: which KB (primary or a registered
    // instance) actually contains this id? MAE's own seeded/built-in
    // content is exempt from gating regardless of the owning KB's policy
    // (#358) -- checked here since the node is already in hand.
    if let Some(node) = editor.kb.primary.get(value) {
        if editor.kb.registry.primary_ai_residency
            == mae_kb::federation::AiResidency::LocalModelsOnly
            && !is_residency_exempt(node)
        {
            return Some("primary".to_string());
        }
        return None;
    }
    for (uuid, kb) in editor.kb.instances.iter() {
        if let Some(node) = kb.get(value) {
            if let Some(inst) = editor.kb.registry.find_by_uuid(uuid) {
                if inst.ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly
                    && !is_residency_exempt(node)
                {
                    return Some(inst.name.clone());
                }
            }
            return None;
        }
    }
    None
}

/// The display label of the first `LocalModelsOnly`-restricted KB found (primary
/// or any registered instance), if any — used by
/// [`ToolResidencyShape::UnscopedFederatedContent`], which has no `scope`
/// argument to narrow the check.
fn any_restricted_kb_label(editor: &Editor) -> Option<String> {
    if editor.kb.registry.primary_ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly {
        return Some("primary".to_string());
    }
    editor
        .kb
        .registry
        .instances
        .iter()
        .find(|inst| inst.ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly)
        .map(|inst| inst.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with_restricted_primary() -> Editor {
        let mut editor = Editor::new();
        editor.kb.registry.primary_ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;
        editor
    }

    /// A registered, open (non-restricted) instance named `name`.
    fn open_instance(name: &str, uuid: &str) -> mae_kb::federation::KbInstance {
        mae_kb::federation::KbInstance {
            uuid: uuid.into(),
            name: name.into(),
            org_dir: std::path::PathBuf::new(),
            db_path: std::path::PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::Open,
            project_root: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            priority: 0,
            remote_hub: None,
        }
    }

    /// A registered, restricted instance named `name`.
    fn restricted_instance(name: &str, uuid: &str) -> mae_kb::federation::KbInstance {
        let mut inst = open_instance(name, uuid);
        inst.ai_residency = mae_kb::federation::AiResidency::LocalModelsOnly;
        inst
    }

    // --- Pre-existing coverage, still true under the new classification ---

    #[test]
    fn local_provider_always_allowed() {
        let editor = editor_with_restricted_primary();
        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_get",
                &serde_json::json!({"id": "index"}),
                Some("ollama")
            ),
            ResidencyDecision::Allow
        );
        assert_eq!(
            check_kb_residency(&editor, "kb_search", &serde_json::json!({}), Some("ollama")),
            ResidencyDecision::Allow
        );
    }

    #[test]
    fn non_local_provider_denied_single_target_tool_on_restricted_primary() {
        let mut editor = editor_with_restricted_primary();
        // A genuinely user-authored (non-seed) node -- "index" itself is
        // seed content and is now correctly exempt (#358), so this test
        // uses real user content to keep testing the general "non-local
        // denied" behavior.
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "user:private-note"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn unauthenticated_requester_treated_as_non_local() {
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "user:private-note"}),
            None,
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn open_kb_never_denied() {
        let editor = Editor::new(); // primary defaults to Open
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn federated_scan_filterable_tool_gate_allows_defers_to_tool_filter() {
        // kb_search/kb_search_context/kb_vector_search/kb_agenda are all
        // ScopedFederatedScanFilterable (#358, and kb_vector_search joined
        // them in ADR-061 Phase F1 once it got real results to filter;
        // kb_agenda joined in ADR-083 once its own federation gap was
        // fixed) -- the gate no longer denies the whole call when a KB in
        // scope is restricted; each tool impl post-filters its own
        // materialized results instead (see crates/ai/src/tool_impls/kb.rs's
        // behavioral tests for the actual filtering coverage).
        let editor = editor_with_restricted_primary();
        for tool in [
            "kb_search",
            "kb_search_context",
            "kb_vector_search",
            "kb_agenda",
        ] {
            let decision =
                check_kb_residency(&editor, tool, &serde_json::json!({}), Some("claude"));
            assert_eq!(decision, ResidencyDecision::Allow, "tool: {tool}");
        }
    }

    #[test]
    fn federated_scan_tool_allowed_when_nothing_restricted() {
        let editor = Editor::new();
        let decision =
            check_kb_residency(&editor, "kb_agenda", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn non_content_kb_tool_never_gated() {
        // kb_instances is meta/admin, not content — never denied regardless of
        // policy or provider.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_instances",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn literal_primary_reference_is_checked() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_restore",
            &serde_json::json!({"id": "primary"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn unknown_node_id_is_not_this_gates_problem() {
        // A nonexistent node id can't be resolved to any KB — this gate allows
        // it through so the underlying tool can report its own "no such node"
        // error, rather than this gate masking it with a confusing denial.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "no:such:node"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: classification architecture (#350/#351 follow-up) ---

    #[test]
    fn every_kb_tool_and_help_open_is_explicitly_classified() {
        // Prevents the #350/#351-adjacent drift class: a new kb_*/help_open
        // AI tool added without updating this gate used to silently fall
        // through to Allow. This test catches it at CI time with a clear,
        // actionable failure; check_kb_residency's runtime fail-closed
        // default (see `unclassified_kb_prefixed_tool_denied_conservatively`)
        // is the defense-in-depth backstop if this test is ever skipped.
        let editor = Editor::new();
        let tools = mae_ai::tools::ai_specific_tools(&editor.option_registry);
        let unclassified: Vec<&str> = tools
            .iter()
            .map(|t| t.name.as_str())
            .filter(|n| n.starts_with("kb_") || *n == "help_open")
            .filter(|n| classify_kb_tool(n).is_none())
            .collect();
        assert!(
            unclassified.is_empty(),
            "kb_*/help_open tools with no explicit residency classification \
             in classify_kb_tool: {unclassified:?}"
        );
    }

    #[test]
    fn unclassified_kb_prefixed_tool_denied_conservatively() {
        let editor = editor_with_restricted_primary();
        // Not a real tool name -- simulates a brand-new kb_* tool nobody has
        // classified yet. Must fail closed, not silently Allow.
        let decision = check_kb_residency(
            &editor,
            "kb_totally_new_tool",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
        // And an unrelated (non-kb_, non-help_open) tool name is unaffected —
        // this gate only concerns itself with kb_*/help_open.
        assert_eq!(
            check_kb_residency(
                &editor,
                "buffer_read",
                &serde_json::json!({}),
                Some("claude")
            ),
            ResidencyDecision::Allow
        );
    }

    // --- New: #351 fix — scope-aware ScopedFederatedScanFilterable ---
    //
    // The original #351 fix shipped as a dedicated `any_restricted_kb_label_in_scope`
    // gate-level pre-check (hard-deny the whole call if the resolved scope included a
    // restricted KB), used only by the now-removed `ScopedFederatedScan` shape.
    // ADR-061 Phase F1 moved `kb_vector_search` -- the last real tool in that shape --
    // to `ScopedFederatedScanFilterable` once un-stubbing it gave it real per-node
    // results to post-filter, leaving the pre-check with zero callers. Removed rather
    // than kept as dead code (principle #15): the #351 property itself (a call scoped
    // away from a restricted KB isn't blocked by that KB's policy) is preserved by the
    // Filterable path's own design -- `kb_federated_search_scoped(query, scope)`
    // already only includes KBs within the resolved scope, and per-node
    // `filter_residency_exempt` then drops non-exempt content from whichever
    // restricted KB genuinely IS in scope -- which is more precise than the old
    // all-or-nothing gate-level deny, not a regression.

    // --- New: kb_agenda gate-level coverage (ScopedFederatedScanFilterable, ADR-083) ---

    #[test]
    fn kb_agenda_unrelated_restricted_instance_does_not_block() {
        // At the GATE level, ScopedFederatedScanFilterable allows the call
        // through unconditionally regardless of what's registered -- scope
        // narrowing now happens inside execute_kb_agenda's own per-instance
        // registry loop (mirroring execute_kb_health's), not here. An
        // unrelated restricted federated instance existing at all must
        // never block the call outright.
        let mut editor = Editor::new(); // open primary
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));

        let decision =
            check_kb_residency(&editor, "kb_agenda", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_agenda_gate_allows_when_primary_restricted_defers_to_tool_filter() {
        // kb_agenda is ScopedFederatedScanFilterable (ADR-083) -- the gate
        // no longer denies the whole call when primary is restricted;
        // execute_kb_agenda post-filters its own materialized
        // (Option<String>, Node) results instead (see
        // crates/ai/src/tool_impls/kb.rs's behavioral tests).
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_agenda", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: PrimaryOnly bucket (kb_raw_query/kb_view_query) ---

    #[test]
    fn kb_raw_query_denied_when_primary_restricted() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_raw_query",
            &serde_json::json!({"query": "?[id] := *nodes{id}"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_view_query_denied_when_primary_restricted() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_view_query",
            &serde_json::json!({"view_id": "view:kanban"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    // --- New: ADR-061 Phase E, kb_enrich (PrimaryOnly bucket) ---

    #[test]
    fn kb_enrich_denied_when_primary_restricted_and_requester_is_not_local() {
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_enrich", &serde_json::json!({}), Some("claude"));
        assert!(
            matches!(decision, ResidencyDecision::Deny(_)),
            "a non-local requester must not even be able to call kb_enrich against a \
             LocalModelsOnly primary KB, regardless of what embedding provider it configures"
        );
    }

    #[test]
    fn kb_enrich_allowed_when_requester_is_a_local_provider() {
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_enrich", &serde_json::json!({}), Some("ollama"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_enrich_allowed_when_primary_is_unrestricted() {
        let editor = Editor::new();
        let decision =
            check_kb_residency(&editor, "kb_enrich", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: UnscopedFederatedContent bucket ---

    #[test]
    fn kb_graph_gate_allows_when_any_kb_restricted_defers_to_tool_filter() {
        // kb_graph is now UnscopedFederatedContentFilterable (#361) -- the
        // gate no longer denies the whole call when a registered KB is
        // restricted; execute_kb_graph post-filters its own materialized
        // per-node BFS results instead (see crates/ai/src/tool_impls/kb.rs's
        // behavioral tests for the actual filtering coverage).
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision = check_kb_residency(
            &editor,
            "kb_graph",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_list_denied_outright_when_any_kb_restricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision =
            check_kb_residency(&editor, "kb_list", &serde_json::json!({}), Some("claude"));
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_health_gate_allows_when_any_kb_restricted_defers_to_tool_filter() {
        // kb_health is now UnscopedFederatedContentFilterable (#361) -- the
        // gate no longer denies the whole call when a registered KB is
        // restricted; execute_kb_health post-filters each KB's health
        // report independently instead (see crates/ai/src/tool_impls/kb.rs's
        // behavioral tests for the actual filtering coverage).
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision =
            check_kb_residency(&editor, "kb_health", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_id_audit_denied_outright_when_any_kb_restricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision = check_kb_residency(
            &editor,
            "kb_id_audit",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_links_to_allowed_at_the_gate_when_anchor_is_unrestricted() {
        // #366: kb_links_to is now SingleTargetFilterable -- the GATE only
        // checks the anchor "id" itself (same as kb_related/kb_neighborhood);
        // a restricted-instance backlink SOURCE is no longer a reason to
        // deny the whole call, since execute_kb_links_to now post-filters
        // its own aggregated backlink list for the seed exemption (see
        // crates/ai/src/tool_impls/kb.rs's behavioral coverage for that).
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision = check_kb_residency(
            &editor,
            "kb_links_to",
            &serde_json::json!({"id": "index"}), // "index" itself is unrestricted
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: omission-fix regressions ---

    #[test]
    fn kb_history_denied_like_kb_restore() {
        // "index" itself is seed content and is now correctly exempt
        // (#358) -- use a real user node to keep testing the general
        // SingleTarget deny behavior.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_history",
            &serde_json::json!({"id": "user:private-note"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_history_and_kb_restore_seed_content_allowed_when_primary_restricted() {
        // #361 correction: kb_history/kb_restore need no new plumbing --
        // they were already SingleTarget with "id" in TARGET_ARG_KEYS, so
        // resolve_restricted_label's existing seed-exemption check already
        // covers them (their result is version metadata for the SAME id;
        // there's no other-node traversal-leak vector to post-filter).
        let editor = editor_with_restricted_primary();
        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_history",
                &serde_json::json!({"id": "index"}),
                Some("claude")
            ),
            ResidencyDecision::Allow,
            "seeded content's history must stay reachable from a restricted primary"
        );
        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_restore",
                &serde_json::json!({"id": "index", "version": 1}),
                Some("claude")
            ),
            ResidencyDecision::Allow,
            "restoring seeded content must stay reachable from a restricted primary"
        );
    }

    #[test]
    fn kb_preview_show_denied_like_kb_get() {
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_preview_show",
            &serde_json::json!({"id": "user:private-note"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_reimport_denied_when_named_instance_restricted() {
        // kb_reimport's target arg key is "name", not "id"/"kb" -- exercises
        // the TARGET_ARG_KEYS extension this fix required.
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision = check_kb_residency(
            &editor,
            "kb_reimport",
            &serde_json::json!({"name": "RestrictedInstance"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn help_open_denied_when_target_kb_restricted() {
        // help_open used to be structurally excluded from ever being gated
        // (the old arrays only ever held "kb_*" strings). Uses a real user
        // node -- "index" itself is seed content and is now correctly
        // exempt (#358), see `help_open_seed_content_allowed_when_primary_restricted`.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "help_open",
            &serde_json::json!({"id": "user:private-note"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    // --- New: seed-content exemption (#358) ---

    #[test]
    fn help_open_seed_content_allowed_when_primary_restricted() {
        // The literal #358 repro: an AI agent must still be able to reach
        // MAE's own built-in help system even when primary is restricted
        // to protect a user's own notes.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "help_open",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert_eq!(
            decision,
            ResidencyDecision::Allow,
            "seeded built-in content must stay reachable even when primary is restricted"
        );
    }

    #[test]
    fn kb_get_seed_content_allowed_when_primary_restricted() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn single_target_node_id_resolves_to_restricted_kb_regardless_of_source() {
        // The critical negative case: a genuinely user-authored node in a
        // restricted primary must still be denied -- the seed exemption
        // must not over-broaden to "everything in primary."
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        // Sanity: seed content in the SAME restricted primary is allowed...
        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_get",
                &serde_json::json!({"id": "index"}),
                Some("claude")
            ),
            ResidencyDecision::Allow
        );
        // ...but the non-seed node is not.
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "user:private-note"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    // --- kb_export_subgraph_html ---

    #[test]
    fn kb_export_subgraph_html_seed_content_allowed_when_primary_restricted() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_export_subgraph_html",
            &serde_json::json!({"id": "index", "path": "/tmp/out.html"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_export_subgraph_html_denied_for_a_non_seed_node_in_a_restricted_kb() {
        // The adversarial case this gate exists for: a real, user-authored
        // node in a residency-restricted KB must not be exportable by a
        // non-local provider just because the tool also happens to take a
        // "path" argument -- same gate, same seed-exemption boundary as
        // plain kb_get.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_export_subgraph_html",
            &serde_json::json!({"id": "user:private-note", "path": "/tmp/out.html"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_export_subgraph_html_allowed_from_a_local_provider_regardless() {
        let mut editor = editor_with_restricted_primary();
        editor
            .kb_create_node(
                "user:private-note",
                "Private",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();
        let decision = check_kb_residency(
            &editor,
            "kb_export_subgraph_html",
            &serde_json::json!({"id": "user:private-note", "path": "/tmp/out.html"}),
            Some("ollama"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: NonContent view-state tools stay ungated ---

    #[test]
    fn graph_view_state_manipulation_tools_never_gated() {
        let editor = editor_with_restricted_primary();
        for tool in [
            "kb_graph_view_close",
            "kb_graph_view_navigate",
            "kb_graph_view_select_current",
            "kb_graph_view_zoom_to",
            "kb_graph_view_set_pinned",
            "kb_graph_view_toggle_overlay",
            "kb_graph_view_set_depth",
        ] {
            assert_eq!(
                check_kb_residency(&editor, tool, &serde_json::json!({}), Some("claude")),
                ResidencyDecision::Allow,
                "{tool} must never be gated (pure view-state manipulation)"
            );
        }
    }

    #[test]
    fn membership_and_policy_lifecycle_tools_never_gated() {
        let editor = editor_with_restricted_primary();
        for tool in [
            "kb_sharing_status",
            "kb_share",
            "kb_share_p2p",
            "kb_join",
            "kb_join_p2p",
            "kb_leave",
            "kb_add_member",
            "kb_remove_member",
            "kb_block_member",
            "kb_unblock_member",
            "kb_approve",
            "kb_set_policy",
            "kb_set_encryption",
            "kb_set_ai_residency",
            "kb_register",
            "kb_unregister",
            "kb_preview_dismiss",
            "kb_sync_status",
        ] {
            assert_eq!(
                check_kb_residency(&editor, tool, &serde_json::json!({}), Some("claude")),
                ResidencyDecision::Allow,
                "{tool} must never be gated (administrative/lifecycle, not content)"
            );
        }
    }

    // --- kb_export_guidance (GuidanceKbTarget) — the target is resolved
    // from editor state (`ai_guidance_kb`), not `arguments`, so this needs
    // its own coverage distinct from SingleTarget's arg-based tests above.

    #[test]
    fn export_guidance_allowed_when_ai_guidance_kb_is_unset() {
        let editor = editor_with_restricted_primary(); // restricted primary is irrelevant here
        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_export_guidance",
                &serde_json::json!({}),
                Some("claude")
            ),
            ResidencyDecision::Allow,
            "nothing configured to export means nothing to leak"
        );
    }

    #[test]
    fn export_guidance_denied_for_a_non_local_provider_when_the_guidance_kb_is_restricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("TeamSecrets", "uuid-secrets"));
        editor.ai_guidance_kb = "TeamSecrets".to_string();

        let decision = check_kb_residency(
            &editor,
            "kb_export_guidance",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert!(
            matches!(decision, ResidencyDecision::Deny(_)),
            "a restricted guidance KB must deny export to a non-local provider, got: {decision:?}"
        );
    }

    #[test]
    fn export_guidance_allowed_for_a_local_provider_even_when_the_guidance_kb_is_restricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("TeamSecrets", "uuid-secrets"));
        editor.ai_guidance_kb = "TeamSecrets".to_string();

        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_export_guidance",
                &serde_json::json!({}),
                Some("ollama")
            ),
            ResidencyDecision::Allow
        );
    }

    #[test]
    fn export_guidance_allowed_when_the_guidance_kb_is_unrestricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("PublicDocs", "uuid-public"));
        editor.ai_guidance_kb = "PublicDocs".to_string();

        assert_eq!(
            check_kb_residency(
                &editor,
                "kb_export_guidance",
                &serde_json::json!({}),
                Some("claude")
            ),
            ResidencyDecision::Allow
        );
    }

    #[test]
    fn export_guidance_denied_when_the_restricted_primary_is_the_guidance_kb() {
        // "primary" is a valid ai_guidance_kb value too (not yet wired in
        // guidance.rs's reader per its own option doc, but the residency
        // gate must fail closed regardless of whether the read path
        // currently resolves it -- a future wiring fix must not silently
        // inherit an ungated path).
        let mut editor = editor_with_restricted_primary();
        editor.ai_guidance_kb = "primary".to_string();

        let decision = check_kb_residency(
            &editor,
            "kb_export_guidance",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    // --- #478: eval_scheme must not bypass residency filtering for KB-reading
    // Scheme primitives. `check_kb_residency` is the exact function BOTH real
    // call sites (`ai_event_handler::handle_ai_event:172` embedded,
    // `handle_mcp_request:895` external-MCP) invoke unchanged with this same
    // (editor, tool_name, arguments, requester_provider) shape -- there is no
    // divergent logic between them for this decision, so testing the function
    // directly here exercises the real fix both call sites rely on, not a
    // mock. `handle_mcp_request` (895) additionally gets a full event-handler
    // dispatch test below, proving the wiring itself; `handle_ai_event` (172)
    // has no existing test harness at all (its `AiEventContext` needs live
    // mpsc channels + an `McpClientMgrRef` with no prior test precedent to
    // build one cheaply) -- not invented here since it would only re-prove
    // what this direct test already covers for a second time via heavier
    // scaffolding.

    #[test]
    fn eval_scheme_denies_the_exact_reported_bypass() {
        // The literal repro from #478's own report.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(kb-graph-view-state)"}),
            Some("claude"),
        );
        assert!(
            matches!(decision, ResidencyDecision::Deny(_)),
            "eval_scheme calling kb-graph-view-state must be denied when primary is \
             restricted, got: {decision:?}"
        );
    }

    #[test]
    fn eval_scheme_denies_a_second_sensitive_primitive_not_in_the_original_report() {
        // #478 explicitly flagged this as "very likely true of other
        // kb_*-reading Scheme primitives too" -- prove the fix covers more
        // than just the one primitive the original report happened to name.
        // kb-get-block returns raw node body text, one of the highest-
        // sensitivity primitives in the inventory and one with no MCP
        // sibling to have borrowed a classification from.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(kb-get-block \"index\" 0)"}),
            Some("claude"),
        );
        assert!(
            matches!(decision, ResidencyDecision::Deny(_)),
            "eval_scheme calling kb-get-block must be denied when primary is restricted, \
             got: {decision:?}"
        );
    }

    #[test]
    fn eval_scheme_denies_when_the_restricted_kb_is_a_registered_instance_not_primary() {
        // The bypass isn't primary-only -- a restricted federated instance
        // must trigger the same denial.
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("Private", "uuid-private"));
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(kb-neighborhood \"n1\" 2)"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn eval_scheme_allows_unrelated_code_even_with_a_restricted_kb() {
        // No false-positive: an eval_scheme call that never references any
        // sensitive primitive must still work normally, even while a KB is
        // restricted -- the gate must not become a blanket eval_scheme ban.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(+ 1 2)"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn eval_scheme_allows_sensitive_primitives_when_nothing_is_restricted() {
        // No false-positive on the other axis: with no restricted KB at all,
        // the exact same code that would be denied above must be allowed --
        // proves the gate is conditioned on actual residency policy, not a
        // static deny-list applied unconditionally.
        let editor = Editor::new();
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(kb-graph-view-state)"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn eval_scheme_allows_a_local_provider_even_with_sensitive_code_and_a_restriction() {
        // Local (Ollama) providers are exempt from AI-residency policy
        // entirely (that's the whole point of LocalModelsOnly) -- must still
        // hold for the eval_scheme path specifically, not just the kb_* tools.
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "eval_scheme",
            &serde_json::json!({"code": "(kb-graph-view-state)"}),
            Some("ollama"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[tokio::test]
    async fn eval_scheme_bypass_denied_end_to_end_via_the_real_external_mcp_dispatch_path() {
        // Real event-handler wiring test (not just the classifier function in
        // isolation) through `handle_mcp_request` -- the external-MCP call
        // site (`ai_event_handler.rs:895`). Proves the actual bypass #478
        // reported no longer works through the real dispatch path an
        // external MCP client would use, including the psk_authenticated
        // gating on `requester_provider`.
        let mut editor = editor_with_restricted_primary();
        let mut scheme = mae_scheme::SchemeRuntime::new().unwrap();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let req = mae_mcp::McpToolRequest {
            tool_name: "eval_scheme".to_string(),
            arguments: serde_json::json!({"code": "(kb-graph-view-state)"}),
            reply: tx,
            requester: mae_mcp::RequesterContext {
                session_id: 1,
                psk_authenticated: true,
                declared_provider: Some("claude".to_string()),
                ..Default::default()
            },
        };
        let global_policy = mae_ai::PermissionPolicy {
            auto_approve_up_to: mae_ai::PermissionTier::Shell,
            ..mae_ai::PermissionPolicy::default()
        };
        let (lsp_tx, _lsp_rx) = tokio::sync::mpsc::channel(1);
        let mut deferred = Vec::new();
        crate::ai_event_handler::handle_mcp_request(
            &mut editor,
            req,
            &[],
            &global_policy,
            &lsp_tx,
            &mut deferred,
            &mut scheme,
        );
        let result = rx.try_recv().expect("reply must have been sent");
        assert!(
            !result.success,
            "the real external-MCP dispatch path must deny the eval_scheme bypass, got \
             success with output: {}",
            result.output
        );
        assert!(
            result.output.contains("local_models_only"),
            "expected an AI-residency denial message, got: {}",
            result.output
        );
    }
}
