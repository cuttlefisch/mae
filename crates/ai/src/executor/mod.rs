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
mod shell_exec;
mod sync_exec;
mod tool_dispatch;

#[cfg(test)]
use mae_core::Editor;

#[cfg(test)]
use crate::tools::PermissionPolicy;
use crate::types::*;

pub use tool_dispatch::execute_tool;

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
