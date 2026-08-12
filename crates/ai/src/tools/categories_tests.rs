//! Tests for the tool-category / permission-policy decision point.
//!
//! Split out of `categories.rs` so that file stays a readable ~550-line policy
//! module. It is the **singular** PDP — ADR-084 D1 makes that singularity a
//! deliberate security property, so `decide`/`decide_tier`/`ambient_scheme_tier`
//! must never be split apart to satisfy a size ceiling. Moving the tests is the
//! honest way to keep the file under it. Mirrors the sibling `decision_tests.rs`.

use super::categories::*;
use crate::types::{PermissionTier, ToolDefinition};

#[cfg(test)]
mod annotation_tests {
    use super::*;

    #[test]
    fn read_only_tier_is_read_only_and_idempotent_never_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::ReadOnly);
        assert!(read_only);
        assert!(!destructive);
        assert!(idempotent);
    }

    #[test]
    fn write_tier_is_neither_read_only_nor_flagged_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::Write);
        assert!(!read_only);
        assert!(!destructive);
        assert!(!idempotent);
    }

    #[test]
    fn shell_and_privileged_tiers_are_flagged_destructive_never_read_only() {
        for tier in [PermissionTier::Shell, PermissionTier::Privileged] {
            let (read_only, destructive, _) = annotations_for_tier(tier);
            assert!(!read_only, "{tier:?} must never be read_only_hint: true");
            assert!(
                destructive,
                "{tier:?} must be flagged destructive_hint: true"
            );
        }
    }

    /// Exhaustive consistency check across every `PermissionTier` variant:
    /// `read_only_hint` must be true if and only if the tier is `ReadOnly`.
    /// This is what makes the mapping in `annotations_for_tier` a genuine
    /// single source of truth rather than something that could silently
    /// drift from `PermissionTier` if a variant is ever added -- add the new
    /// variant to this array and the compiler/test forces the mapping to be
    /// considered.
    #[test]
    fn read_only_hint_is_exactly_read_only_tier() {
        for tier in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            let (read_only, _, _) = annotations_for_tier(tier);
            assert_eq!(
                read_only,
                tier == PermissionTier::ReadOnly,
                "read_only_hint mismatch for {tier:?}"
            );
        }
    }
}

#[cfg(test)]
mod category_allowlist_tests {
    use super::*;
    use crate::tools::{ai_specific_tools, tools_from_registry};

    /// Every tool MAE actually registers -- the command-derived registry
    /// surface plus the AI-specific meta-tools -- mirrors the enumeration
    /// pattern `crates/ai/src/executor/mod_tests.rs::all_tools()` uses for
    /// ADR-050 D2's annotation-consistency audit. ADR-085 D4: tests in this
    /// module assert properties of the *whole* registry, not a hand-picked
    /// sample -- three cherry-picked tool names is exactly the "unicorn
    /// values" shape (principle #14) that let `babel_execute`/`babel_tangle`
    /// sit inside `Knowledge` undetected.
    fn all_tools() -> Vec<ToolDefinition> {
        let mut tools = tools_from_registry(&mae_core::CommandRegistry::with_builtins());
        tools.extend(ai_specific_tools(&mae_core::OptionRegistry::new()));
        tools
    }

    #[test]
    fn unrestricted_policy_allows_everything() {
        let policy = PermissionPolicy::default();
        assert!(policy.is_category_allowed("kb_search"));
        assert!(policy.is_category_allowed("execute_command"));
        assert!(policy.is_category_allowed("shell_exec"));
    }

    /// ADR-085 D2, the invariant that generalises the babel fix: a
    /// read-flavoured category (`ToolCategory::is_read_flavoured`) may never
    /// contain a tool declared above `Write` tier. Iterates the entire live
    /// tool registry -- not a sample -- so a future tool added under a
    /// knowledge-ish/lsp-ish/web-ish/debug-ish prefix that happens to be
    /// Shell/Privileged tier fails the build instead of silently becoming
    /// reachable from a `knowledge`-only (or `lsp`-only, `web`-only, ...)
    /// session. Do NOT add an allowlist/exception to make a future failure
    /// here pass -- fix the tool's classification instead (move it to a
    /// non-read-flavoured category, per ADR-085 D5/D6).
    #[test]
    fn read_flavoured_categories_never_exceed_write_tier() {
        let tools = all_tools();
        assert!(
            tools.len() > 100,
            "sanity check: expected hundreds of registered tools, got {}",
            tools.len()
        );

        let offenders: Vec<String> = tools
            .iter()
            .filter_map(|tool| {
                let category = classify_tool_category(&tool.name)?;
                if !category.is_read_flavoured() {
                    return None;
                }
                let tier = tool.permission?;
                (tier > PermissionTier::Write)
                    .then(|| format!("{} (category {category:?}, tier {tier:?})", tool.name))
            })
            .collect();

        assert!(
            offenders.is_empty(),
            "read-flavoured categories must not contain a tool above Write tier, found:\n{}",
            offenders.join("\n")
        );
    }

    /// The regression test for the specific bug ADR-085 fixes: a session
    /// allowlisted to `knowledge` must not be able to reach either babel
    /// tool. The fix is that they are never classified as `Knowledge` in the
    /// first place (not that they're offered and then separately refused by
    /// tier), so this asserts both the classification and the gate.
    #[test]
    fn knowledge_only_session_cannot_reach_babel_execution_tools() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        for tool in ["babel_execute", "babel_tangle"] {
            assert_ne!(
                classify_tool_category(tool),
                Some(ToolCategory::Knowledge),
                "{tool} must not classify as Knowledge (ADR-085)"
            );
            assert!(
                !policy.is_category_allowed(tool),
                "a knowledge-only session must not reach {tool}"
            );
        }
    }

    /// Positive case, derived from the live registry rather than three
    /// hand-picked names: every tool that actually classifies as `Knowledge`
    /// today is reachable under a `knowledge`-only restriction.
    #[test]
    fn knowledge_only_allows_every_registered_knowledge_tool() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        let knowledge_tools: Vec<_> = all_tools()
            .into_iter()
            .filter(|t| classify_tool_category(&t.name) == Some(ToolCategory::Knowledge))
            .collect();
        assert!(
            knowledge_tools.len() > 10,
            "sanity check: expected a healthy number of Knowledge tools, got {}",
            knowledge_tools.len()
        );
        for tool in &knowledge_tools {
            assert!(
                policy.is_category_allowed(&tool.name),
                "{} classifies as Knowledge but is denied under a knowledge-only restriction",
                tool.name
            );
        }
    }

    #[test]
    fn knowledge_only_denies_a_wrong_category_tool() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert!(!policy.is_category_allowed("git_push"));
        assert!(!policy.is_category_allowed("lsp_hover"));
    }

    // The highest-value bypass: an uncategorized tool (classify_tool_category
    // returns None) must fail CLOSED under a restriction, not open.
    #[test]
    fn knowledge_only_denies_uncategorized_tools_fail_closed() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert_eq!(
            classify_tool_category("execute_command"),
            None,
            "sanity: must be uncategorized"
        );
        assert!(!policy.is_category_allowed("execute_command"));
        assert_eq!(
            classify_tool_category("shell_exec"),
            None,
            "sanity: must be uncategorized"
        );
        assert!(!policy.is_category_allowed("shell_exec"));
    }

    #[test]
    fn discovery_tools_stay_reachable_under_any_restriction() {
        let policy = PermissionPolicy {
            allowed_categories: Some([ToolCategory::Knowledge].into_iter().collect()),
            ..PermissionPolicy::default()
        };
        assert!(policy.is_category_allowed("request_tools"));
        assert!(policy.is_category_allowed("search_tools"));
    }

    /// `ToolCategory::ALL` exists so registry-wide invariants (like the two
    /// tests above) can iterate rather than depend on someone remembering to
    /// extend a hand-written list -- so `ALL` itself needs a check that
    /// actually catches a missing/duplicated entry. A test that merely loops
    /// `for c in ToolCategory::ALL { match c { ... } }` would NOT catch a
    /// variant missing from `ALL`: the loop simply never visits what isn't
    /// there, so the match is vacuously exhaustive over an incomplete set.
    /// Instead, the array literal below is independent of `ALL` and is
    /// checked against the real enum by the exhaustive `match` inside the
    /// loop -- adding a `ToolCategory` variant without adding it here fails
    /// to *compile*, and the `ALL.contains` + length assertions then catch a
    /// variant present in the enum but missing (or duplicated) in `ALL`.
    #[test]
    fn tool_category_all_covers_every_variant_exactly_once() {
        let every_variant = [
            ToolCategory::Lsp,
            ToolCategory::Dap,
            ToolCategory::Knowledge,
            ToolCategory::Execution,
            ToolCategory::ShellMgmt,
            ToolCategory::Commands,
            ToolCategory::Git,
            ToolCategory::Web,
            ToolCategory::Ai,
            ToolCategory::Visual,
            ToolCategory::Debug,
            ToolCategory::Mcp,
        ];
        for category in every_variant {
            // Exhaustive match over the REAL enum: adding a `ToolCategory`
            // variant that isn't listed in `every_variant` above leaves this
            // match non-exhaustive, so the test file fails to compile until
            // both the array above and this match are extended.
            match category {
                ToolCategory::Lsp
                | ToolCategory::Dap
                | ToolCategory::Knowledge
                | ToolCategory::Execution
                | ToolCategory::ShellMgmt
                | ToolCategory::Commands
                | ToolCategory::Git
                | ToolCategory::Web
                | ToolCategory::Ai
                | ToolCategory::Visual
                | ToolCategory::Debug
                | ToolCategory::Mcp => {}
            }
            assert!(
                ToolCategory::ALL.contains(&category),
                "{category:?} is a real ToolCategory variant but is missing from \
                 ToolCategory::ALL -- registry-wide invariant tests silently skip it"
            );
        }
        assert_eq!(
            ToolCategory::ALL.len(),
            every_variant.len(),
            "ToolCategory::ALL has a different number of entries than there are real \
             variants (duplicate or stale entry) -- update ALL to match exactly"
        );
    }
}
