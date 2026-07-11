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

    // --- Adversarial / gap coverage (principle #14) ---

    #[test]
    fn own_provider_check_is_case_insensitive() {
        // `is_local_provider` uses `eq_ignore_ascii_case`, not `==` -- an
        // uppercase/mixed-case provider string (e.g. from a config file or
        // CLI flag typed differently than the canonical "ollama") must still
        // be treated as local and bypass the check entirely. The target here
        // deliberately points AT the restricted KB: if the case-insensitive
        // comparison were broken (e.g. `==`), this would incorrectly Refuse
        // instead of Proceed.
        let kbs = vec![restricted("primary")];
        assert_eq!(
            check_before_call(
                "kb_get",
                &serde_json::json!({"kb": "primary"}),
                "OLLAMA",
                &kbs
            ),
            SelfCheckDecision::Proceed
        );
        assert_eq!(
            check_before_call(
                "kb_get",
                &serde_json::json!({"kb": "primary"}),
                "OlLaMa",
                &kbs
            ),
            SelfCheckDecision::Proceed
        );
    }

    #[test]
    fn kb_name_match_is_case_insensitive() {
        // The TARGET_ARG_KEYS loop matches via `kb.name.eq_ignore_ascii_case`,
        // so a restricted KB registered as "primary" must still be caught when
        // the tool argument spells it "PRIMARY" (a different case than what's
        // in the registry).
        let kbs = vec![restricted("primary")];
        let decision = check_before_call(
            "kb_get",
            &serde_json::json!({"kb": "PRIMARY"}),
            "claude",
            &kbs,
        );
        assert!(
            matches!(decision, SelfCheckDecision::Refuse(_)),
            "differently-cased KB name must still match: {decision:?}"
        );
    }

    #[test]
    fn near_miss_kb_names_do_not_match() {
        // Exact-string comparison only -- a restricted KB named "journal"
        // must NOT be caught by a substring/prefix/suffix near-miss like
        // "journal-drafts" or "myjournal". This guards against a future
        // refactor accidentally loosening `eq_ignore_ascii_case` into a
        // `contains`/`starts_with`/`ends_with` check.
        let kbs = vec![restricted("journal")];
        for near_miss in ["journal-drafts", "myjournal", "journalist", "my-journal-x"] {
            let decision = check_before_call(
                "kb_get",
                &serde_json::json!({"kb": near_miss}),
                "claude",
                &kbs,
            );
            assert_eq!(
                decision,
                SelfCheckDecision::Proceed,
                "near-miss '{near_miss}' must not match restricted KB 'journal'"
            );
        }
    }

    #[test]
    fn every_target_arg_key_is_checked() {
        // TARGET_ARG_KEYS = ["id", "src", "dst", "from", "to", "kb"]. Exercise
        // every key at least once so a typo/reordering/removal in that array
        // would actually fail a test, not just the "id"/"kb" pair the
        // pre-existing tests covered.
        let kbs = vec![restricted("primary")];
        for key in ["id", "src", "dst", "from", "to", "kb"] {
            let decision = check_before_call(
                "kb_get",
                &serde_json::json!({ key: "primary" }),
                "claude",
                &kbs,
            );
            assert!(
                matches!(decision, SelfCheckDecision::Refuse(_)),
                "key '{key}' should be checked against restricted KBs: {decision:?}"
            );
        }
    }

    #[test]
    fn federated_scan_proceeds_when_no_kb_restricted() {
        // The refuse case (some KB IS restricted) is covered by
        // `federated_scan_refused_when_any_kb_restricted`; this covers the
        // Proceed half of the same branch.
        let kbs = vec![open("work"), open("journal")];
        let decision = check_before_call("kb_search", &serde_json::json!({}), "claude", &kbs);
        assert_eq!(decision, SelfCheckDecision::Proceed);
    }

    #[test]
    fn tool_outside_both_lists_always_proceeds() {
        // A tool name in neither SINGLE_TARGET_KB_TOOLS nor
        // FEDERATED_SCAN_KB_TOOLS must proceed unconditionally -- regardless
        // of arguments or a fully-restricted KB set.
        let kbs = vec![restricted("primary")];
        for tool in ["shell_exec", "buffer_read"] {
            let decision = check_before_call(
                tool,
                &serde_json::json!({"kb": "primary", "id": "primary"}),
                "claude",
                &kbs,
            );
            assert_eq!(
                decision,
                SelfCheckDecision::Proceed,
                "tool '{tool}' is not KB-target-checked and must always proceed"
            );
        }
    }

    #[test]
    fn multiple_restricted_kbs_matches_the_correct_one() {
        // With several restricted KBs registered, the check must correctly
        // identify the one actually named in the arguments, not refuse based
        // on an unrelated restricted KB or fail to match at all.
        let kbs = vec![restricted("alpha"), restricted("beta"), restricted("gamma")];
        let decision =
            check_before_call("kb_get", &serde_json::json!({"kb": "beta"}), "claude", &kbs);
        match decision {
            SelfCheckDecision::Refuse(msg) => {
                assert!(
                    msg.contains("beta"),
                    "refusal must name the actual matching KB, got: {msg}"
                );
                assert!(
                    !msg.contains("alpha") && !msg.contains("gamma"),
                    "refusal must not misattribute to an unrelated restricted KB: {msg}"
                );
            }
            other => panic!("expected Refuse for restricted 'beta', got {other:?}"),
        }
    }

    #[test]
    fn non_string_argument_values_are_skipped_gracefully() {
        // `arguments.get(key).and_then(|v| v.as_str())` returns None for a
        // non-string JSON value (number, null, object, array) -- the loop
        // must skip it (not panic) and fall through to Proceed when no other
        // key matches.
        let kbs = vec![restricted("primary")];
        for bad_value in [
            serde_json::json!(123),
            serde_json::json!(null),
            serde_json::json!({"nested": "primary"}),
            serde_json::json!(["primary"]),
        ] {
            let decision = check_before_call(
                "kb_get",
                &serde_json::json!({"kb": bad_value}),
                "claude",
                &kbs,
            );
            assert_eq!(
                decision,
                SelfCheckDecision::Proceed,
                "non-string 'kb' value {bad_value:?} must not panic and must not match"
            );
        }
    }
}
