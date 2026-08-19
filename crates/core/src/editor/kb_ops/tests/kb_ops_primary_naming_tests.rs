//! ADR-105 D4: one question — "does this name mean the primary KB?" — must have
//! one answer, across the two crates that ask it.
//!
//! Split from `kb_ops_collab_sync_tests.rs` to stay under the structural ceiling.

use super::*;

/// ADR-105 D4: `KB_DEFAULT_NAME` and `mae_kb::PRIMARY_NAME_ALIASES` answer the
/// same question — "does this name mean the primary KB?" — from two crates.
///
/// They already disagreed before D4: `mae-core` compared against
/// `KB_DEFAULT_NAME || "primary"` while `mae-kb`'s `set_ai_residency` accepted
/// only `"primary"`, so `:kb-set-ai-residency default …` silently reported "no
/// such KB". Two spellings of one predicate is the shape that produced it, and
/// the shape H3 is about. This fails the moment either side moves.
#[test]
fn kb_default_name_agrees_with_the_shared_alias_list() {
    assert!(
        mae_kb::PRIMARY_NAME_ALIASES.contains(&crate::editor::KB_DEFAULT_NAME),
        "KB_DEFAULT_NAME ({:?}) is not in mae_kb::PRIMARY_NAME_ALIASES ({:?}); a \
         name the editor accepts for the primary would not resolve in the registry",
        crate::editor::KB_DEFAULT_NAME,
        mae_kb::PRIMARY_NAME_ALIASES,
    );
}

/// ADR-048 + ADR-105 D4: every name MAE accepts for the primary KB must reach the
/// primary's residency policy.
///
/// `"default"` did not. `set_ai_residency` compared against a bare `"primary"`
/// while the rest of MAE calls the primary `KB_DEFAULT_NAME` ("default"), so
/// `kb_set_ai_residency("default", …)` returned `Err("no instance found matching
/// 'default'")` — a residency policy that silently declines to apply, on the
/// control whose entire job is keeping a sensitive KB away from hosted models.
///
/// Asserted over the alias list rather than the two names, so adding an alias
/// without wiring it fails here.
#[test]
fn every_primary_alias_reaches_the_primary_residency_policy() {
    for alias in mae_kb::PRIMARY_NAME_ALIASES {
        let mut editor = Editor::new();
        let _test_dirs = with_test_dirs(&mut editor);
        let r = editor.kb_set_ai_residency(alias, mae_kb::federation::AiResidency::LocalModelsOnly);
        assert!(
            r.is_ok(),
            "'{alias}' names the primary KB but was refused: {r:?}"
        );
        assert_eq!(
            editor.kb.registry.primary_ai_residency,
            mae_kb::federation::AiResidency::LocalModelsOnly,
            "'{alias}' was accepted but the policy did not actually change — an \
             Ok() that applies nothing is the worse failure"
        );
    }
}
