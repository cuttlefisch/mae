//! ADVERSARIAL: a `command_<name>` mirror must never be a weaker route to an
//! effect than the hand-authored tool that reaches the same one.
//!
//! Split out of `categories.rs` rather than blessing its growth — the
//! structural ratchet is doing its job, and a tier-parity suite is exactly the
//! kind of thing that should not push a classifier module past its ceiling.

use super::categories::classify_command_permission;
use crate::tools::ai_specific_tools;
use crate::types::PermissionTier;
use mae_core::options::OptionRegistry;
use mae_core::CommandRegistry;

/// Every registered command is also exposed as a `command_<name>` MCP tool
/// (`tools_from_registry`). Those mirrors are tiered by
/// `classify_command_permission`, whose default is `Write` — and because
/// `ToolCategory::Commands` is not read-flavoured, the ADR-085 registry-wide
/// tier audit never looks at any of them.
///
/// The result was tier laundering: `babel_execute` is `Shell` while
/// `command_babel_execute` was `Write`; `terminal_spawn` is `Shell` while
/// `command_terminal` was `Write`; `terminal_send` is `Shell` while
/// `command_send_to_shell` was `Write`. Three `Write` calls —
/// `command_terminal`, `buffer_write`, `command_send_to_shell` — were RCE.
///
/// This generalises the shape the maintainers already built for the nine
/// `AUTHORIZATION_CHANGE_OPS`: a mirror must never be *weaker* than the
/// hand-authored tool that reaches the same effect.
#[test]
fn command_mirror_tiers_match_their_hand_authored_twins() {
    let tools = ai_specific_tools(&OptionRegistry::new());
    let registry = CommandRegistry::with_builtins();

    // (command name, hand-authored tool reaching the same effect)
    let twins = [
        ("babel-execute", "babel_execute"),
        ("babel-execute-all", "babel_execute"),
        ("babel-tangle", "babel_tangle"),
        ("terminal", "terminal_spawn"),
        ("terminal-here", "terminal_spawn"),
        ("send-to-shell", "terminal_send"),
        ("send-region-to-shell", "terminal_send"),
        ("kb-register", "kb_register"),
        ("kb-reimport", "kb_reimport"),
    ];

    let mut checked = 0usize;
    for (command, tool) in twins {
        // Skip pairs whose halves no longer exist rather than asserting on
        // a renamed thing — but count, so an empty run cannot pass.
        let Some(twin) = tools.iter().find(|t| t.name == tool) else {
            continue;
        };
        if registry.get(command).is_none() {
            continue;
        }
        let mirror = classify_command_permission(command);
        // An untiered tool is treated as Privileged everywhere else
        // (`an_untiered_tool_is_treated_as_privileged_not_trusted`), so
        // match that here rather than skipping it.
        let twin_tier = twin.permission.unwrap_or(PermissionTier::Privileged);
        assert!(
            mirror >= twin_tier,
            "command mirror `command_{command}` is {mirror:?} but the \
             hand-authored `{tool}` that reaches the same effect is \
             {twin_tier:?}. The mirror is a weaker route to the same \
             capability — raise it in classify_command_permission."
        );
        checked += 1;
    }
    assert!(
        checked >= 7,
        "only {checked} twin pairs resolved; the test is going vacuous as \
         names drift — re-point it rather than letting it pass on nothing"
    );
}
