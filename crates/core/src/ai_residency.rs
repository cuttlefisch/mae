//! AI-residency FILTER primitives (ADR-048 / issue #358).
//!
//! Split for Rust crate-graph reasons, not a conceptual split: the
//! classification/gate half of ADR-048 (`classify_kb_tool`,
//! `check_kb_residency`) lives in `crates/mae/src/ai_residency.rs`, but that
//! file is compiled into the `mae`/`build-manual-kb` *binaries* only (the
//! `mae` package has no `[lib]` target) — it is structurally unreachable
//! from `mae-ai`'s tool implementations (`crates/ai/src/tool_impls/kb.rs`),
//! which is a library crate `mae` itself depends on (the reverse dependency
//! would be circular). `mae-core` is the closest common crate both `mae`
//! and `mae-ai` already depend on, and it already owns `Editor` — so the
//! exemption/filter primitives that tool-impls must call live here instead.
//!
//! Exemption: MAE's own seeded/built-in manual content (help docs, commands,
//! concepts — identical on every install, compiled in, never sensitive) is
//! exempt from `LocalModelsOnly` gating even when it lives in a restricted
//! KB instance — restricting `primary` to protect a user's own notes must
//! not also block MAE's own built-in help system. Exemption is keyed on
//! `Node::source == Some(NodeSource::Seed)`, already stamped once at startup
//! by `KnowledgeBase::stamp_source` (`shared/kb/src/lib.rs`) — no new
//! tagging infrastructure. This is orthogonal to `NodeKind`/
//! `properties["role"]` (the signals `search_ranked`'s `kind_role_prior`
//! uses to down-weight "hub" nodes in ranking, #357) — a seeded concept
//! page is never a molecular-notes hub, and a user's own hub note is never
//! seed content.

use crate::Editor;
use mae_kb::federation::AiResidency;

/// AI provider names MAE classifies as local (self-hosted). Single source
/// of truth — reused by the gate in `crates/mae/src/ai_residency.rs` so
/// both the gate and this filter agree on exactly one definition.
pub const LOCAL_AI_PROVIDERS: &[&str] = &["ollama"];

/// Is `provider` one MAE classifies as local (self-hosted)?
pub fn is_local_provider(provider: &str) -> bool {
    LOCAL_AI_PROVIDERS.contains(&provider)
}

/// Is `node` exempt from AI-residency gating regardless of its owning KB's
/// policy? Currently: MAE's own seeded/built-in content (#358).
pub fn is_residency_exempt(node: &mae_kb::Node) -> bool {
    node.source == Some(mae_kb::NodeSource::Seed)
}

/// `AiResidency` of the KB a federated-search hit's `instance` label names —
/// `None` means the primary KB.
fn kb_label_ai_residency(editor: &Editor, instance_name: Option<&str>) -> AiResidency {
    match instance_name {
        None => editor.kb.registry.primary_ai_residency,
        Some(name) => editor
            .kb
            .registry
            .find(name)
            .map(|inst| inst.ai_residency)
            .unwrap_or(AiResidency::Open),
    }
}

/// Core filtering primitive (CLAUDE.md #8 — the one place this logic
/// lives). Drops any `(instance, node)` hit that both (a) comes from a KB
/// currently `LocalModelsOnly`, and (b) isn't itself [`is_residency_exempt`].
/// No-op if `requester_provider` is already local. Used directly by
/// `kb_search`/`kb_search_context`'s federated hits;
/// [`filter_residency_exempt_primary`] is a thin adapter over this same
/// primitive for `kb_agenda`'s plain `Vec<Node>` shape (always primary) —
/// not a second implementation.
pub fn filter_residency_exempt(
    editor: &Editor,
    requester_provider: Option<&str>,
    results: Vec<(Option<String>, mae_kb::Node)>,
) -> Vec<(Option<String>, mae_kb::Node)> {
    if requester_provider.is_some_and(is_local_provider) {
        return results;
    }
    results
        .into_iter()
        .filter(|(instance_name, node)| {
            kb_label_ai_residency(editor, instance_name.as_deref()) != AiResidency::LocalModelsOnly
                || is_residency_exempt(node)
        })
        .collect()
}

/// Adapter over [`filter_residency_exempt`] for `kb_agenda`'s plain
/// `Vec<Node>` (always the primary KB — `kb_agenda` is
/// `PrimaryOnlyFilterable`, never federated).
pub fn filter_residency_exempt_primary(
    editor: &Editor,
    requester_provider: Option<&str>,
    nodes: Vec<mae_kb::Node>,
) -> Vec<mae_kb::Node> {
    filter_residency_exempt(
        editor,
        requester_provider,
        nodes.into_iter().map(|n| (None, n)).collect(),
    )
    .into_iter()
    .map(|(_, n)| n)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_node(id: &str) -> mae_kb::Node {
        mae_kb::Node::new(id, "Seeded", mae_kb::NodeKind::Concept, "body")
            .with_source(mae_kb::NodeSource::Seed, 1)
    }

    fn node_with_source(id: &str, source: Option<mae_kb::NodeSource>) -> mae_kb::Node {
        let mut n = mae_kb::Node::new(id, "Node", mae_kb::NodeKind::Note, "body");
        n.source = source;
        n
    }

    #[test]
    fn is_residency_exempt_true_only_for_seed() {
        assert!(is_residency_exempt(&seed_node("a")));
        assert!(!is_residency_exempt(&node_with_source(
            "b",
            Some(mae_kb::NodeSource::UserOrg)
        )));
        assert!(!is_residency_exempt(&node_with_source(
            "c",
            Some(mae_kb::NodeSource::Manual)
        )));
        assert!(!is_residency_exempt(&node_with_source(
            "d",
            Some(mae_kb::NodeSource::Federation)
        )));
        assert!(!is_residency_exempt(&node_with_source(
            "e",
            Some(mae_kb::NodeSource::Promoted)
        )));
        assert!(!is_residency_exempt(&node_with_source("f", None)));
    }

    #[test]
    fn filter_residency_exempt_keeps_seed_drops_non_seed_from_restricted_primary() {
        let mut editor = Editor::new();
        editor.kb.registry.primary_ai_residency = AiResidency::LocalModelsOnly;
        let results = vec![
            (None, seed_node("seed:a")),
            (
                None,
                node_with_source("user:b", Some(mae_kb::NodeSource::UserOrg)),
            ),
        ];
        let filtered = filter_residency_exempt(&editor, Some("claude"), results);
        let ids: Vec<&str> = filtered.iter().map(|(_, n)| n.id.as_str()).collect();
        assert_eq!(ids, vec!["seed:a"]);
    }

    #[test]
    fn filter_residency_exempt_is_noop_for_local_provider() {
        let mut editor = Editor::new();
        editor.kb.registry.primary_ai_residency = AiResidency::LocalModelsOnly;
        let results = vec![(
            None,
            node_with_source("user:b", Some(mae_kb::NodeSource::UserOrg)),
        )];
        let filtered = filter_residency_exempt(&editor, Some("ollama"), results);
        assert_eq!(
            filtered.len(),
            1,
            "local provider must bypass filtering entirely"
        );
    }

    #[test]
    fn filter_residency_exempt_is_noop_when_kb_open() {
        let editor = Editor::new(); // primary defaults to Open
        let results = vec![(
            None,
            node_with_source("user:b", Some(mae_kb::NodeSource::UserOrg)),
        )];
        let filtered = filter_residency_exempt(&editor, Some("claude"), results);
        assert_eq!(filtered.len(), 1, "an open KB's content is never filtered");
    }

    #[test]
    fn filter_residency_exempt_only_affects_the_named_restricted_instance() {
        let mut editor = Editor::new(); // open primary
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: "uuid-r".into(),
                name: "Restricted".into(),
                org_dir: std::path::PathBuf::new(),
                db_path: std::path::PathBuf::new(),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: AiResidency::LocalModelsOnly,
            });
        let results = vec![
            (
                None,
                node_with_source("primary:a", Some(mae_kb::NodeSource::UserOrg)),
            ),
            (
                Some("Restricted".to_string()),
                node_with_source("fed:b", Some(mae_kb::NodeSource::UserOrg)),
            ),
        ];
        let filtered = filter_residency_exempt(&editor, Some("claude"), results);
        let ids: Vec<&str> = filtered.iter().map(|(_, n)| n.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["primary:a"],
            "the open primary's hit must survive; the restricted instance's non-seed hit must not"
        );
    }

    #[test]
    fn filter_residency_exempt_primary_adapter_matches_core_semantics() {
        let mut editor = Editor::new();
        editor.kb.registry.primary_ai_residency = AiResidency::LocalModelsOnly;
        let nodes = vec![
            seed_node("seed:a"),
            node_with_source("user:b", Some(mae_kb::NodeSource::UserOrg)),
        ];
        let filtered = filter_residency_exempt_primary(&editor, Some("claude"), nodes);
        let ids: Vec<&str> = filtered.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["seed:a"]);
    }
}
