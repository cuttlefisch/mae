//! **A command mirror must never be a weaker route to a tool's effect.**
//!
//! MAE exposes most editor commands to the agent three ways: the hand-authored
//! MCP tool, the generated `command_<name>` mirror, and `execute_command` with the
//! command name as an argument. The first carries a deliberate
//! `PermissionTier`; the other two both take theirs from
//! `classify_command_permission`, whose default is `_ => Write`.
//!
//! So raising a tool's tier does nothing on its own. That has now been the defect
//! twice:
//!
//! * **`kb_share`** — raised to `Privileged`, its command left at `Write`. Fixed
//!   by `is_authorization_change`, and `categories.rs` records the reasoning.
//! * **`kb_raw_query`** — raised to `Privileged` by ADR-085 because arbitrary
//!   Datalog reaches every relation in the store, bypassing every per-tool result
//!   filter. Its command stayed `Write` until this guard was written.
//!
//! Two instances of one shape is a class, so this asserts the *property* rather
//! than the two instances: for every hand-authored tool with a same-named
//! command, the command's tier is at least the tool's. A third instance cannot be
//! introduced without failing here.

use mae_ai::tools::authorization::normalize_op;
use mae_ai::{classify_command_permission, PermissionTier};

/// Every hand-authored tool paired with the **registered command** it mirrors.
///
/// Derived from the tool definitions and the real command registry rather than
/// listed, so a new tool or command is covered the moment it exists.
///
/// Restricted to commands that genuinely exist. `classify_command_permission`
/// answers `Write` for any name, so pairing on the name alone reports routes that
/// are not routes -- `pkg_sync`, `run_build` and `ai_set_budget` are tools with no
/// registered command twin. A guard that cries wolf gets disabled, which is the
/// failure mode E1 documented for the file-size ratchet.
fn tool_command_pairs() -> Vec<(String, PermissionTier, String)> {
    let registry = mae_core::CommandRegistry::with_builtins();
    let registered: std::collections::HashSet<String> = registry
        .list_commands()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert!(
        registered.len() > 100,
        "the command registry looks empty ({} commands) -- this guard would pass \
         vacuously",
        registered.len()
    );
    mae_ai::tools::ai_specific_tools(&mae_core::OptionRegistry::new())
        .into_iter()
        .filter_map(|t| {
            let tier = t.permission?;
            // The command spelling of `kb_raw_query` is `kb-raw-query`.
            let command = t.name.replace('_', "-");
            if !registered.contains(&command) {
                return None;
            }
            Some((t.name, tier, command))
        })
        .collect()
}

#[test]
fn no_command_mirror_is_a_weaker_route_than_its_tool() {
    let mut offences = Vec::new();
    for (tool, tool_tier, command) in tool_command_pairs() {
        let cmd_tier = classify_command_permission(&command);
        if cmd_tier < tool_tier {
            offences.push(format!(
                "  {tool} is {tool_tier:?} but the command `{command}` \
                 (reachable via execute_command AND command_{}) is {cmd_tier:?}",
                normalize_op(&command)
            ));
        }
    }
    assert!(
        offences.is_empty(),
        "a command mirror is a weaker route to a tool's effect, which makes the \
         tool's tier decorative:\n{}\n\nRaise the command in \
         `classify_command_permission`, next to the `is_authorization_change` and \
         `is_raw_datalog_op` arms that fixed the previous two instances.",
        offences.join("\n")
    );
}

/// The specific regression: the three routes to arbitrary Datalog must agree.
#[test]
fn every_route_to_arbitrary_datalog_is_privileged() {
    assert_eq!(
        classify_command_permission("kb-raw-query"),
        PermissionTier::Privileged,
        "`:kb-raw-query` is arbitrary Datalog; the generated command_kb_raw_query \
         mirror takes its tier from here"
    );
}

/// `:kb-agenda custom <datalog>` is argument-sensitive: the command name alone is
/// an ordinary read, so only inspecting the argument catches it.
///
/// This is the capability A5 removed from the `kb_agenda` tool. The human keeps it
/// on the command surface deliberately (principle #16 — the asymmetry IS the
/// control), so an agent driving `execute_command` must not silently regain it.
#[test]
fn kb_agenda_custom_escalates_but_ordinary_filters_do_not() {
    use mae_ai::tools::authorization::effective_tier;
    let tier = |line: &str| {
        effective_tier(
            "execute_command",
            &serde_json::json!({ "command": line }),
            PermissionTier::Write,
        )
    };

    // The attacker's line: arbitrary Datalog through a ReadOnly-looking filter.
    assert_eq!(
        tier("kb-agenda custom ?[id] := *node_versions{id}"),
        PermissionTier::Privileged,
        "`kb-agenda custom` executes its argument verbatim against every relation"
    );
    assert_eq!(
        tier("kb-raw-query ?[id] := *nodes{id}"),
        PermissionTier::Privileged
    );

    // ...while every filter that is NOT arbitrary Datalog stays ordinary. A fix
    // that escalated all of `kb-agenda` would break a core read tool, which is
    // why the argument is inspected rather than the name.
    for ordinary in [
        "kb-agenda todo TODO",
        "kb-agenda orphan",
        "kb-agenda stale 30",
        "kb-agenda dead-end",
        "kb-agenda",
    ] {
        assert_eq!(
            tier(ordinary),
            PermissionTier::Write,
            "`{ordinary}` is an ordinary agenda read and must not escalate"
        );
    }
}
