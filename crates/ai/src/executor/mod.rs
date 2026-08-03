mod ai_exec;
mod collab_exec;
mod core_exec;
mod dap_exec;
pub(crate) mod grading;
mod kb_exec;
mod lsp_exec;
pub(crate) mod model_exam;
mod perf;
mod permission;
pub mod sandbox;
pub(crate) mod self_test;
mod session_exec;
mod shell_exec;
mod sync_exec;
mod tool_dispatch;

#[cfg(test)]
use mae_core::Editor;

#[cfg(test)]
use crate::tools::PermissionPolicy;
use crate::types::*;

pub use tool_dispatch::{execute_tool, execute_tool_with_requester};
// Exposed crate-wide so `crate::tools`' name-sanitisation round-trip test
// (paired with `crate::tools::sanitize_command_name`) can reach it without
// duplicating the decode logic. Test-only: its one consumer
// (`crate::tools::name_roundtrip_tests`) is itself `#[cfg(test)]`, so the
// plain (non-test) lib target has no user of this re-export -- pre-existing
// `unused_imports` warning under `cargo clippy --all-targets -D warnings`,
// unrelated to ADR-087; fixed opportunistically while getting a clean
// clippy run for this change.
#[cfg(test)]
pub(crate) use tool_dispatch::unsanitize_command_name;

/// What kind of deferred tool call is pending (LSP or DAP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredKind {
    LspDefinition,
    LspReferences,
    LspHover,
    LspWorkspaceSymbol,
    LspDocumentSymbols,
    DapStart,
    DapContinue,
    DapStep,
}

impl DeferredKind {
    /// True for LSP-originated deferred calls.
    pub fn is_lsp(self) -> bool {
        matches!(
            self,
            DeferredKind::LspDefinition
                | DeferredKind::LspReferences
                | DeferredKind::LspHover
                | DeferredKind::LspWorkspaceSymbol
                | DeferredKind::LspDocumentSymbols
        )
    }

    /// True for DAP-originated deferred calls.
    pub fn is_dap(self) -> bool {
        matches!(
            self,
            DeferredKind::DapStart | DeferredKind::DapContinue | DeferredKind::DapStep
        )
    }

    /// Inverse of [`DeferredKind::tool_name`].
    ///
    /// Needed because the Scheme surface stores a *name* rather than a
    /// `DeferredKind` (`mae-core`, where `Editor::scheme_async` lives, cannot
    /// depend on `mae-ai`), and must map back to reuse
    /// `lsp_bridge::try_complete_deferred` rather than re-deriving the
    /// event→payload conversion a second time.
    ///
    /// @ai-caution: [dispatch] Keep this exhaustive and in step with
    /// `tool_name`; `deferred_kind_names_round_trip` fails if they diverge.
    pub fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "lsp_definition" => Some(DeferredKind::LspDefinition),
            "lsp_references" => Some(DeferredKind::LspReferences),
            "lsp_hover" => Some(DeferredKind::LspHover),
            "lsp_workspace_symbol" => Some(DeferredKind::LspWorkspaceSymbol),
            "lsp_document_symbols" => Some(DeferredKind::LspDocumentSymbols),
            "dap_start" => Some(DeferredKind::DapStart),
            "dap_continue" => Some(DeferredKind::DapContinue),
            "dap_step" => Some(DeferredKind::DapStep),
            _ => None,
        }
    }

    /// Return the tool name string for this deferred kind.
    pub fn tool_name(self) -> &'static str {
        match self {
            DeferredKind::LspDefinition => "lsp_definition",
            DeferredKind::LspReferences => "lsp_references",
            DeferredKind::LspHover => "lsp_hover",
            DeferredKind::LspWorkspaceSymbol => "lsp_workspace_symbol",
            DeferredKind::LspDocumentSymbols => "lsp_document_symbols",
            DeferredKind::DapStart => "dap_start",
            DeferredKind::DapContinue => "dap_continue",
            DeferredKind::DapStep => "dap_step",
        }
    }
}

/// Result of executing a tool call — either immediately available or
/// deferred until an async response (e.g. from the LSP task) arrives.
#[derive(Debug)]
pub enum ExecuteResult {
    /// Tool completed synchronously.
    Immediate(ToolResult),
    /// Tool queued an async request (e.g. LSP). The caller must hold the
    /// reply channel open and complete it when the matching event arrives.
    Deferred {
        tool_call_id: String,
        kind: DeferredKind,
    },
    /// ADR-090: the policy answered `Ask` — the call exceeds the
    /// auto-approval ceiling, but nothing forbids it. **Nothing was
    /// executed.**
    ///
    /// @ai-caution: [security] This variant exists so that `rustc` forces
    /// every surface to say what it does with `Ask` (ADR-090 D3, the same
    /// Capsicum "change the signature to find the missed paths" technique
    /// ADR-084 D3 uses). A surface either prompts a human and re-dispatches
    /// with `PermissionPolicy::with_one_time_approval`, or it declares itself
    /// non-interactive and converts this to a denial via
    /// [`ApprovalRequest::into_denied`]. Treating it as success — or matching
    /// it with a `_ =>` arm that falls through to execution — is the one
    /// outcome the ADR forbids.
    NeedsApproval(ApprovalRequest),
}

/// A tool call parked awaiting a human's answer to [`ExecuteResult::NeedsApproval`].
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    /// The tier the human must be shown — already through `effective_tier`,
    /// so it reflects the call's real blast radius, not just the tool's
    /// declared floor.
    pub tier: PermissionTier,
    /// The ceiling that was exceeded, for the message.
    pub auto_approve_up_to: PermissionTier,
}

impl ApprovalRequest {
    /// The non-interactive mapping (ADR-090 D3): `Ask` becomes a denial, and
    /// says so, naming the ceiling an operator can raise. `surface` names the
    /// caller ("--prompt mode", "external MCP dispatch") so the message is
    /// actionable rather than generic.
    ///
    /// One implementation, shared by every non-interactive surface — the
    /// wording is not re-typed per call site (principle #8).
    pub fn into_denied(self, surface: &str) -> ToolResult {
        let output = crate::tools::ask_denied_message(
            &self.tool_name,
            self.tier,
            self.auto_approve_up_to,
            surface,
        );
        ToolResult {
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            success: false,
            output,
        }
    }

    /// The prompt line for an interactive surface.
    pub fn prompt_line(&self) -> String {
        crate::tools::ask_message(&self.tool_name, self.tier, self.auto_approve_up_to)
    }
}

// Convenience re-export for tests that use `build_self_test_plan` directly.
#[cfg(test)]
use self_test::build_self_test_plan;

// `build_self_test_plan` moved to self_test.rs; re-exported above for tests.
// `execute_tool` + `dispatch_tool` moved to tool_dispatch.rs; re-exported above.
// `format_permissions_info` moved to permission.rs.
// `execute_perf_stats` + `execute_perf_benchmark` moved to perf.rs.

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "authorization_tests.rs"]
mod authorization_tests;

#[cfg(test)]
#[path = "session_dispatch_tests.rs"]
mod session_dispatch_tests;

#[cfg(test)]
mod deferred_kind_tests {
    use super::DeferredKind;

    /// Every variant, not a sample: `from_tool_name` must be the exact inverse
    /// of `tool_name`, or a Scheme-initiated LSP request would sit on
    /// `'pending` forever because its stored name no longer maps back to a
    /// kind. Enumerated explicitly so adding a variant without updating
    /// `from_tool_name` fails here rather than in production.
    const ALL: &[DeferredKind] = &[
        DeferredKind::LspDefinition,
        DeferredKind::LspReferences,
        DeferredKind::LspHover,
        DeferredKind::LspWorkspaceSymbol,
        DeferredKind::LspDocumentSymbols,
        DeferredKind::DapStart,
        DeferredKind::DapContinue,
        DeferredKind::DapStep,
    ];

    #[test]
    fn deferred_kind_names_round_trip() {
        for kind in ALL {
            assert_eq!(
                DeferredKind::from_tool_name(kind.tool_name()),
                Some(*kind),
                "{:?} did not round-trip through its tool name",
                kind
            );
        }
        // Names are distinct, so the round trip is a bijection and not two
        // variants collapsing onto one name.
        let mut names: Vec<&str> = ALL.iter().map(|k| k.tool_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len(), "two DeferredKinds share a name");
    }

    #[test]
    fn an_unknown_or_adjacent_name_is_rejected_not_guessed() {
        for bogus in [
            "",
            "lsp",
            "lsp_definitions",    // plural — a plausible typo
            "LSP_DEFINITION",     // case must matter
            "lsp_diagnostics",    // a real tool, but NOT a deferred one
            "dap_set_breakpoint", // synchronous, must not map to a deferred kind
            "kb_search",
        ] {
            assert_eq!(
                DeferredKind::from_tool_name(bogus),
                None,
                "{bogus:?} must not resolve to a deferred kind"
            );
        }
    }
}
