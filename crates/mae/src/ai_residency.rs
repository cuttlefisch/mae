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
//! **Scope note (v1):** single-target tools (an explicit node id or KB name in
//! the arguments) are checked precisely and denied outright when restricted.
//! The federated-scan tools (`kb_search`, `kb_agenda`, `kb_vector_search`,
//! `kb_search_context`) do not share a consistent per-result "which instance did
//! this come from" shape today (confirmed: `kb_agenda` doesn't even tag results
//! by instance) — rather than risk a subtly-wrong per-tool result filter, v1
//! conservatively denies the *entire* federated scan whenever ANY registered KB
//! (or the primary) is `LocalModelsOnly` and the requester isn't local. This is
//! coarser than ADR-048's original "post-filter, don't fail the whole call"
//! design — a documented, honest simplification, not a silent gap. Fine-grained
//! per-tool filtering is a real, separate follow-up once those tools carry
//! consistent instance attribution.

use mae_core::Editor;

/// AI provider names MAE classifies as "local" for residency purposes.
const LOCAL_AI_PROVIDERS: &[&str] = &["ollama"];

/// `kb_*` tools whose arguments resolve to exactly one (or two, for `kb_add_link`)
/// specific node id(s) or an explicit KB instance name — checked precisely.
const SINGLE_TARGET_KB_TOOLS: &[&str] = &[
    "kb_get",
    "kb_update",
    "kb_delete",
    "kb_add_link",
    "kb_restore",
    "kb_links_from",
    "kb_links_to",
    "kb_related",
    "kb_shortest_path",
    "kb_neighborhood",
];

/// Argument keys, across [`SINGLE_TARGET_KB_TOOLS`], that hold a node id or an
/// explicit KB instance name/uuid worth resolving.
const TARGET_ARG_KEYS: &[&str] = &["id", "src", "dst", "from", "to", "kb"];

/// `kb_*` tools that scan across potentially multiple KB instances at once —
/// conservatively denied outright when any KB is residency-restricted (see
/// module doc for why fine-grained filtering isn't implemented yet).
const FEDERATED_SCAN_KB_TOOLS: &[&str] = &[
    "kb_search",
    "kb_agenda",
    "kb_vector_search",
    "kb_search_context",
];

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

    if FEDERATED_SCAN_KB_TOOLS.contains(&tool_name) {
        if let Some(label) = any_restricted_kb_label(editor) {
            return ResidencyDecision::Deny(format!(
                "AI-residency policy: KB '{label}' is set to local_models_only, and this \
                 session's AI provider ({}) isn't a local model. '{tool_name}' scans across \
                 all registered KBs, so it's blocked outright rather than silently omitting \
                 that KB's results — use kb_get/kb_search with an explicit scope/kb argument \
                 that excludes it, or switch to a local (Ollama) provider.",
                requester_provider.unwrap_or("none/unauthenticated")
            ));
        }
        return ResidencyDecision::Allow;
    }

    if !SINGLE_TARGET_KB_TOOLS.contains(&tool_name) {
        // Not a content-touching kb_* tool this gate cares about (kb_health,
        // kb_instances, kb_share, membership/policy tools, non-kb tools, etc).
        return ResidencyDecision::Allow;
    }

    for key in TARGET_ARG_KEYS {
        let Some(value) = arguments.get(*key).and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(label) = resolve_restricted_label(editor, value) {
            return ResidencyDecision::Deny(format!(
                "AI-residency policy: KB '{label}' is set to local_models_only, and this \
                 session's AI provider ({}) isn't a local model.",
                requester_provider.unwrap_or("none/unauthenticated")
            ));
        }
    }
    ResidencyDecision::Allow
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
/// or any registered instance), if any.
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
    fn federated_scan_tool_denied_outright_when_any_kb_restricted() {
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
}
