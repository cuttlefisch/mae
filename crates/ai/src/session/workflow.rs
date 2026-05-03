//! Compaction-resilient workflow tracking for multi-step AI tasks.
//!
//! The WorkflowTracker is a lightweight state machine that lives on AgentSession
//! (not in message history), so it survives context compaction. On every turn it
//! injects a compact status string into the prompt, giving the model perfect
//! orientation even after mid-flight `collapse_transaction`.

use std::fmt;

/// Tracks progress through a multi-step workflow.
/// Lives on AgentSession — survives context compaction.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkflowTracker {
    /// Active workflow name (e.g. "self-test", "refactor", "debug").
    pub workflow: Option<String>,
    /// Ordered list of steps in the workflow.
    pub steps: Vec<WorkflowStep>,
    /// Index of the current step (0-based).
    pub current_step: usize,
    /// Tools that have been called in the current step (for dedup).
    pub step_tools_called: Vec<String>,
    /// Number of times the agent has re-requested the workflow plan.
    pub plan_request_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkflowStep {
    pub name: String,
    pub status: StepStatus,
    /// Short summary of the result (filled on completion).
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Skipped,
    Failed,
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepStatus::Pending => write!(f, " "),
            StepStatus::InProgress => write!(f, "▶"),
            StepStatus::Completed => write!(f, "✓"),
            StepStatus::Skipped => write!(f, "⊘"),
            StepStatus::Failed => write!(f, "✗"),
        }
    }
}

impl WorkflowTracker {
    /// Initialize a new workflow from a task plan.
    pub fn start_workflow(&mut self, name: String, step_names: Vec<String>) {
        let mut steps: Vec<WorkflowStep> = step_names
            .into_iter()
            .map(|n| WorkflowStep {
                name: n,
                status: StepStatus::Pending,
                result_summary: None,
            })
            .collect();
        if let Some(first) = steps.first_mut() {
            first.status = StepStatus::InProgress;
        }
        self.workflow = Some(name);
        self.steps = steps;
        self.current_step = 0;
        self.step_tools_called.clear();
        self.plan_request_count = 1; // The initial call counts
    }

    /// Mark current step complete and move to next.
    pub fn advance(&mut self, summary: String) {
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::Completed;
            step.result_summary = Some(summary);
        }
        self.current_step += 1;
        self.step_tools_called.clear();
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::InProgress;
        }
    }

    /// Skip current step.
    pub fn skip(&mut self, reason: String) {
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::Skipped;
            step.result_summary = Some(reason);
        }
        self.current_step += 1;
        self.step_tools_called.clear();
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::InProgress;
        }
    }

    /// Mark current step failed and move to next.
    pub fn fail(&mut self, reason: String) {
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::Failed;
            step.result_summary = Some(reason);
        }
        self.current_step += 1;
        self.step_tools_called.clear();
        if let Some(step) = self.steps.get_mut(self.current_step) {
            step.status = StepStatus::InProgress;
        }
    }

    /// Record a tool call in the current step.
    pub fn record_tool(&mut self, tool_name: &str) {
        self.step_tools_called.push(tool_name.to_string());
    }

    /// Whether a workflow is currently active.
    pub fn is_active(&self) -> bool {
        self.workflow.is_some() && !self.steps.is_empty()
    }

    /// Whether all steps are done/skipped/failed.
    pub fn is_complete(&self) -> bool {
        self.is_active()
            && self.steps.iter().all(|s| {
                matches!(
                    s.status,
                    StepStatus::Completed | StepStatus::Skipped | StepStatus::Failed
                )
            })
    }

    /// Name of the current step, or "" if out of bounds.
    pub fn current_step_name(&self) -> &str {
        self.steps
            .get(self.current_step)
            .map(|s| s.name.as_str())
            .unwrap_or("")
    }

    /// Produce a compact per-turn context injection string (~150 tokens).
    /// Includes completed results summary and explicit next-action directive.
    pub fn context_injection(&self) -> String {
        if !self.is_active() {
            return String::new();
        }

        let workflow_name = self.workflow.as_deref().unwrap_or("unknown");
        let total = self.steps.len();
        let step_num = (self.current_step + 1).min(total);
        let current_name = self.current_step_name();

        let mut done = Vec::new();
        let mut remaining = Vec::new();
        let mut results = Vec::new();
        for (i, step) in self.steps.iter().enumerate() {
            match step.status {
                StepStatus::Completed | StepStatus::Skipped | StepStatus::Failed => {
                    let summary = step.result_summary.as_deref().unwrap_or("done");
                    done.push(format!("{}({})", step.name, step.status));
                    results.push(format!("{}={}", step.name, summary));
                }
                StepStatus::InProgress => {
                    done.push(format!("{}(\u{25b6})", step.name));
                }
                StepStatus::Pending => {
                    if i > self.current_step {
                        remaining.push(step.name.as_str());
                    }
                }
            }
        }

        let done_str = done.join(", ");
        let remaining_str = remaining.join(", ");

        let mut ctx = format!(
            "[Workflow: {} | Step {}/{}: {} | Done: {} | Remaining: {}]",
            workflow_name, step_num, total, current_name, done_str, remaining_str
        );

        // Include completed results summary for post-compaction orientation
        if !results.is_empty() {
            ctx.push_str(&format!("\n[Completed results: {}]", results.join(", ")));
        }

        if self.plan_request_count > 0 {
            ctx.push_str(&format!(
                "\n[IMPORTANT: Do NOT re-call self_test_suite. You already have the plan. \
                 NEXT: Execute '{}' category tests directly using the tools listed in the plan.]",
                current_name
            ));
        }

        if self.is_complete() {
            ctx.push_str(
                "\n[ALL WORKFLOW STEPS COMPLETE. Do NOT re-run any tests. Do NOT call self_test_suite. \
                 Output the final === MAE Self-Test Report === using the results above.]"
            );
        }

        ctx
    }
}

/// Classify a tool name to a self-test workflow step (category name).
/// Returns None if the tool doesn't map to a known self-test category.
pub(crate) fn classify_tool_to_self_test_step(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "introspect"
        | "cursor_info"
        | "editor_state"
        | "list_buffers"
        | "window_layout"
        | "command_list"
        | "ai_permissions"
        | "audit_configuration" => Some("introspection"),

        "create_file" | "buffer_write" | "buffer_read" | "open_file" | "close_buffer"
        | "switch_buffer" | "rename_file" | "file_read" => Some("editing"),

        "kb_search" | "kb_list" | "kb_get" | "kb_links_from" | "kb_links_to" | "kb_graph"
        | "help_open" => Some("help"),

        "project_search" | "project_files" | "project_info" => Some("project"),

        "lsp_definition"
        | "lsp_references"
        | "lsp_hover"
        | "lsp_workspace_symbol"
        | "lsp_document_symbols"
        | "lsp_diagnostics" => Some("lsp"),

        "perf_stats" | "perf_benchmark" => Some("performance"),

        // All actual DAP tools from dap_exec.rs
        "dap_start"
        | "dap_set_breakpoint"
        | "dap_continue"
        | "dap_step"
        | "dap_inspect_variable"
        | "dap_remove_breakpoint"
        | "dap_list_variables"
        | "dap_expand_variable"
        | "dap_select_frame"
        | "dap_select_thread"
        | "dap_output"
        | "dap_evaluate"
        | "dap_disconnect"
        | "debug_state" => Some("dap"),

        "git_status" | "git_diff" | "git_log" | "github_pr_status" => Some("git"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_tracker_default_is_inactive() {
        let tracker = WorkflowTracker::default();
        assert!(!tracker.is_active());
        assert!(!tracker.is_complete());
        assert_eq!(tracker.context_injection(), "");
    }

    #[test]
    fn workflow_tracker_start_and_context_injection() {
        let mut tracker = WorkflowTracker::default();
        tracker.start_workflow(
            "self-test".into(),
            vec!["introspection".into(), "editing".into(), "help".into()],
        );

        assert!(tracker.is_active());
        assert!(!tracker.is_complete());
        assert_eq!(tracker.current_step_name(), "introspection");

        let ctx = tracker.context_injection();
        assert!(ctx.contains("Workflow: self-test"));
        assert!(ctx.contains("Step 1/3: introspection"));
        assert!(ctx.contains("Do NOT re-call self_test_suite"));
    }

    #[test]
    fn workflow_tracker_advance_and_complete() {
        let mut tracker = WorkflowTracker::default();
        tracker.start_workflow(
            "self-test".into(),
            vec!["step1".into(), "step2".into(), "step3".into()],
        );

        tracker.advance("3 passed".into());
        assert_eq!(tracker.current_step, 1);
        assert_eq!(tracker.current_step_name(), "step2");
        assert_eq!(tracker.steps[0].status, StepStatus::Completed);
        assert_eq!(tracker.steps[1].status, StepStatus::InProgress);
        assert!(!tracker.is_complete());

        tracker.skip("no LSP server".into());
        assert_eq!(tracker.current_step, 2);
        assert_eq!(tracker.steps[1].status, StepStatus::Skipped);

        tracker.fail("timeout".into());
        assert_eq!(tracker.current_step, 3);
        assert!(tracker.is_complete());

        let ctx = tracker.context_injection();
        assert!(ctx.contains("ALL WORKFLOW STEPS COMPLETE"));
    }

    #[test]
    fn tool_to_step_classifier() {
        assert_eq!(
            classify_tool_to_self_test_step("introspect"),
            Some("introspection")
        );
        assert_eq!(
            classify_tool_to_self_test_step("buffer_write"),
            Some("editing")
        );
        assert_eq!(classify_tool_to_self_test_step("kb_search"), Some("help"));
        assert_eq!(
            classify_tool_to_self_test_step("project_search"),
            Some("project")
        );
        assert_eq!(
            classify_tool_to_self_test_step("lsp_definition"),
            Some("lsp")
        );
        assert_eq!(
            classify_tool_to_self_test_step("perf_benchmark"),
            Some("performance")
        );
        assert_eq!(
            classify_tool_to_self_test_step("dap_set_breakpoint"),
            Some("dap")
        );
        assert_eq!(classify_tool_to_self_test_step("git_status"), Some("git"));
        assert_eq!(classify_tool_to_self_test_step("shell_exec"), None);
        assert_eq!(classify_tool_to_self_test_step("log_activity"), None);
    }

    #[test]
    fn workflow_tracker_record_tool_tracks_calls() {
        let mut tracker = WorkflowTracker::default();
        tracker.start_workflow("test".into(), vec!["s1".into(), "s2".into()]);
        tracker.record_tool("introspect");
        tracker.record_tool("editor_state");
        assert_eq!(tracker.step_tools_called.len(), 2);

        tracker.advance("done".into());
        assert!(tracker.step_tools_called.is_empty());
    }

    #[test]
    fn plan_request_count_increments() {
        let mut tracker = WorkflowTracker::default();
        tracker.start_workflow("self-test".into(), vec!["introspection".into()]);
        assert_eq!(tracker.plan_request_count, 1);
        tracker.plan_request_count += 1;
        assert_eq!(tracker.plan_request_count, 2);
    }
}
