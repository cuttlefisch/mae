//! Property test for the tool-name sanitisation round trip (#521-era
//! permission-enforcement audit, defect #3): `crate::tools::
//! sanitize_command_name` (`crates/ai/src/tools/mod.rs`) encodes a
//! `CommandRegistry` command name into the `[a-z0-9_]` alphabet an MCP/LLM
//! tool name must use; `crate::executor::unsanitize_command_name`
//! (`crates/ai/src/executor/tool_dispatch.rs`) is supposed to invert it so
//! `execute_registry_command` can recover the original command name and
//! dispatch it. Before this fix, the encoder mapped `-` -> `_` and simply
//! *deleted* `!`, which is not invertible — `ai-status!` encoded to
//! `ai_status`, and decoding `ai_status` back gives `ai-status`, not
//! `ai-status!`. That made the `ai-status!` command permanently
//! unreachable through the `command_ai_status` MCP tool: no argument the
//! agent could pass would ever re-derive the `!`.
//!
//! `sanitize_command_name` was changed to escape any character outside
//! `[a-z0-9-]` as `_{hex}_` instead of dropping it, and
//! `unsanitize_command_name` now decodes that escape. This test is the
//! property that change is supposed to establish: for every name actually
//! in `CommandRegistry::with_builtins()`, decode(encode(name)) == name.

use mae_core::CommandRegistry;

use super::sanitize_command_name;
use crate::executor::unsanitize_command_name;

/// The property test itself, run against the real registry (not hand-picked
/// samples — CLAUDE.md principle #14). Every single registered command name
/// must round-trip, not just a spot-checked subset.
#[test]
fn all_registered_command_names_round_trip() {
    let reg = CommandRegistry::with_builtins();
    let mut failures: Vec<String> = Vec::new();
    for name in reg.list_names() {
        let encoded = sanitize_command_name(name);
        let decoded = unsanitize_command_name(&encoded);
        if decoded != name {
            failures.push(format!("{name:?} -> {encoded:?} -> {decoded:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} command name(s) do not round-trip through sanitize/unsanitize:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The specific regression this fix closes: `ai-status!` is the one
/// registered name (as of this writing) containing a character outside
/// `[a-z0-9-]`. Named explicitly, rather than relying solely on the
/// registry sweep above, so a future refactor that accidentally special-
/// cases `!` back out (instead of keeping the general escape mechanism)
/// fails with a message that points straight at the regressed command.
#[test]
fn ai_status_bang_round_trips() {
    let encoded = sanitize_command_name("ai-status!");
    let decoded = unsanitize_command_name(&encoded);
    assert_eq!(decoded, "ai-status!");
    // Encoding must not just "happen to" work out — the resulting tool-name
    // suffix must actually be a valid MCP tool name (alphanumeric +
    // underscore only), matching `all_tool_names_are_alphanumeric_underscore`.
    assert!(
        encoded
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "sanitized name must stay within [a-z0-9_]: {encoded:?}"
    );
}

/// Sanitisation must still produce the exact same tool-name suffixes as
/// before for plain kebab-case names with no unusual characters — the
/// escape mechanism is additive, not a wire-format break for the vast
/// majority of already-shipped `command_*` tool names (API stability,
/// CLAUDE.md's "API Stability" section).
#[test]
fn plain_kebab_case_names_unchanged_by_escape_mechanism() {
    assert_eq!(sanitize_command_name("move-down"), "move_down");
    assert_eq!(sanitize_command_name("kb-share-p2p"), "kb_share_p2p");
    assert_eq!(unsanitize_command_name("move_down"), "move-down");
    assert_eq!(unsanitize_command_name("kb_share_p2p"), "kb-share-p2p");
}

/// A round-trip property in the other direction: decode(x) then re-encode
/// should reproduce a canonical sanitized form. Guards against an
/// unsanitize implementation that "succeeds" on the forward direction but
/// is not actually a true inverse (e.g. by being overly permissive and
/// accepting inputs `sanitize_command_name` would never produce).
#[test]
fn encode_decode_is_idempotent_for_registry_names() {
    let reg = CommandRegistry::with_builtins();
    for name in reg.list_names() {
        let encoded = sanitize_command_name(name);
        let re_encoded = sanitize_command_name(&unsanitize_command_name(&encoded));
        assert_eq!(
            encoded, re_encoded,
            "encode/decode/encode not idempotent for {name:?}"
        );
    }
}
