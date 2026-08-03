//! Which advertised tools an external MCP client can actually *call*
//! (decision #9, ADR-091).
//!
//! Nine tools used to be registered in `ai_specific_tools` — and therefore
//! present in an external client's `tools/list`, `search_tools`, and
//! `request_tools` results — while being handled only inside the embedded
//! `AgentSession`'s own loop. An external `tools/call` for any of them fell
//! through `dispatch_tool` to `Err("Unknown tool: …")`. `ask_user` was at the
//! default Core tier, so it sat in the *first* list any paired external agent
//! saw.
//!
//! The nine split cleanly in two, and each half gets a different fix:
//!
//! - **Six are not inherently interactive.** They read or mutate session-local
//!   state, so ADR-091's session handle (`Editor::agent_session_mut`) makes
//!   them genuinely dispatchable. They are now routed and are *not* in this
//!   module's exclusion list.
//! - **Three are inherently interactive** — [`EMBEDDED_SESSION_ONLY_TOOLS`].
//!   `ask_user` and `propose_changes` park the session task on a
//!   `tokio::sync::oneshot` awaiting a human reply, and `delegate` spawns a
//!   sub-agent. Making those work mid-`tools/call` for an external client is a
//!   UX question, not a wiring one. Per ADR-085's stated shape — *"the fix is
//!   that they are not offered, not that they are offered and then refused"* —
//!   they are withheld from every external discovery surface and remain
//!   embedded-session-only.
//!
//! @ai-caution: [dispatch] Withholding is scoped to *external* callers
//! (`Editor::is_external_mcp_dispatch`). The embedded `AgentSession` must keep
//! seeing all three — it is the one context where they work. A filter applied
//! unconditionally would silently disable `ask_user` for the human's own
//! agent.
//!
//! @stability: experimental

use crate::types::ToolDefinition;

/// Tools that only the embedded `AgentSession` can execute, because executing
/// them means suspending an agent turn on a human reply (or spawning a
/// sub-agent that does). Withheld from external MCP discovery.
///
/// This is not a permission decision — it is a capability one. An external
/// client at `Privileged` tier still cannot run these, because there is
/// nothing on its side of the connection to run them *with*.
pub const EMBEDDED_SESSION_ONLY_TOOLS: &[&str] = &["ask_user", "propose_changes", "delegate"];

/// The six that ADR-091's session handle makes dispatchable. Named so the
/// invariant tests can assert the positive claim ("these ARE routable") rather
/// than only the negative one, and so a regression that un-wires one is caught
/// by name.
pub const SESSION_SCOPED_DISPATCHABLE_TOOLS: &[&str] = &[
    "ai_set_mode",
    "ai_set_profile",
    "ai_set_budget",
    "log_activity",
    "read_transcript",
    "web_fetch",
];

/// Should `name` be hidden from an external MCP client's discovery surfaces?
pub fn is_embedded_session_only(name: &str) -> bool {
    EMBEDDED_SESSION_ONLY_TOOLS.contains(&name)
}

/// Filter a tool list down to what an external MCP client may discover.
///
/// The single helper for every external discovery surface — `tools/list`
/// (`crates/mae/src/main.rs`), `search_tools`, and `request_tools` — so the
/// three cannot be closed on one surface and left open on another. That
/// three-surface split is exactly how decision #6's KB tools stayed reachable
/// after being "fixed" in one place.
pub fn external_discovery_tools(tools: &[ToolDefinition]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .filter(|t| !is_embedded_session_only(&t.name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ai_specific_tools;
    use mae_core::OptionRegistry;

    /// All nine of decision #9's phantoms are still *registered* — the fix is
    /// not deletion. Six became routable, three became undiscoverable.
    #[test]
    fn all_nine_phantoms_are_accounted_for_and_still_registered() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        for name in EMBEDDED_SESSION_ONLY_TOOLS
            .iter()
            .chain(SESSION_SCOPED_DISPATCHABLE_TOOLS)
        {
            assert!(
                names.contains(name),
                "{name} is no longer registered — decision #9's fix was to route or \
                 withhold these, not to delete them"
            );
        }
        assert_eq!(
            EMBEDDED_SESSION_ONLY_TOOLS.len() + SESSION_SCOPED_DISPATCHABLE_TOOLS.len(),
            9,
            "decision #9 enumerated nine tools; the two lists must still partition them"
        );
        // The two lists must be disjoint, or a tool would be claimed both
        // routable and withheld.
        for name in EMBEDDED_SESSION_ONLY_TOOLS {
            assert!(
                !SESSION_SCOPED_DISPATCHABLE_TOOLS.contains(name),
                "{name} is in both lists"
            );
        }
    }

    #[test]
    fn external_discovery_withholds_exactly_the_interactive_three() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let external = external_discovery_tools(&tools);
        assert_eq!(
            external.len(),
            tools.len() - EMBEDDED_SESSION_ONLY_TOOLS.len(),
            "the filter removed something other than the three interactive tools"
        );
        for name in EMBEDDED_SESSION_ONLY_TOOLS {
            assert!(
                !external.iter().any(|t| t.name == *name),
                "{name} survived the external-discovery filter"
            );
        }
        for name in SESSION_SCOPED_DISPATCHABLE_TOOLS {
            assert!(
                external.iter().any(|t| t.name == *name),
                "{name} is dispatchable and must still be discoverable"
            );
        }
    }

    /// Source files that make up `dispatch_tool`'s routing chain. A tool name
    /// that appears in none of them cannot be routed, and
    /// `dispatch_tool` will answer `Unknown tool: <name>`.
    const DISPATCH_SOURCES: &[(&str, &str)] = &[
        (
            "executor/tool_dispatch.rs",
            include_str!("../executor/tool_dispatch.rs"),
        ),
        (
            "executor/core_exec.rs",
            include_str!("../executor/core_exec.rs"),
        ),
        (
            "executor/session_exec.rs",
            include_str!("../executor/session_exec.rs"),
        ),
        (
            "executor/ai_exec.rs",
            include_str!("../executor/ai_exec.rs"),
        ),
        (
            "executor/lsp_exec.rs",
            include_str!("../executor/lsp_exec.rs"),
        ),
        (
            "executor/dap_exec.rs",
            include_str!("../executor/dap_exec.rs"),
        ),
        (
            "executor/kb_exec.rs",
            include_str!("../executor/kb_exec.rs"),
        ),
        (
            "executor/shell_exec.rs",
            include_str!("../executor/shell_exec.rs"),
        ),
        (
            "executor/sync_exec.rs",
            include_str!("../executor/sync_exec.rs"),
        ),
        (
            "executor/collab_exec.rs",
            include_str!("../executor/collab_exec.rs"),
        ),
        ("executor/perf.rs", include_str!("../executor/perf.rs")),
        (
            "executor/self_test.rs",
            include_str!("../executor/self_test.rs"),
        ),
        (
            "executor/model_exam.rs",
            include_str!("../executor/model_exam.rs"),
        ),
    ];

    /// **The invariant decision #9 exists to install.** Nothing MAE advertises
    /// to an external MCP client may be a tool `dispatch_tool` cannot route.
    ///
    /// This is the check whose absence let nine tools sit in `tools/list` for
    /// as long as they did: every individual tool was correct in isolation
    /// (registered, tiered, schema'd), and the defect only existed in the
    /// relationship between the registry and the dispatcher — which nothing
    /// asserted.
    ///
    /// Source-text based rather than by dispatching every tool: actually
    /// calling ~210 tools to see which answer "Unknown tool" would mean
    /// really running `editor_save_state`, `kb_reimport`, `run_build`, and
    /// every `command_*` mirror including `quit`. Reading the routing chain
    /// answers the same question without executing anything. It is a
    /// heuristic over source text — the same class as
    /// `dispatch_contract_tests` — so a surprising result is a prompt to look
    /// at the dispatcher, not to add a name to an exemption list.
    #[test]
    fn no_advertised_tool_is_unroutable() {
        let all = ai_specific_tools(&OptionRegistry::new());
        let advertised = external_discovery_tools(&all);
        assert!(
            advertised.len() > 150,
            "sanity: only {} tools advertised",
            advertised.len()
        );

        let mut unroutable: Vec<String> = Vec::new();
        for tool in &advertised {
            // `command_*` tools are routed by prefix, not by name
            // (`tool_dispatch.rs`'s `strip_prefix("command_")` branch), so
            // their individual names are legitimately absent.
            if tool.name.starts_with("command_") {
                continue;
            }
            let needle = format!("\"{}\"", tool.name);
            let routed = DISPATCH_SOURCES
                .iter()
                .any(|(_, src)| src.contains(needle.as_str()));
            if !routed {
                unroutable.push(tool.name.clone());
            }
        }
        assert!(
            unroutable.is_empty(),
            "these tools are advertised to external MCP clients but appear nowhere in \
             dispatch_tool's routing chain, so a `tools/call` for them answers \
             `Unknown tool` ({} of {} advertised): {unroutable:?}",
            unroutable.len(),
            advertised.len()
        );
    }

    /// The other half of the invariant: the three withheld tools are withheld
    /// *because* they are unroutable, so if one ever becomes routable this
    /// test fails and forces the exclusion to be reconsidered rather than
    /// left as a stale hard-coded list.
    #[test]
    fn the_withheld_three_are_still_the_unroutable_ones() {
        for name in EMBEDDED_SESSION_ONLY_TOOLS {
            let needle = format!("\"{name}\"");
            let routed = DISPATCH_SOURCES
                .iter()
                .any(|(_, src)| src.contains(needle.as_str()));
            assert!(
                !routed,
                "{name} now appears in dispatch_tool's routing chain — if it is genuinely \
                 dispatchable, move it from EMBEDDED_SESSION_ONLY_TOOLS to \
                 SESSION_SCOPED_DISPATCHABLE_TOOLS instead of withholding it"
            );
        }
    }

    /// `ask_user` sitting at Core tier is what made this urgent: Core is the
    /// list a fresh external client sees before it knows `search_tools`
    /// exists. Pinned so a future re-tiering does not quietly reintroduce the
    /// worst version of the bug.
    #[test]
    fn the_withheld_tools_are_gone_even_from_the_core_tier_list() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let external = external_discovery_tools(&tools);
        let core: Vec<&str> = external
            .iter()
            .filter(|t| crate::tools::classify_tool_tier(&t.name) == crate::tools::ToolTier::Core)
            .map(|t| t.name.as_str())
            .collect();
        for name in EMBEDDED_SESSION_ONLY_TOOLS {
            assert!(!core.contains(name), "{name} is still in the Core list");
        }
        assert!(
            core.len() > 10,
            "sanity: the Core list collapsed to {} tools",
            core.len()
        );
    }
}
