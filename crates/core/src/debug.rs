use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Shared debug types
//
// These types are used by both the editor self-debugger (Rust + Scheme state)
// and DAP debug sessions. The renderer, AI tools, commands, and Scheme
// procedures all operate on `DebugState` regardless of the debug target.
//
// Emacs lesson: Emacs's GUD (Grand Unified Debugger) attempted unification
// across gdb/pdb/etc but only at the UI command level, not data model.
// We unify at the data model so the AI agent gets structured access.
// ---------------------------------------------------------------------------

/// A thread in the debugged context.
///
/// For self-debugging: "Rust Core", "Scheme Runtime", "AI Agent".
/// For DAP: actual OS/VM threads from the debug adapter.
#[derive(Debug, Clone)]
pub struct DebugThread {
    pub id: i64,
    pub name: String,
    pub stopped: bool,
}

/// A frame in a call stack.
///
/// For self-debugging: synthetic frames from event loop / Scheme call stack.
/// For DAP: real stack frames from the debug adapter.
#[derive(Debug, Clone)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    pub source: Option<String>,
    pub line: i64,
    pub column: i64,
}

/// A scope within a stack frame (e.g. "Locals", "Editor State", "Scheme Globals").
#[derive(Debug, Clone)]
pub struct Scope {
    pub name: String,
    pub variables_reference: i64,
    pub expensive: bool,
}

/// A variable within a scope. Nestable via `variables_reference`.
#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub var_type: Option<String>,
    /// Non-zero if this variable has child variables (expandable in UI).
    pub variables_reference: i64,
}

/// A breakpoint set in a source file.
#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: i64,
    pub verified: bool,
    pub source: String,
    pub line: i64,
}

/// A captured Scheme evaluation error with stack trace.
#[derive(Debug, Clone)]
pub struct SchemeErrorEntry {
    pub expression: String,
    pub error_message: String,
    pub stack_trace: Vec<String>,
    /// Monotonic sequence number for ordering.
    pub seq: u64,
}

/// What kind of debug target is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugTarget {
    /// Debugging MAE's own Rust + Scheme state.
    SelfDebug,
    /// Debugging an external program via DAP.
    Dap {
        adapter_name: String,
        program: String,
    },
}

/// Complete debug state snapshot — the unified data model.
///
/// Both the self-debugger and DAP client populate this same struct.
/// The renderer and AI tools read from it without caring about the source.
#[derive(Debug, Clone)]
pub struct DebugState {
    pub target: DebugTarget,
    pub threads: Vec<DebugThread>,
    pub active_thread_id: i64,
    pub stack_frames: Vec<StackFrame>,
    pub scopes: Vec<Scope>,
    /// Variables keyed by scope name for display grouping.
    pub variables: HashMap<String, Vec<Variable>>,
    /// Breakpoints keyed by source file path.
    pub breakpoints: HashMap<String, Vec<Breakpoint>>,
    /// Current stopped location (source, line), if stopped.
    pub stopped_location: Option<(String, i64)>,
    /// Debug output / console log lines.
    pub output_log: Vec<String>,
    /// Scheme eval errors (self-debug only, but included for uniformity).
    pub scheme_errors: Vec<SchemeErrorEntry>,
}

impl DebugState {
    /// Create a new empty debug state for a given target.
    pub fn new(target: DebugTarget) -> Self {
        DebugState {
            target,
            threads: Vec::new(),
            active_thread_id: 0,
            stack_frames: Vec::new(),
            scopes: Vec::new(),
            variables: HashMap::new(),
            breakpoints: HashMap::new(),
            stopped_location: None,
            output_log: Vec::new(),
            scheme_errors: Vec::new(),
        }
    }

    /// Create a self-debug state pre-populated with the standard threads.
    pub fn new_self_debug() -> Self {
        let mut state = Self::new(DebugTarget::SelfDebug);
        state.threads = vec![
            DebugThread {
                id: 1,
                name: "Rust Core".into(),
                stopped: true,
            },
            DebugThread {
                id: 2,
                name: "Scheme Runtime".into(),
                stopped: true,
            },
        ];
        state.active_thread_id = 1;
        state
    }

    /// Add the AI Agent thread (when an AI session is active).
    pub fn add_ai_thread(&mut self) {
        if !self.threads.iter().any(|t| t.id == 3) {
            self.threads.push(DebugThread {
                id: 3,
                name: "AI Agent".into(),
                stopped: true,
            });
        }
    }

    /// Remove the AI Agent thread.
    pub fn remove_ai_thread(&mut self) {
        self.threads.retain(|t| t.id != 3);
        if self.active_thread_id == 3 {
            self.active_thread_id = 1;
        }
    }

    /// Whether the debug session is in a stopped state.
    pub fn is_stopped(&self) -> bool {
        self.stopped_location.is_some()
    }

    /// Get the active thread.
    pub fn active_thread(&self) -> Option<&DebugThread> {
        self.threads.iter().find(|t| t.id == self.active_thread_id)
    }

    /// Set the active thread by id. Returns false if the id doesn't exist.
    pub fn set_active_thread(&mut self, id: i64) -> bool {
        if self.threads.iter().any(|t| t.id == id) {
            self.active_thread_id = id;
            true
        } else {
            false
        }
    }

    /// Add a breakpoint. Returns the assigned id.
    pub fn add_breakpoint(&mut self, source: &str, line: i64) -> i64 {
        let id = self
            .breakpoints
            .values()
            .flat_map(|bps| bps.iter().map(|b| b.id))
            .max()
            .unwrap_or(0)
            + 1;
        let bp = Breakpoint {
            id,
            verified: true,
            source: source.to_string(),
            line,
        };
        self.breakpoints
            .entry(source.to_string())
            .or_default()
            .push(bp);
        id
    }

    /// Remove a breakpoint by id. Returns true if found and removed.
    pub fn remove_breakpoint(&mut self, id: i64) -> bool {
        for bps in self.breakpoints.values_mut() {
            if let Some(pos) = bps.iter().position(|b| b.id == id) {
                bps.remove(pos);
                return true;
            }
        }
        false
    }

    /// Append a line to the debug output log.
    pub fn log(&mut self, line: impl Into<String>) {
        self.output_log.push(line.into());
    }

    /// Record a Scheme error.
    pub fn record_scheme_error(&mut self, entry: SchemeErrorEntry) {
        self.scheme_errors.push(entry);
    }

    /// Total number of breakpoints across all sources.
    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_debug_state_is_empty() {
        let state = DebugState::new(DebugTarget::SelfDebug);
        assert!(state.threads.is_empty());
        assert!(state.stack_frames.is_empty());
        assert!(state.variables.is_empty());
        assert!(state.breakpoints.is_empty());
        assert!(!state.is_stopped());
        assert_eq!(state.breakpoint_count(), 0);
    }

    #[test]
    fn self_debug_has_standard_threads() {
        let state = DebugState::new_self_debug();
        assert_eq!(state.threads.len(), 2);
        assert_eq!(state.threads[0].name, "Rust Core");
        assert_eq!(state.threads[1].name, "Scheme Runtime");
        assert_eq!(state.active_thread_id, 1);
        assert_eq!(state.active_thread().unwrap().name, "Rust Core");
    }

    #[test]
    fn ai_thread_lifecycle() {
        let mut state = DebugState::new_self_debug();
        assert_eq!(state.threads.len(), 2);

        state.add_ai_thread();
        assert_eq!(state.threads.len(), 3);
        assert_eq!(state.threads[2].name, "AI Agent");

        // Adding again is idempotent.
        state.add_ai_thread();
        assert_eq!(state.threads.len(), 3);

        state.remove_ai_thread();
        assert_eq!(state.threads.len(), 2);
        assert!(!state.threads.iter().any(|t| t.id == 3));
    }

    #[test]
    fn set_active_thread() {
        let mut state = DebugState::new_self_debug();
        assert!(state.set_active_thread(2));
        assert_eq!(state.active_thread().unwrap().name, "Scheme Runtime");

        // Non-existent thread returns false, doesn't change active.
        assert!(!state.set_active_thread(99));
        assert_eq!(state.active_thread_id, 2);
    }

    #[test]
    fn breakpoint_add_remove() {
        let mut state = DebugState::new_self_debug();

        let id1 = state.add_breakpoint("main.rs", 42);
        let id2 = state.add_breakpoint("main.rs", 100);
        let id3 = state.add_breakpoint("lib.rs", 10);

        assert_eq!(state.breakpoint_count(), 3);
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);

        assert!(state.remove_breakpoint(id2));
        assert_eq!(state.breakpoint_count(), 2);

        // Removing again returns false.
        assert!(!state.remove_breakpoint(id2));
    }

    #[test]
    fn stopped_location() {
        let mut state = DebugState::new_self_debug();
        assert!(!state.is_stopped());

        state.stopped_location = Some(("editor.rs".into(), 55));
        assert!(state.is_stopped());
    }

    #[test]
    fn scheme_error_recording() {
        let mut state = DebugState::new_self_debug();
        state.record_scheme_error(SchemeErrorEntry {
            expression: "(/ 1 0)".into(),
            error_message: "division by zero".into(),
            stack_trace: vec!["(/ 1 0)".into(), "(eval ...)".into()],
            seq: 1,
        });
        assert_eq!(state.scheme_errors.len(), 1);
        assert_eq!(state.scheme_errors[0].error_message, "division by zero");
    }

    #[test]
    fn output_log() {
        let mut state = DebugState::new_self_debug();
        state.log("Program started");
        state.log("Hit breakpoint at main.rs:42");
        assert_eq!(state.output_log.len(), 2);
        assert_eq!(state.output_log[0], "Program started");
    }

    #[test]
    fn dap_target_construction() {
        let state = DebugState::new(DebugTarget::Dap {
            adapter_name: "codelldb".into(),
            program: "./target/debug/myapp".into(),
        });
        assert!(matches!(state.target, DebugTarget::Dap { .. }));
        if let DebugTarget::Dap {
            adapter_name,
            program,
        } = &state.target
        {
            assert_eq!(adapter_name, "codelldb");
            assert_eq!(program, "./target/debug/myapp");
        }
    }

    #[test]
    fn variable_nesting() {
        let parent = Variable {
            name: "editor".into(),
            value: "Editor { ... }".into(),
            var_type: Some("Editor".into()),
            variables_reference: 100, // has children
        };
        let child = Variable {
            name: "mode".into(),
            value: "Normal".into(),
            var_type: Some("Mode".into()),
            variables_reference: 0, // leaf
        };
        assert!(parent.variables_reference > 0);
        assert_eq!(child.variables_reference, 0);
    }
}
