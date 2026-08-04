//! Which operations change *authorization* rather than content — and the
//! argument-sensitive tier escalation that keeps that judgment true on every
//! surface (decision #6, `docs/DECISIONS_FOR_REVIEW.md`).
//!
//! # Why this is one list and not three
//!
//! The same effect is reachable from three MCP surfaces:
//!
//! 1. the hand-authored tool (`kb_add_member`, declared in `kb_tools.rs`),
//! 2. the generated command mirror (`command_kb_add_member`, whose tier comes
//!    from [`crate::tools::classify_command_permission`]'s unknown-command
//!    default — `Write`),
//! 3. `execute_command` with `{"command": "kb-add-member"}`, which is a
//!    `Write`-tier tool that calls `Editor::dispatch_builtin` with whatever
//!    name it is handed.
//!
//! Raising only (1) would have been theatre: `command_kb_share` shares the
//! *active* KB with no arguments at all (`crates/core/src/editor/dispatch/collab.rs`),
//! and `execute_command {"command": "kb-share"}` reaches the identical code.
//! So the classification lives here once, in a surface-independent
//! (underscore) spelling, and all three surfaces consult it.
//!
//! # The criterion
//!
//! An operation belongs in [`AUTHORIZATION_CHANGE_OPS`] when it **changes who
//! may access a resource, or relaxes a protective restriction** — as opposed
//! to changing content. ADR-018 already treats KB membership as owner-only;
//! the tier table should agree with it. Acquiring access *for yourself* using
//! a credential you already hold (`kb_join`) and *tightening* a restriction
//! (`kb_block_member`) are deliberately NOT in the list — see the per-entry
//! notes.
//!
//! @ai-caution: [permission] Adding a KB sharing/membership tool means adding
//! it here too, not just declaring a tier in `kb_tools.rs` —
//! `authorization_change_ops_are_privileged_on_every_surface` fails otherwise.
//!
//! @stability: experimental

use crate::types::PermissionTier;

/// Canonical, surface-independent names of the operations that change
/// authorization. Written in the underscore spelling; [`normalize_op`] maps
/// the kebab-case command spelling onto it, so `kb-add-member`,
/// `kb_add_member`, and `command_kb_add_member` all resolve to one entry.
///
/// Every entry is `Privileged` on every surface. The list is deliberately
/// short and effect-based — it is not "everything with `member` in the name".
pub const AUTHORIZATION_CHANGE_OPS: &[&str] = &[
    // Exposes a KB's contents to the daemon and to every peer the join
    // policy admits. `command_kb_share` takes no arguments and shares the
    // *active* KB, which is why the command mirror matters here.
    "kb_share",
    // Mints a bearer join ticket (`mae://join/…`). Strictly a stronger grant
    // than `kb_share`: anyone holding the string can request to join.
    "kb_share_p2p",
    // Membership grant/revoke/role change — owner-only per ADR-018.
    "kb_add_member",
    "kb_remove_member",
    "kb_approve",
    // Join policy: `permissive` auto-admits any authenticated peer as a
    // viewer. Relaxing it is a bulk membership grant.
    "kb_set_policy",
    // Removes an entry from the ADR-039 A2 self-protection deny-list —
    // i.e. restores trust in a principal the operator deliberately distrusted.
    // Relaxing a protection, same shape as `kb_set_ai_residency`.
    // (Its inverse, `kb_block_member`, is deliberately absent: see below.)
    "kb_unblock_member",
    // Owner-only, irreversible, and rewrites the signed membership log's
    // per-member key wrapping (ADR-037/038) — an operation on the
    // authorization substrate itself, not on content.
    "kb_set_encryption",
    // Relaxes the ADR-048 residency restriction, i.e. re-permits hosted
    // providers to read a KB the operator restricted to local models.
    "kb_set_ai_residency",
];

/// Deliberately *not* authorization changes, recorded so the omissions are
/// reviewable rather than looking like oversights. Used by the tests, and by
/// anyone re-deciding one of these.
///
/// - `kb_join` / `kb_join_p2p` — acquire access *for the caller*, using a
///   credential the caller already holds (a ticket, or the daemon's own
///   admission decision). They grant no third party anything and disclose no
///   local content; the flow is inbound.
/// - `kb_leave` — revokes only the caller's own subscription, is reversible
///   via `kb_join`, and preserves the local copy.
/// - `kb_block_member` — *tightens*, never relaxes. ADR-039 A2 makes it
///   deliberately non-owner-gated self-protection; putting the safety valve
///   behind a higher tier than the thing it protects against would be
///   backwards.
/// - `kb_set_role` — a name collision only. It stamps a molecular-note
///   `:role:` property (source/atom/molecule/hub) on a KB *node*; it has
///   nothing to do with the ADR-018 membership role.
pub const DELIBERATELY_NOT_AUTHORIZATION_CHANGES: &[&str] = &[
    "kb_join",
    "kb_join_p2p",
    "kb_leave",
    "kb_block_member",
    "kb_set_role",
];

/// The option whose value *is* the permission policy.
///
/// ADR-084 D7 makes `ai_tier` reach the enforced policy rather than only a
/// status-bar string. From that point on, "set an option" and "raise my own
/// tier" are the same operation for this one name — so it is gated
/// specifically, rather than raising `set_option` wholesale (which would put
/// ordinary configuration behind `Privileged` for no security gain).
pub const PERMISSION_TIER_OPTION: &str = "ai_tier";

/// Normalize an operation name to the canonical underscore spelling:
/// strips a `command_` MCP-tool prefix, maps `-` to `_`, and lowercases.
///
/// `command_kb_add_member` -> `kb_add_member`; `kb-add-member` ->
/// `kb_add_member`.
pub fn normalize_op(name: &str) -> String {
    name.strip_prefix("command_")
        .unwrap_or(name)
        .replace('-', "_")
        .to_ascii_lowercase()
}

/// Is `name` — in any of its surface spellings — an authorization change?
pub fn is_authorization_change(name: &str) -> bool {
    let normalized = normalize_op(name);
    AUTHORIZATION_CHANGE_OPS.contains(&normalized.as_str())
}

/// Is `name` the option that carries the permission tier? Matches both the
/// registry spelling (`ai_tier`) and its documented alias (`ai-tier`), since
/// `OptionRegistry` accepts either.
pub fn is_permission_tier_option(name: &str) -> bool {
    normalize_op(name) == PERMISSION_TIER_OPTION
}

/// The tier a call actually requires, given the tier its tool *declares* and
/// the arguments it was called with.
///
/// Two tools' blast radius depends on their arguments rather than their
/// identity, and for both the honest answer is a per-call decision:
///
/// - `set_option` is ordinary configuration for every option except
///   [`PERMISSION_TIER_OPTION`], for which it is a tier change.
/// - `execute_command` is a `Write`-tier passthrough to
///   `Editor::dispatch_builtin`, so its real tier is the tier of the command
///   named in its argument.
///
/// Never lowers: the result is always at least `declared`, so this can add a
/// requirement but never remove one.
///
/// @ai-caution: [permission] This is consulted at exactly one site
/// (`execute_tool_dispatch_body`'s permission check). Do not re-derive it at
/// a call site — a second copy is a second thing to keep in sync, and the
/// one that drifts is the one that grants.
pub fn effective_tier(
    tool_name: &str,
    args: &serde_json::Value,
    declared: PermissionTier,
) -> PermissionTier {
    let escalated = match tool_name {
        "set_option" => args
            .get("option")
            .and_then(|v| v.as_str())
            .is_some_and(is_permission_tier_option),
        "execute_command" => args
            .get("command")
            .and_then(|v| v.as_str())
            // `dispatch_builtin` matches whole command names, but an
            // argument-bearing ex line (`set ai-tier privileged`) would be
            // routed by its first token, so classify on that token.
            .and_then(|c| c.split_whitespace().next())
            .is_some_and(|cmd| is_authorization_change(cmd) || is_permission_tier_command(cmd)),
        _ => false,
    };
    if escalated {
        declared.max(PermissionTier::Privileged)
    } else {
        declared
    }
}

/// Does this ex-command name set editor options (and therefore possibly the
/// permission-tier one)? `:set`/`:set-save`/`:set-local` are the three
/// spellings `Editor::execute_command` recognises.
fn is_permission_tier_command(cmd: &str) -> bool {
    matches!(normalize_op(cmd).as_str(), "set" | "set_save" | "set_local")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ai_specific_tools, classify_command_permission, sanitize_command_name};
    use mae_core::{CommandRegistry, OptionRegistry};
    use serde_json::json;

    /// The invariant decision #6 exists to install, asserted across all three
    /// surfaces at once rather than tool-by-tool. A tool raised in
    /// `kb_tools.rs` but forgotten in `classify_command_permission` leaves
    /// `command_<name>` as a `Write`-tier path to the identical effect —
    /// which is the bypass, not a cosmetic inconsistency.
    #[test]
    fn authorization_change_ops_are_privileged_on_every_surface() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let registry = CommandRegistry::with_builtins();
        let command_tools = crate::tools::tools_from_registry(&registry);

        let mut checked_direct = 0usize;
        let mut checked_mirror = 0usize;
        for op in AUTHORIZATION_CHANGE_OPS {
            // Surface 1: the hand-authored tool.
            if let Some(t) = tools.iter().find(|t| t.name == *op) {
                assert_eq!(
                    t.permission,
                    Some(PermissionTier::Privileged),
                    "{op} is an authorization change but its tool declares {:?}",
                    t.permission
                );
                checked_direct += 1;
            }
            // Surface 2: the generated command mirror, if the command exists.
            let kebab = op.replace('_', "-");
            let mirror = format!("command_{}", sanitize_command_name(&kebab));
            if let Some(t) = command_tools.iter().find(|t| t.name == mirror) {
                assert_eq!(
                    t.permission,
                    Some(PermissionTier::Privileged),
                    "{mirror} mirrors the authorization change {op} but declares {:?} — \
                     a Write-tier session would reach the same effect through it",
                    t.permission
                );
                checked_mirror += 1;
            }
            assert_eq!(
                classify_command_permission(&kebab),
                PermissionTier::Privileged,
                "classify_command_permission({kebab}) must agree with the tool table"
            );
        }
        assert!(
            checked_direct >= 9,
            "expected every listed op to be a registered tool, only matched {checked_direct}"
        );
        assert!(
            checked_mirror >= 5,
            "expected several command mirrors to exist, only matched {checked_mirror}"
        );
    }

    /// Surface 3. `execute_command` is `Write`, and `dispatch_builtin`
    /// happily takes `kb-share` — which shares the active KB with no
    /// arguments. Without the argument-sensitive escalation this is a
    /// one-call bypass of everything the test above asserts.
    #[test]
    fn execute_command_cannot_launder_an_authorization_change_through_write_tier() {
        for cmd in AUTHORIZATION_CHANGE_OPS {
            for spelling in [cmd.to_string(), cmd.replace('_', "-")] {
                let tier = effective_tier(
                    "execute_command",
                    &json!({ "command": spelling }),
                    PermissionTier::Write,
                );
                assert_eq!(
                    tier,
                    PermissionTier::Privileged,
                    "execute_command {{command: {spelling}}} stayed at {tier:?}"
                );
            }
        }
    }

    /// The self-escalation path: `set_option` is `Write`, so once ADR-084 D7
    /// lands, a `Write` session could set its own ceiling to `privileged`.
    /// Both registry spellings must escalate — matching an underscore-only
    /// name would leave `ai-tier` open, which is exactly the kind of
    /// half-closed gate the option registry's dual spelling invites.
    #[test]
    fn set_option_escalates_only_for_the_permission_tier_option() {
        for spelling in ["ai_tier", "ai-tier", "AI_TIER"] {
            assert_eq!(
                effective_tier(
                    "set_option",
                    &json!({ "option": spelling, "value": "privileged" }),
                    PermissionTier::Write
                ),
                PermissionTier::Privileged,
                "set_option {spelling} must require Privileged"
            );
        }
        // ...and nothing else moves. Sampled across the whole registry rather
        // than one hand-picked "unicorn" option (principle #14): every other
        // registered option must still be settable at the declared tier, or
        // this guard has quietly become "raise set_option wholesale".
        let registry = OptionRegistry::new();
        let mut others = 0usize;
        for opt in registry.list() {
            if is_permission_tier_option(&opt.name) {
                continue;
            }
            assert_eq!(
                effective_tier(
                    "set_option",
                    &json!({ "option": opt.name, "value": "x" }),
                    PermissionTier::Write
                ),
                PermissionTier::Write,
                "setting '{}' must not have been escalated",
                opt.name
            );
            others += 1;
        }
        assert!(others > 100, "sanity: only {others} other options checked");
    }

    /// `:set ai-tier privileged` routed through `execute_command` is the same
    /// escalation wearing a different hat.
    #[test]
    fn execute_command_option_setters_escalate() {
        for line in [
            "set ai-tier privileged",
            "set-save ai_tier privileged",
            "set-local ai-tier privileged",
        ] {
            assert_eq!(
                effective_tier(
                    "execute_command",
                    &json!({ "command": line }),
                    PermissionTier::Write
                ),
                PermissionTier::Privileged,
                "execute_command {{command: {line:?}}} must require Privileged"
            );
        }
    }

    /// Escalation must be monotone — it may add a requirement, never remove
    /// one. A `Shell`-tier tool passing through here must not come out
    /// `Write`, and an unrelated call must come out exactly as declared.
    #[test]
    fn effective_tier_never_lowers_a_declared_tier() {
        for declared in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            for (tool, args) in [
                ("set_option", json!({"option": "line_numbers"})),
                ("set_option", json!({"option": "ai_tier"})),
                ("execute_command", json!({"command": "undo"})),
                ("execute_command", json!({"command": "kb-share"})),
                ("buffer_write", json!({"content": "x"})),
                // Malformed/absent arguments must not panic or downgrade.
                ("set_option", json!({})),
                ("set_option", json!({"option": 7})),
                ("execute_command", json!({"command": ""})),
                ("execute_command", json!(null)),
            ] {
                let got = effective_tier(tool, &args, declared);
                assert!(
                    got >= declared,
                    "effective_tier({tool}, {args}, {declared:?}) lowered to {got:?}"
                );
            }
        }
    }

    /// The omissions are a decision, not an oversight — so they are pinned.
    /// If one of these is later judged an authorization change, this test is
    /// where the reasoning gets revisited.
    #[test]
    fn deliberate_omissions_stay_out_of_the_privileged_set() {
        for op in DELIBERATELY_NOT_AUTHORIZATION_CHANGES {
            assert!(
                !is_authorization_change(op),
                "{op} is in both lists — decide which"
            );
            assert!(
                !AUTHORIZATION_CHANGE_OPS.contains(op),
                "{op} appears in both lists"
            );
        }
    }

    #[test]
    fn normalize_op_folds_every_surface_spelling_together() {
        for name in [
            "kb_add_member",
            "kb-add-member",
            "command_kb_add_member",
            "command_kb-add-member",
            "KB_ADD_MEMBER",
        ] {
            assert_eq!(normalize_op(name), "kb_add_member", "for {name}");
            assert!(is_authorization_change(name), "for {name}");
        }
        // A near-miss must NOT match: substring/prefix matching here would
        // silently privilege unrelated future tools.
        for name in ["kb_add_member_note", "xkb_add_member", "kb_add_link"] {
            assert!(!is_authorization_change(name), "for {name}");
        }
    }
}
