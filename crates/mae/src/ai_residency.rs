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
//! [`ToolResidencyShape::ScopedFederatedScan`] tools (`kb_search`,
//! `kb_search_context`, `kb_vector_search`) are the escape hatch from that
//! coarseness: they accept a `scope` argument (or fall back to the
//! `kb_search_scope` option) that names exactly which KB(s) participate, so the
//! residency check can — and now does — restrict itself to that resolved scope
//! instead of every registered KB (this is the actual #351 fix; see
//! `any_restricted_kb_label_in_scope`).

use mae_core::Editor;

/// AI provider names MAE classifies as "local" for residency purposes.
const LOCAL_AI_PROVIDERS: &[&str] = &["ollama"];

/// Argument keys, across [`ToolResidencyShape::SingleTarget`] tools, that hold
/// a node id or an explicit KB instance name/uuid worth resolving.
const TARGET_ARG_KEYS: &[&str] = &["id", "src", "dst", "from", "to", "kb", "name"];

/// How a `kb_*`/`help_open` tool's content exposure relates to AI-residency
/// policy — see the module doc for why this replaced two hand-maintained
/// arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolResidencyShape {
    /// Resolves to exactly one (or two) specific node id(s)/KB name(s) in
    /// `arguments` — checked precisely via [`TARGET_ARG_KEYS`].
    SingleTarget,
    /// Only ever touches the primary store (`editor.kb.store`), never a
    /// federated instance — checked against `primary_ai_residency` only.
    PrimaryOnly,
    /// Scans across multiple KB instances AND accepts a `scope` argument (or
    /// falls back to the `kb_search_scope` option) that can exclude a
    /// specific instance — scope is resolved FIRST, then residency is
    /// checked only for KBs within that resolved scope (the #351 fix).
    ScopedFederatedScan,
    /// Scans across multiple KB instances with no way to exclude one —
    /// denied outright whenever ANY registered KB (or primary) is
    /// restricted (see the module doc's "Scope note").
    UnscopedFederatedContent,
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
        | "kb_links_from" | "kb_related" | "kb_shortest_path" | "kb_neighborhood"
        | "kb_history" | "kb_preview_show" | "kb_create" | "kb_set_role" | "kb_reimport"
        | "help_open" => SingleTarget,

        // --- PrimaryOnly: implementation only ever reads editor.kb.store ---
        "kb_agenda" | "kb_raw_query" | "kb_view_query" => PrimaryOnly,

        // --- ScopedFederatedScan: has (or, for kb_vector_search, will have)
        // a `scope` argument that names exactly which KB(s) participate ---
        "kb_search" | "kb_search_context" | "kb_vector_search" => ScopedFederatedScan,

        // --- UnscopedFederatedContent: genuinely scans multiple instances,
        // no scope argument to narrow it ---
        "kb_graph" | "kb_graph_view_open" | "kb_graph_view_refresh" | "kb_graph_view_state"
        | "kb_list" | "kb_health" | "kb_id_audit" | "kb_links_to" => UnscopedFederatedContent,

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

/// Is `provider` one MAE classifies as local (self-hosted)?
pub fn is_local_provider(provider: &str) -> bool {
    LOCAL_AI_PROVIDERS.contains(&provider)
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

        ToolResidencyShape::SingleTarget => {
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
    // instance) actually contains this id?
    if editor.kb.primary.get(value).is_some() {
        if editor.kb.registry.primary_ai_residency
            == mae_kb::federation::AiResidency::LocalModelsOnly
        {
            return Some("primary".to_string());
        }
        return None;
    }
    for (uuid, kb) in editor.kb.instances.iter() {
        if kb.get(value).is_some() {
            if let Some(inst) = editor.kb.registry.find_by_uuid(uuid) {
                if inst.ai_residency == mae_kb::federation::AiResidency::LocalModelsOnly {
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
        let editor = editor_with_restricted_primary();
        // "index" is seeded into the primary KB by default (seed_kb).
        let decision = check_kb_residency(
            &editor,
            "kb_get",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn unauthenticated_requester_treated_as_non_local() {
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_get", &serde_json::json!({"id": "index"}), None);
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
    fn federated_scan_tool_denied_outright_when_scope_not_given_and_any_kb_restricted() {
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_search", &serde_json::json!({}), Some("claude"));
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
        // restricted, but scope explicitly names a different, open instance.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("OpenInstance", "uuid-open"));

        let decision = check_kb_residency(
            &editor,
            "kb_search",
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
        let mut editor = Editor::new(); // open primary
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));

        let decision = check_kb_residency(
            &editor,
            "kb_search",
            &serde_json::json!({"query": "x", "scope": "RestrictedInstance"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_search_scope_all_still_denied_when_any_kb_restricted() {
        // Regression guard: unscoped ("all") behavior is unchanged — still
        // the conservative deny-outright per the module doc's "Scope note".
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));

        let decision = check_kb_residency(
            &editor,
            "kb_search",
            &serde_json::json!({"query": "x", "scope": "all"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_search_scope_local_only_ignores_a_restricted_remote_instance() {
        let mut editor = Editor::new(); // open primary
        let mut remote = restricted_instance("RemoteRestricted", "uuid-remote");
        remote.shared = true; // is_remote() == true
        editor.kb.registry.instances.push(remote);

        let decision = check_kb_residency(
            &editor,
            "kb_search",
            &serde_json::json!({"query": "x", "scope": "local"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_search_context_scope_excludes_restricted_kb() {
        let mut editor = editor_with_restricted_primary();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("OpenInstance", "uuid-open"));

        let decision = check_kb_residency(
            &editor,
            "kb_search_context",
            &serde_json::json!({"query": "x", "scope": "OpenInstance"}),
            Some("claude"),
        );
        assert_eq!(decision, ResidencyDecision::Allow);
    }

    #[test]
    fn kb_search_missing_scope_falls_back_to_the_search_scope_option() {
        // No `scope` arg given at all -- must resolve via kb.search_scope,
        // exactly like execute_kb_search's own default resolution.
        let mut editor = editor_with_restricted_primary();
        editor
            .kb
            .registry
            .instances
            .push(open_instance("OpenInstance", "uuid-open"));
        editor.kb.search_scope = "OpenInstance".to_string();

        let decision = check_kb_residency(
            &editor,
            "kb_search",
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
    fn kb_agenda_denied_when_primary_itself_restricted() {
        let editor = editor_with_restricted_primary();
        let decision =
            check_kb_residency(&editor, "kb_agenda", &serde_json::json!({}), Some("claude"));
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
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
    fn kb_graph_denied_outright_when_any_kb_restricted() {
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
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
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
    fn kb_health_denied_outright_when_any_kb_restricted() {
        let mut editor = Editor::new();
        editor
            .kb
            .registry
            .instances
            .push(restricted_instance("RestrictedInstance", "uuid-r"));
        let decision =
            check_kb_residency(&editor, "kb_health", &serde_json::json!({}), Some("claude"));
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
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
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_history",
            &serde_json::json!({"id": "index"}),
            Some("claude"),
        );
        assert!(matches!(decision, ResidencyDecision::Deny(_)));
    }

    #[test]
    fn kb_preview_show_denied_like_kb_get() {
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "kb_preview_show",
            &serde_json::json!({"id": "index"}),
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
        // (the old arrays only ever held "kb_*" strings).
        let editor = editor_with_restricted_primary();
        let decision = check_kb_residency(
            &editor,
            "help_open",
            &serde_json::json!({"id": "index"}),
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
