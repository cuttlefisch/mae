//! Progress checkpoint system — replaces blunt round counting with
//! semantic progress evaluation.
//!
//! Every N rounds the tracker snapshots tool usage metrics and scores
//! them 0-6. Consecutive low-score windows trigger a stagnation abort.
//! This catches runaway loops without killing complex legitimate tasks.

use std::collections::HashSet;

/// Metrics accumulated during one checkpoint window.
#[derive(Debug, Clone, Default)]
pub(crate) struct CheckpointSnapshot {
    pub unique_tools: HashSet<String>,
    pub successful_calls: usize,
    pub failed_calls: usize,
    pub mutating_calls: usize,
    pub read_calls: usize,
    pub dap_calls: usize,
    pub files_touched: HashSet<String>,
    pub rounds: usize,
    pub had_user_interaction: bool,
    pub had_reasoning_log: bool,
}

/// Verdict from a checkpoint evaluation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CheckpointVerdict {
    Continue,
    Warn { message: String },
    Abort { message: String },
}

/// Evaluates progress across checkpoint windows.
#[derive(Debug)]
pub(crate) struct ProgressTracker {
    pub checkpoint_interval: usize,
    stagnant_count: usize,
    max_stagnant: usize,
    current: CheckpointSnapshot,
    previous: Option<CheckpointSnapshot>,
    checkpoint_count: usize,
}

const MUTATING_TOOLS: &[&str] = &[
    "buffer_write",
    "create_file",
    "shell_exec",
    "rename_file",
    "buffer_delete",
];

const READ_TOOLS: &[&str] = &[
    "buffer_read",
    "project_search",
    "lsp_definition",
    "lsp_references",
    "lsp_hover",
    "lsp_workspace_symbol",
    "lsp_document_symbols",
    "debug_state",
    "open_file",
    "read_transcript",
    "introspect",
];

const DAP_TOOLS: &[&str] = &[
    "dap_start",
    "dap_set_breakpoint",
    "dap_remove_breakpoint",
    "dap_continue",
    "dap_step",
    "dap_list_variables",
    "dap_inspect_variable",
    "dap_expand_variable",
    "dap_select_frame",
    "dap_select_thread",
    "dap_output",
    "dap_evaluate",
    "dap_disconnect",
    "debug_state",
];

/// Keys in tool arguments that identify file paths.
const FILE_ARG_KEYS: &[&str] = &["path", "file_path", "buffer", "file"];

impl ProgressTracker {
    pub fn new(checkpoint_interval: usize, self_test: bool) -> Self {
        ProgressTracker {
            checkpoint_interval,
            stagnant_count: 0,
            max_stagnant: if self_test { 4 } else { 2 },
            current: CheckpointSnapshot::default(),
            previous: None,
            checkpoint_count: 0,
        }
    }

    /// Record a tool call. Called after each tool result is processed.
    pub fn record_tool_call(&mut self, name: &str, arguments: &serde_json::Value, success: bool) {
        self.current.unique_tools.insert(name.to_string());

        if success {
            self.current.successful_calls += 1;
        } else {
            self.current.failed_calls += 1;
        }

        if MUTATING_TOOLS.contains(&name) {
            self.current.mutating_calls += 1;
        }
        if READ_TOOLS.contains(&name) {
            self.current.read_calls += 1;
        }
        if DAP_TOOLS.contains(&name) {
            self.current.dap_calls += 1;
        }

        if name == "ask_user" || name == "propose_changes" {
            self.current.had_user_interaction = true;
        }
        if name == "log_activity" {
            self.current.had_reasoning_log = true;
        }

        // Extract file paths from arguments
        if let Some(obj) = arguments.as_object() {
            for key in FILE_ARG_KEYS {
                if let Some(val) = obj.get(*key).and_then(|v| v.as_str()) {
                    self.current.files_touched.insert(val.to_string());
                }
            }
        }
    }

    /// Record that a round completed.
    pub fn record_round(&mut self) {
        self.current.rounds += 1;
    }

    /// Evaluate progress at a checkpoint boundary. Returns the verdict.
    pub fn evaluate(&mut self) -> CheckpointVerdict {
        let score = self.score();
        self.checkpoint_count += 1;

        let verdict = if score >= 3 {
            self.stagnant_count = 0;
            CheckpointVerdict::Continue
        } else if score >= 1 {
            // Marginal — decrement stagnant if accumulated
            self.stagnant_count = self.stagnant_count.saturating_sub(1);
            CheckpointVerdict::Warn {
                message: format!(
                    "Progress checkpoint: marginal progress (score {}/6, stagnant={})",
                    score, self.stagnant_count
                ),
            }
        } else {
            self.stagnant_count += 1;
            if self.stagnant_count >= self.max_stagnant {
                return CheckpointVerdict::Abort {
                    message: format!(
                        "AI stagnation detected: {} consecutive checkpoints with no progress — aborting",
                        self.stagnant_count
                    ),
                };
            }
            CheckpointVerdict::Warn {
                message: format!(
                    "Progress checkpoint: no progress detected (score 0/6, stagnant {}/{})",
                    self.stagnant_count, self.max_stagnant
                ),
            }
        };

        // Rotate: current becomes previous, reset current
        self.previous = Some(std::mem::take(&mut self.current));

        verdict
    }

    /// Score the current window (0-6 points).
    fn score(&self) -> usize {
        let mut score = 0;

        // First checkpoint gets benefit of the doubt (+2, replaces file/tool novelty)
        if self.checkpoint_count == 0 {
            score += 2;
        } else {
            // +1 for new files touched (vs previous window)
            if let Some(ref prev) = self.previous {
                let new_files = self
                    .current
                    .files_touched
                    .iter()
                    .any(|f| !prev.files_touched.contains(f));
                if new_files {
                    score += 1;
                }

                // +1 for new tool types used (vs previous window)
                let new_tools = self
                    .current
                    .unique_tools
                    .iter()
                    .any(|t| !prev.unique_tools.contains(t));
                if new_tools {
                    score += 1;
                }
            }
        }

        // +1 for 3+ different tools
        if self.current.unique_tools.len() >= 3 {
            score += 1;
        }

        // +1 for any mutating tool
        if self.current.mutating_calls > 0 {
            score += 1;
        }

        // +1 for active debug session work (≥2 DAP tool calls this window)
        if self.current.dap_calls >= 2 {
            score += 1;
        }

        // +1 for user interaction
        if self.current.had_user_interaction {
            score += 1;
        }

        // +1 for >50% success rate (only if there were calls)
        let total = self.current.successful_calls + self.current.failed_calls;
        if total > 0 && self.current.successful_calls * 2 > total {
            score += 1;
        }

        score
    }

    /// Increment stagnant count (used by oscillation detector).
    pub fn increment_stagnant(&mut self) {
        self.stagnant_count += 1;
    }

    /// Check if stagnant count has reached the abort threshold.
    pub fn should_abort_stagnant(&self) -> bool {
        self.stagnant_count >= self.max_stagnant
    }

    pub fn stagnant_count(&self) -> usize {
        self.stagnant_count
    }

    pub fn max_stagnant(&self) -> usize {
        self.max_stagnant
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_first_checkpoint_benefit_of_doubt() {
        let mut tracker = ProgressTracker::new(10, false);
        // Minimal activity: just one read tool
        tracker.record_tool_call("buffer_read", &json!({"path": "foo.rs"}), true);
        tracker.record_round();

        // First checkpoint should get +2 for benefit of doubt, +1 for success
        let verdict = tracker.evaluate();
        assert_eq!(verdict, CheckpointVerdict::Continue);
    }

    #[test]
    fn test_stagnant_abort_after_max() {
        let mut tracker = ProgressTracker::new(5, false);

        // First checkpoint — benefit of doubt
        tracker.record_tool_call("buffer_read", &json!({}), false);
        tracker.record_round();
        let v1 = tracker.evaluate();
        // First checkpoint: +2 benefit, 0 success (failed) = 2 → Warn
        assert!(matches!(v1, CheckpointVerdict::Warn { .. }));

        // Second checkpoint — same failed tool, no progress
        tracker.record_tool_call("buffer_read", &json!({}), false);
        tracker.record_round();
        let v2 = tracker.evaluate();
        // Score 0: same tool, same files, all failed → stagnant_count=1
        assert!(matches!(v2, CheckpointVerdict::Warn { .. }));

        // Third checkpoint — still nothing → stagnant_count=2 → abort
        tracker.record_tool_call("buffer_read", &json!({}), false);
        tracker.record_round();
        let v3 = tracker.evaluate();
        assert!(matches!(v3, CheckpointVerdict::Abort { .. }));
    }

    #[test]
    fn test_mutation_resets_stagnant() {
        let mut tracker = ProgressTracker::new(5, false);

        // First checkpoint: minimal, uses benefit of doubt
        tracker.record_tool_call("buffer_read", &json!({}), true);
        tracker.record_round();
        tracker.evaluate(); // Continue (first checkpoint)

        // Second: stagnant (same tool, same file, no new work)
        tracker.record_tool_call("buffer_read", &json!({}), true);
        tracker.record_round();
        let v = tracker.evaluate();
        // Score: 0 new files, 0 new tools, no mutation, success = 1 → Warn
        assert!(matches!(v, CheckpointVerdict::Warn { .. }));

        // Third: now do a write + read different tools + new file → should reset
        tracker.record_tool_call("buffer_write", &json!({"path": "new.rs"}), true);
        tracker.record_tool_call("project_search", &json!({}), true);
        tracker.record_tool_call("buffer_read", &json!({"path": "other.rs"}), true);
        tracker.record_round();
        let v = tracker.evaluate();
        assert_eq!(v, CheckpointVerdict::Continue);
        assert_eq!(tracker.stagnant_count(), 0);
    }

    #[test]
    fn test_new_files_count_as_progress() {
        let mut tracker = ProgressTracker::new(5, false);

        // First checkpoint
        tracker.record_tool_call("buffer_read", &json!({"path": "a.rs"}), true);
        tracker.record_tool_call("buffer_write", &json!({"path": "a.rs"}), true);
        tracker.record_tool_call("project_search", &json!({}), true);
        tracker.record_round();
        tracker.evaluate(); // Continue

        // Second: different files → new_files=true
        tracker.record_tool_call("buffer_read", &json!({"path": "b.rs"}), true);
        tracker.record_tool_call("buffer_write", &json!({"path": "b.rs"}), true);
        tracker.record_tool_call("project_search", &json!({}), true);
        tracker.record_round();
        let v = tracker.evaluate();
        assert_eq!(v, CheckpointVerdict::Continue);
    }

    #[test]
    fn test_user_interaction_always_progress() {
        let mut tracker = ProgressTracker::new(5, false);

        // First checkpoint to consume benefit of doubt
        tracker.record_tool_call("buffer_read", &json!({}), true);
        tracker.record_round();
        tracker.evaluate();

        // Second: only ask_user → +1 user interaction, +1 success = 2 → Warn but not stagnant
        tracker.record_tool_call("ask_user", &json!({}), true);
        tracker.record_round();
        let v = tracker.evaluate();
        // Score: no new files (1 if new tool), user interaction (+1), success (+1) = could be 2-3
        // ask_user is a new tool vs buffer_read → +1 new tool
        // = new_tool(1) + user_interaction(1) + success(1) = 3 → Continue
        assert_eq!(v, CheckpointVerdict::Continue);
    }

    #[test]
    fn test_all_failures_low_score() {
        let mut tracker = ProgressTracker::new(5, false);

        // First checkpoint to consume benefit of doubt
        tracker.record_tool_call("buffer_read", &json!({}), true);
        tracker.record_round();
        tracker.evaluate();

        // All failures, same tool
        for _ in 0..5 {
            tracker.record_tool_call("buffer_read", &json!({}), false);
        }
        tracker.record_round();
        let v = tracker.evaluate();
        // Score: same tool, same files, no mutation, no interaction, all failed = 0
        assert!(matches!(v, CheckpointVerdict::Warn { .. }));
    }

    #[test]
    fn test_self_test_higher_tolerance() {
        let mut tracker = ProgressTracker::new(15, true);
        assert_eq!(tracker.checkpoint_interval, 15);
        assert_eq!(tracker.max_stagnant(), 4);

        // Need 4 stagnant checkpoints to abort (not 2)
        // First: benefit of doubt with failure → score 2 → Warn
        tracker.record_tool_call("buffer_read", &json!({}), false);
        tracker.record_round();
        tracker.evaluate();

        // 4 more stagnant windows needed
        for i in 0..4 {
            tracker.record_tool_call("buffer_read", &json!({}), false);
            tracker.record_round();
            let v = tracker.evaluate();
            if i < 3 {
                assert!(matches!(v, CheckpointVerdict::Warn { .. }), "round {}", i);
            } else {
                assert!(matches!(v, CheckpointVerdict::Abort { .. }), "round {}", i);
            }
        }
    }
}
