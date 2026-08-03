//! Permission tier formatting and capability checks.

use crate::tools::PermissionPolicy;
use crate::types::PermissionTier;

/// What `ai_permissions` reports.
///
/// ADR-090 D4: the tier names come from `PermissionTier::config_name()`, the
/// one vocabulary. This function used to spell them `standard`/`trusted`/`full`
/// — a fourth set of aliases, and the one an agent would read and then paste
/// into a config where a *different* parser had to accept it. The legacy
/// aliases are still accepted on input (`PermissionTier::parse`); they are
/// simply not produced here.
pub(crate) fn format_permissions_info(policy: &PermissionPolicy) -> String {
    let tier_name = policy.auto_approve_up_to.config_name();

    let mut out = format!(
        "Current auto-approve tier: {tier_name}\n\n\
         Permission tiers (lowest to highest):\n\
         - readonly: Read buffer contents, cursor state, file listings, project search\n\
         - write: Modify buffers, edit files, save, undo/redo\n\
         - shell: Execute shell commands, run builds and tests\n\
         - privileged: Quit editor, modify config, change KB authorization\n\n\
         A permission check answers one of three things (ADR-090):\n\
         - allow: at or below '{tier_name}' — runs with no prompt.\n\
         - ask:   above '{tier_name}' — an interactive surface asks a human first.\n\
                  A non-interactive surface (external MCP, `mae-agent --prompt`,\n\
                  `--self-test`) denies instead, and says so, because there is\n\
                  nobody to ask. It does NOT silently run.\n\
         - deny:  forbidden outright. No prompt can raise it.\n\n"
    );

    match policy.hard_ceiling {
        Some(hc) => out.push_str(&format!(
            "This session has a HARD ceiling of {} ({}). Anything above it is denied \
             outright, not asked — raising the auto-approve tier will not help.\n",
            hc.tier.config_name(),
            match hc.source {
                crate::tools::HardCeilingSource::SessionDeclared =>
                    "declared by this session at initialize",
                crate::tools::HardCeilingSource::UnparseableDeclaration =>
                    "this session's declared ceiling could not be parsed, so the most \
                     restrictive tier applies",
            }
        )),
        None => out.push_str(
            "This session has no hard ceiling: everything above the auto-approve tier is \
             askable rather than forbidden.\n",
        ),
    }

    if let Some(cats) = &policy.allowed_categories {
        let mut names: Vec<String> = cats.iter().map(|c| format!("{c:?}")).collect();
        names.sort();
        out.push_str(&format!(
            "Tool categories are restricted to: [{}]. Tools outside that set are denied \
             regardless of tier, and a tool with no classified category is denied too \
             (fail-closed).\n",
            names.join(", ")
        ));
    }

    out.push_str(
        "\nConfigure via MAE_AI_PERMISSIONS env var or [ai] auto_approve_tier in config.toml.\n\
         Agent tool approval (MCP) is separate — see [agents] auto_approve_tools in config.toml.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{HardCeiling, HardCeilingSource};

    /// The reported tier name must round-trip through the one parser. An agent
    /// that reads this output and writes the name into a config must not have
    /// produced a value that config refuses — which is exactly what the old
    /// `standard`/`trusted`/`full` spellings risked once the parsers diverged.
    #[test]
    fn every_reported_tier_name_parses_back_to_the_same_tier() {
        for tier in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            let policy = PermissionPolicy {
                auto_approve_up_to: tier,
                ..PermissionPolicy::default()
            };
            let out = format_permissions_info(&policy);
            let name = tier.config_name();
            assert!(
                out.contains(&format!("Current auto-approve tier: {name}")),
                "{tier:?}: {out}"
            );
            assert_eq!(PermissionTier::parse(name), Some(tier));
        }
    }

    /// The report must not let an agent mistake a hard ceiling for something a
    /// raised tier would fix — that is the one piece of advice that is actively
    /// wrong for a `Deny`.
    #[test]
    fn a_hard_ceiling_is_reported_as_unraisable() {
        let policy = PermissionPolicy::default().with_hard_ceiling(HardCeiling {
            tier: PermissionTier::ReadOnly,
            source: HardCeilingSource::SessionDeclared,
        });
        let out = format_permissions_info(&policy);
        assert!(out.contains("HARD ceiling"), "{out}");
        assert!(out.contains("will not help"), "{out}");

        let unrestricted = format_permissions_info(&PermissionPolicy::default());
        assert!(unrestricted.contains("no hard ceiling"), "{unrestricted}");
        assert!(!unrestricted.contains("HARD ceiling"), "{unrestricted}");
    }

    /// `ask` must be described as "a human is asked", never as "denied" —
    /// otherwise the model concludes the tool is unavailable and stops trying,
    /// which is the behavioural half of the regression ADR-090 exists to avoid.
    #[test]
    fn the_report_describes_ask_as_asking_not_as_failing() {
        let out = format_permissions_info(&PermissionPolicy::default());
        assert!(out.contains("asks a human first"), "{out}");
        assert!(out.contains("does NOT silently run"), "{out}");
    }

    /// A category restriction is surfaced, and named as tier-independent.
    #[test]
    fn a_category_restriction_is_reported_as_orthogonal_to_the_tier() {
        let mut cats = std::collections::HashSet::new();
        cats.insert(crate::tools::ToolCategory::Knowledge);
        let policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            hard_ceiling: None,
            allowed_categories: Some(cats),
        };
        let out = format_permissions_info(&policy);
        assert!(out.contains("Knowledge"), "{out}");
        assert!(out.contains("regardless of tier"), "{out}");
        assert!(out.contains("fail-closed"), "{out}");
    }
}
