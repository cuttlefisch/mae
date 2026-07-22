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
//! [`ToolResidencyShape::ScopedFederatedScan`]/[`ToolResidencyShape::ScopedFederatedScanFilterable`]
//! tools (`kb_vector_search`; `kb_search`/`kb_search_context`, respectively)
//! are the escape hatch from that coarseness: they accept a `scope` argument
//! (or fall back to the `kb_search_scope` option) that names exactly which
//! KB(s) participate, so the residency check can — and now does — restrict
//! itself to that resolved scope instead of every registered KB (this is the
//! actual #351 fix; see `any_restricted_kb_label_in_scope`).
//!
//! ## Seed-content exemption (#358)
//!
//! `SingleTarget`, `PrimaryOnlyFilterable`, and `ScopedFederatedScanFilterable`
//! tools exempt MAE's own seeded/built-in content (`Node::source ==
//! Some(NodeSource::Seed)`, stamped once at startup, identical on every
//! install, never sensitive) from `LocalModelsOnly` gating even when it lives
//! in a restricted KB — restricting `primary` to protect a user's own notes
//! must not also lock an AI agent out of MAE's own built-in help system. The
//! filter primitives (`is_residency_exempt`, `filter_residency_exempt`,
//! `filter_residency_exempt_primary`) live in `mae_core::ai_residency`
//! rather than here — a Rust crate-graph constraint (the `mae` package has
//! no `[lib]` target, so nothing in `mae-ai`'s tool implementations can
//! reach this file), not a conceptual split. `SingleTarget` applies the
//! exemption directly in `resolve_restricted_label` (the node is already
//! resolved there); the two `*Filterable` shapes allow the call through
//! unconditionally and rely on the tool implementation
//! (`execute_kb_agenda`/`execute_kb_search`/`execute_kb_search_context` in
//! `crates/ai/src/tool_impls/kb.rs`) to post-filter its own materialized
//! results — see each shape's doc comment.
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
//! `kb_list`, `kb_links_to`, `kb_shortest_path`, `kb_links_from` are real,
//! structurally feasible candidates for the same exemption but need deeper
//! plumbing (shared trait extensions, new per-id lookups, or a Datalog
//! query change) — tracked as a follow-up rather than silently left as a
//! gap, see the issue cross-linked from #358.

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
    /// against `primary_ai_residency` only, hard-denied outright. Distinct
    /// from [`Self::PrimaryOnlyFilterable`] below.
    PrimaryOnly,
    /// Only ever touches the primary store, but its results ARE real
    /// `Node`s the tool impl can post-filter — the gate allows the call
    /// through; `execute_kb_agenda` calls
    /// `mae_core::ai_residency::filter_residency_exempt_primary` on its own
    /// materialized results (#358).
    PrimaryOnlyFilterable,
    /// Scans across multiple KB instances AND accepts a `scope` argument (or
    /// falls back to the `kb_search_scope` option) that can exclude a
    /// specific instance — scope is resolved FIRST, then residency is
    /// checked only for KBs within that resolved scope (the #351 fix). Has
    /// no per-result filtering today — hard-denied when scope includes a
    /// restricted KB. Distinct from [`Self::ScopedFederatedScanFilterable`]
    /// below; currently only `kb_vector_search` (a permanent stub with no
    /// real results to filter yet).
    ScopedFederatedScan,
    /// Scans across multiple KB instances via a `scope` arg AND its results
    /// ARE real `(Option<String>, Node)` pairs — the gate allows the call
    /// through; `kb_search`/`kb_search_context` call
    /// `mae_core::ai_residency::filter_residency_exempt` on their own
    /// materialized results (#358).
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
        | "kb_links_from" | "kb_shortest_path" | "kb_history" | "kb_preview_show"
        | "kb_create" | "kb_set_role" | "kb_reimport" | "help_open" => SingleTarget,

        // --- SingleTargetFilterable: same anchor-id gate check as
        // SingleTarget, PLUS the tool impl post-filters its own multi-node
        // traversal results (#361 -- see the shape's doc comment) ---
        "kb_related" | "kb_neighborhood" => SingleTargetFilterable,

        // --- PrimaryOnly: implementation only ever reads editor.kb.store,
        // AND runs arbitrary Datalog with no per-row node-identity to
        // filter -- structurally incapable of the seed exemption (#358) ---
        "kb_raw_query" | "kb_view_query" => PrimaryOnly,

        // --- PrimaryOnlyFilterable: implementation only ever reads
        // editor.kb.store, and returns real Node results the tool impl
        // post-filters for the seed exemption (#358) ---
        "kb_agenda" => PrimaryOnlyFilterable,

        // --- ScopedFederatedScan: has a `scope` argument that names
        // exactly which KB(s) participate, but no real results to filter
        // yet -- kb_vector_search is a permanent stub today (no embedding
        // provider wired). Move it to ScopedFederatedScanFilterable
        // alongside whatever work actually implements ranked vector
        // search, not before. ---
        "kb_vector_search" => ScopedFederatedScan,

        // --- ScopedFederatedScanFilterable: same scope-narrowing as above,
        // AND the tool impl post-filters its real (Option<String>, Node)
        // results for the seed exemption (#358) ---
        "kb_search" | "kb_search_context" => ScopedFederatedScanFilterable,

        // --- UnscopedFederatedContent: genuinely scans multiple instances,
        // no scope argument to narrow it ---
        "kb_graph_view_open" | "kb_graph_view_refresh" | "kb_list" | "kb_id_audit"
        | "kb_links_to" => UnscopedFederatedContent,

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

        _ => return None,
    })
}

/// Result of an AI-residency check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidencyDecision {
    Allow,
    Deny(String),
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

        // The gate allows the call through unconditionally; the tool impl
        // (execute_kb_agenda) post-filters its own materialized Node
        // results via mae_core::ai_residency::filter_residency_exempt_primary,
        // dropping non-seed-exempt hits from a restricted primary (#358).
        ToolResidencyShape::PrimaryOnlyFilterable => ResidencyDecision::Allow,

        ToolResidencyShape::ScopedFederatedScan => {
            let scope_arg = arguments.get("scope").and_then(|v| v.as_str());
            if let Some(label) = any_restricted_kb_label_in_scope(editor, scope_arg) {
                return ResidencyDecision::Deny(format!(
                    "AI-residency policy: KB '{label}' is set to local_models_only, and this \
                     session's AI provider ({}) isn't a local model. Use an explicit `scope` \
                     argument that excludes it, or switch to a local (Ollama) provider.",
                    requester_provider.unwrap_or("none/unauthenticated")
                ));
            }
            ResidencyDecision::Allow
        }

        // Same scope-narrowing intent as ScopedFederatedScan, but the gate
        // allows the call through unconditionally; execute_kb_search/
        // execute_kb_search_context post-filter their own materialized
        // (Option<String>, Node) results via
        // mae_core::ai_residency::filter_residency_exempt (#358).
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

/// Like [`any_restricted_kb_label`], but resolves `scope_arg` (falling back
/// to the `kb_search_scope` option, mirroring
/// `crates/ai/src/tool_impls/kb.rs::execute_kb_search`'s own resolution
/// exactly) FIRST, and only checks residency for KBs within that resolved
/// scope — the actual #351 fix. A call explicitly scoped away from a
/// restricted KB must not be blocked by that KB's policy.
fn any_restricted_kb_label_in_scope(editor: &Editor, scope_arg: Option<&str>) -> Option<String> {
    let scope = scope_arg
        .filter(|s| !s.is_empty())
        .map(mae_kb::KbScope::parse)
        .unwrap_or_else(|| mae_kb::KbScope::parse(&editor.kb.search_scope));

    let is_restricted = |ai_residency: mae_kb::federation::AiResidency| {
        ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly
    };

    match scope {
        mae_kb::KbScope::All => any_restricted_kb_label(editor),
        mae_kb::KbScope::LocalOnly => {
            is_restricted(editor.kb.registry.primary_ai_residency).then(|| "primary".to_string())
        }
        mae_kb::KbScope::RemoteOnly => editor
            .kb
            .registry
            .instances
            .iter()
            .filter(|inst| inst.is_remote())
            .find(|inst| is_restricted(inst.ai_residency))
            .map(|inst| inst.name.clone()),
        mae_kb::KbScope::Named(name) => {
            if name.eq_ignore_ascii_case("primary") {
                is_restricted(editor.kb.registry.primary_ai_residency)
                    .then(|| "primary".to_string())
            } else {
                editor
                    .kb
                    .registry
                    .find(&name)
                    .filter(|inst| is_restricted(inst.ai_residency))
                    .map(|inst| inst.name.clone())
            }
        }
    }
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
        // kb_search is now ScopedFederatedScanFilterable (#358) -- the gate
        // no longer denies the whole call when a KB in scope is restricted;
        // execute_kb_search post-filters its own materialized results
        // instead (see crates/ai/src/tool_impls/kb.rs's behavioral tests
        // for the actual filtering coverage). kb_vector_search (still plain
        // ScopedFederatedScan, no real results to filter yet) keeps the
        // old hard-deny behavior -- see
        // `plain_scoped_federated_scan_tool_still_denied_outright`.
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_search", &serde_json::json!({}), Some("claude"));
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn plain_scoped_federated_scan_tool_still_denied_outright() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
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

    // --- New: #351 fix — scope-aware ScopedFederatedScan ---

    #[test]
    fn kb_search_scope_excludes_restricted_kb_when_scope_names_an_open_instance() {
        // The actual #351 repro, inverted to must-pass: primary is
        // restricted, but scope explicitly names a different, open
        // instance. Retargeted onto kb_vector_search (#358) -- kb_search
        // itself no longer uses this gate path (see
        // `federated_scan_filterable_tool_gate_allows_defers_to_tool_filter`),
        // so real any_restricted_kb_label_in_scope coverage moves to the
        // one remaining plain ScopedFederatedScan tool.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("OpenInstance", "uuid-open"));

        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({"query": "x", "scope": "OpenInstance"}),
            Some("claude"),
        );
        assert_eq!(
            decision,
            ResidencyDecision::Allow,
            "scope excludes the restricted primary -- must not be denied"
        );
    }

    #[test]
    fn kb_search_scope_named_restricted_instance_is_still_denied() {
        // Retargeted onto kb_vector_search (#358) -- kb_search itself is now
        // ScopedFederatedScanFilterable and no longer denies at the gate
        // (see `federated_scan_filterable_tool_gate_allows_defers_to_tool_filter`),
        // but any_restricted_kb_label_in_scope still needs real coverage
        // via the one remaining plain ScopedFederatedScan tool.
        let mut editor = Editor::new(); // open primary
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));

        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({"query": "x", "scope": "RestrictedInstance"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_search_scope_all_still_denied_when_any_kb_restricted() {
        // Regression guard: unscoped ("all") behavior is unchanged for the
        // still-hard-denied kb_vector_search -- still the conservative
        // deny-outright per the module doc's "Scope note".
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));

        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({"query": "x", "scope": "all"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_search_scope_local_only_ignores_a_restricted_remote_instance() {
        // Retargeted onto kb_vector_search (#358), same reasoning as above.
        let mut editor = Editor::new(); // open primary
        let mut remote = restricted_instance("RemoteRestricted", "uuid-remote");
        remote.shared = true; // is_remote() == true
        editor.kb.registry.instances.push(remote);

        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({"query": "x", "scope": "local"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_search_missing_scope_falls_back_to_the_search_scope_option() {
        // No `scope` arg given at all -- must resolve via kb.search_scope,
        // exactly like execute_kb_search's own default resolution.
        // Retargeted onto kb_vector_search (#358), same reasoning as above.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("OpenInstance", "uuid-open"));
        editor.kb.search_scope = "OpenInstance".to_string();

        let decision = check_kb_residency(
            &editor,
            "kb_vector_search",
            &serde_json::json!({"query": "x"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    // --- New: kb_agenda's inverse bug (PrimaryOnly, not UnscopedFederatedContent) ---

    #[test]
    fn kb_agenda_unrelated_restricted_instance_does_not_block() {
        // kb_agenda only ever reads editor.kb.store (primary) -- an
        // unrelated restricted federated instance must not block it.
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
        // kb_agenda is now PrimaryOnlyFilterable (#358) -- the gate no
        // longer denies the whole call when primary is restricted;
        // execute_kb_agenda post-filters its own materialized Node results
        // instead (see crates/ai/src/tool_impls/kb.rs's behavioral tests).
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
    fn kb_links_to_denied_outright_when_any_kb_restricted() {
        // kb_links_to used to be SingleTarget, checking only the *target*
        // id's home KB -- but it aggregates backlink *sources* across every
        // federated instance, so a restricted instance's backlink could leak
        // via an unrestricted target. Now UnscopedFederatedContent.
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
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
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
}
