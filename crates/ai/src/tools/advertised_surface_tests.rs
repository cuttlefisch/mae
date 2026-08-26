//! Story C / R10 — the size of the tool list an external client actually sees.
//!
//! **The finding this exists for:** MAE's full tool surface is ~770 tools — one
//! per registered command plus the hand-authored set. That is **15× Anthropic's
//! stated 30–50 tool-selection degradation threshold** and **6× VS Code's
//! `HARD_TOOL_LIMIT = 128`**. It is upstream of every "which tool will the model
//! reach for?" question, including Story C's own `file_read`-vs-`kb_search` one:
//! steering wording cannot rescue a list the model cannot hold.
//!
//! MAE already has both mechanisms the field settled on, and they are not
//! interchangeable:
//!
//! * **Static, operator-selected toolsets** (`mcp_tool_category_allowlist`) —
//!   what GitHub's MCP server **kept**.
//! * **Progressive discovery** (`request_tools`/`search_tools`) — what GitHub
//!   **built, ran for 13 months, and deleted**, on the reasoning that progressive
//!   discovery belongs at the client/model-API level and *"server-specific
//!   progressive discovery of MCP tools feels increasingly outdated"*.
//!
//! What was missing is the thing that makes either one trustworthy: **a measured
//! bound on the default advertised list, checked in CI**. Without it, the number
//! that matters is a claim in a doc comment, and this repo has already learned
//! that a moving number cannot live in prose.

use super::*;
use crate::tools::dispatchability::external_discovery_tools;
use mae_core::commands::CommandRegistry;
use mae_core::options::OptionRegistry;

/// Anthropic's stated tool-selection degradation threshold. Named rather than
/// inlined, so the assertion below reads as the claim it is testing.
const DEGRADATION_THRESHOLD: usize = 50;

/// VS Code's `HARD_TOOL_LIMIT`. The most permissive number any shipped client
/// in the survey enforces — a list above this is not merely degraded, it is
/// rejected outright by a real consumer.
const VSCODE_HARD_TOOL_LIMIT: usize = 128;

/// Everything a session can dispatch: hand-authored + one mirror per command.
fn full_surface() -> Vec<ToolDefinition> {
    let mut tools = ai_specific_tools(&OptionRegistry::new());
    tools.extend(tools_from_registry(&CommandRegistry::with_builtins()));
    tools
}

/// What a fresh external MCP client is actually offered, tiering on (the
/// default): the Core tier of the externally-advertisable set.
fn default_advertised() -> Vec<ToolDefinition> {
    let all = full_surface();
    external_discovery_tools(&all)
        .into_iter()
        .filter(|t| classify_tool_tier(&t.name) == ToolTier::Core)
        .collect()
}

/// What the default list is **today**, measured. Only ever lower this.
///
/// It is **over** [`DEGRADATION_THRESHOLD`], and that is the finding rather than
/// an oversight — see the test below. Ratcheting on the exact measured value
/// with no tolerance band is what every baseline tool in the field does, and
/// what this repo's own structural gate settled on after a proportional band
/// produced sub-threshold drift.
const MEASURED_DEFAULT_ADVERTISED: usize = 67;

/// **The ratchet.** The default list may shrink; it may not grow.
///
/// The Core tier's own doc comment said "~15 tools" while the measured number
/// was 67 — a hand-maintained figure in prose, drifted 4×, in exactly the way
/// CLAUDE.md records that a moving number cannot survive in prose. The number
/// lives here now, next to the thing that measures it.
#[test]
fn the_default_advertised_tool_list_does_not_grow() {
    let advertised = default_advertised();
    assert!(
        advertised.len() <= MEASURED_DEFAULT_ADVERTISED,
        "a fresh external client is now offered {} tools, up from the recorded {}. \
         Do not raise this constant to make the test pass: tier the addition Extended \
         (still reachable via request_tools/search_tools) or fold it into an existing \
         tool. Advertised: {:?}",
        advertised.len(),
        MEASURED_DEFAULT_ADVERTISED,
        advertised.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    assert!(
        advertised.len() >= 10,
        "sanity: the advertised list collapsed to {} tools, which would make MAE useless \
         to a paired external agent rather than merely noisy",
        advertised.len()
    );
}

/// **The gap, asserted rather than described**, so it cannot quietly become
/// folklore that MAE meets a threshold it does not.
///
/// R10's own recommendation is that re-tiering be settled by the three-arm
/// experiment it specifies — measuring **stale-answer rate**, not tool-call
/// counts — not by one person's judgement about which of 67 tools are
/// "essential". The one controlled measurement in the field went the *wrong*
/// way: an aggressive-steering arm roughly **tripled** the wrong-tool rate. So
/// the gap is recorded and bounded here rather than closed by taste.
///
/// Tracked as issue #800, which carries the three-arm experiment. When it lands
/// and the list comes under the threshold, **delete this test** and tighten the
/// ratchet above.
#[test]
fn the_default_list_is_still_over_the_degradation_threshold_and_says_so() {
    let advertised = default_advertised();
    assert!(
        advertised.len() > DEGRADATION_THRESHOLD,
        "the default advertised list is now {} tools, at or under the \
         {DEGRADATION_THRESHOLD}-tool degradation threshold. That is the goal — delete \
         this test and lower MEASURED_DEFAULT_ADVERTISED to the new figure.",
        advertised.len()
    );
}

/// **The regression this bound exists to catch.** `classify_tool_tier`'s
/// fallthrough is `_ => Extended`, which is the only thing keeping ~560 command
/// mirrors out of the default list. Change that default and the advertised
/// surface goes from tens to hundreds in one line.
#[test]
fn no_generated_command_mirror_is_core_tier() {
    let mirrors = tools_from_registry(&CommandRegistry::with_builtins());
    assert!(
        mirrors.len() > 300,
        "sanity: only {} command mirrors — this test is not measuring what it thinks",
        mirrors.len()
    );
    let core: Vec<&str> = mirrors
        .iter()
        .filter(|t| classify_tool_tier(&t.name) == ToolTier::Core)
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        core.is_empty(),
        "{} generated command mirror(s) reached the Core tier, which is what puts the \
         whole ~770-tool surface in front of a fresh client: {core:?}",
        core.len()
    );
}

/// The untiered path is the one that genuinely exceeds every published limit,
/// so the measurement is recorded rather than left as an assumption. It is a
/// supported configuration (`mcp_tools_tiered_by_default = false`) for a
/// deployment tuned around the full list — the point is that its size is known.
#[test]
fn the_untiered_surface_is_measured_and_is_far_past_every_published_limit() {
    let all = full_surface();
    let advertisable = external_discovery_tools(&all).len();
    assert!(
        advertisable > VSCODE_HARD_TOOL_LIMIT,
        "the untiered surface is {advertisable} tools; if it has genuinely dropped below \
         VS Code's HARD_TOOL_LIMIT of {VSCODE_HARD_TOOL_LIMIT}, that is good news and this \
         test should be rewritten to assert the new state rather than deleted"
    );
    assert!(
        advertisable > DEGRADATION_THRESHOLD * 5,
        "sanity: {advertisable} tools is not the full surface this test means to measure"
    );
}

/// Static, operator-selected toolsets are the mechanism the field kept — so
/// every category an operator can name must actually narrow the surface. A
/// category that selects nothing is an allowlist entry that silently denies
/// everything.
#[test]
fn every_category_an_operator_can_allowlist_selects_at_least_one_tool() {
    let all = full_surface();
    for category in ToolCategory::ALL {
        let n = all
            .iter()
            .filter(|t| classify_tool_category(&t.name) == Some(*category))
            .count();
        assert!(
            n > 0,
            "category {category:?} is offered in the allowlist but selects no tool — an \
             operator setting it would get an empty surface and no explanation"
        );
    }
}
