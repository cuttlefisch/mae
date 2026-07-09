//! Client-side AI-residency self-check (ADR-048) — defense in depth on top of
//! the server-side PSK-gated enforcement in `ai_event_handler.rs`. Non-
//! authoritative: a modified binary could skip this. The real boundary is the
//! server-side gate; this exists to give a fast, clear, local error instead of
//! relying on the server denial alone (and to avoid burning a round-trip when
//! the answer is already knowable client-side from `kb_instances`).
//!
//! **Deliberately coarser than the server-side gate**: this harness only sees
//! `kb_instances`-level metadata (KB names/uuids + their residency policy), not
//! each KB's node index — so it can only catch a *literal* KB name/uuid
//! reference in the arguments (e.g. `kb: "journal"`), not an arbitrary node id
//! that happens to be owned by a restricted KB. The server-side gate resolves
//! node ids to their owning KB and is the one that actually enforces this;
//! this check is a best-effort, cheap early exit, not a substitute for it.

use serde_json::Value;

/// Tools this harness pre-checks before issuing them, mirroring the
/// single-target set the server-side gate enforces authoritatively
/// (`crates/mae/src/ai_residency.rs::SINGLE_TARGET_KB_TOOLS`).
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
const FEDERATED_SCAN_KB_TOOLS: &[&str] = &[
    "kb_search",
    "kb_agenda",
    "kb_vector_search",
    "kb_search_context",
];
const TARGET_ARG_KEYS: &[&str] = &["id", "src", "dst", "from", "to", "kb"];

/// Parsed subset of a `kb_instances` tool result this check needs: for each
/// entry, its name/uuid and whether it's `local_models_only`.
#[derive(Debug, Clone)]
pub struct KbResidencyInfo {
    pub name: String,
    pub local_models_only: bool,
}

/// Result of the client-side self-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfCheckDecision {
    Proceed,
    Refuse(String),
}

/// Best-effort pre-flight check the harness runs before issuing `tool_name`
/// with `arguments`, given `own_provider` (this harness's own configured
/// provider) and the KB residency info it already fetched via `kb_instances`.
/// Conservative: if it can't determine a target (e.g. an id it doesn't
/// recognize), it lets the call through — the server-side gate is the real
/// authority and will deny it there if warranted.
pub fn check_before_call(
    tool_name: &str,
    arguments: &Value,
    own_provider: &str,
    kb_residency: &[KbResidencyInfo],
) -> SelfCheckDecision {
    if is_local_provider(own_provider) {
        return SelfCheckDecision::Proceed;
    }

    if FEDERATED_SCAN_KB_TOOLS.contains(&tool_name) {
        if let Some(restricted) = kb_residency.iter().find(|kb| kb.local_models_only) {
            return SelfCheckDecision::Refuse(format!(
                "AI-residency (client-side check): KB '{}' is local_models_only and this \
                 harness's provider ({own_provider}) isn't local — '{tool_name}' scans all KBs, \
                 so it's refused locally before even asking the server.",
                restricted.name
            ));
        }
        return SelfCheckDecision::Proceed;
    }

    if !SINGLE_TARGET_KB_TOOLS.contains(&tool_name) {
        return SelfCheckDecision::Proceed;
    }

    for key in TARGET_ARG_KEYS {
        let Some(value) = arguments.get(*key).and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(kb) = kb_residency
            .iter()
            .find(|kb| kb.name.eq_ignore_ascii_case(value) && kb.local_models_only)
        {
            return SelfCheckDecision::Refuse(format!(
                "AI-residency (client-side check): KB '{}' is local_models_only and this \
                 harness's provider ({own_provider}) isn't local.",
                kb.name
            ));
        }
    }
    SelfCheckDecision::Proceed
}

fn is_local_provider(provider: &str) -> bool {
    provider.eq_ignore_ascii_case("ollama")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restricted(name: &str) -> KbResidencyInfo {
        KbResidencyInfo {
            name: name.to_string(),
            local_models_only: true,
        }
    }
    fn open(name: &str) -> KbResidencyInfo {
        KbResidencyInfo {
            name: name.to_string(),
            local_models_only: false,
        }
    }

    #[test]
    fn local_provider_always_proceeds() {
        let kbs = vec![restricted("primary")];
        assert_eq!(
            check_before_call("kb_get", &serde_json::json!({"id": "x"}), "ollama", &kbs),
            SelfCheckDecision::Proceed
        );
    }

    #[test]
    fn non_local_provider_refused_for_restricted_target() {
        let kbs = vec![restricted("primary")];
        let decision = check_before_call(
            "kb_get",
            &serde_json::json!({"kb": "primary"}),
            "claude",
            &kbs,
        );
        assert!(matches!(decision, SelfCheckDecision::Refuse(_)));
    }

    #[test]
    fn open_kb_never_refused() {
        let kbs = vec![open("primary")];
        assert_eq!(
            check_before_call(
                "kb_get",
                &serde_json::json!({"kb": "primary"}),
                "claude",
                &kbs
            ),
            SelfCheckDecision::Proceed
        );
    }

    #[test]
    fn federated_scan_refused_when_any_kb_restricted() {
        let kbs = vec![open("work"), restricted("journal")];
        let decision = check_before_call("kb_search", &serde_json::json!({}), "claude", &kbs);
        assert!(matches!(decision, SelfCheckDecision::Refuse(_)));
    }

    #[test]
    fn unrecognized_target_lets_server_decide() {
        // Conservative: don't guess, let the server-side gate be the authority.
        let kbs = vec![restricted("primary")];
        let decision = check_before_call(
            "kb_get",
            &serde_json::json!({"id": "some-unrelated-node"}),
            "claude",
            &kbs,
        );
        assert_eq!(decision, SelfCheckDecision::Proceed);
    }
}
