//! Editor-side DAP helpers.
//!
//! Mirrors `lsp_ops.rs`: the editor's synchronous dispatch layer cannot
//! call the async DAP client directly, so commands that drive a debug
//! session push a `DapIntent` onto `pending_dap_intents`. The binary
//! drains the queue each event-loop tick and forwards each intent to
//! `run_dap_task`.
//!
//! Response handling — updating `DebugState` from thread/stack/scope/
//! variables results, applying verified breakpoints, recording stopped
//! locations — lives here so the binary stays thin.
//!
//! Emacs lesson: Emacs's GUD (Grand Unified Debugger) coordinated
//! gdb/pdb by spawning subprocesses and parsing human-readable output
//! inside one monolithic elisp file. We keep the protocol layer in
//! `mae-dap`, the transport coordination in `run_dap_task`, and the
//! state updates here — three small, testable modules instead of one
//! inscrutable 3000-line file.

use crate::dap_intent::{DapIntent, DapSpawnConfig};
use crate::debug::{
    DebugState, DebugTarget, DebugThread, Scope, StackFrame, Variable,
};

use super::Editor;

impl Editor {
    // ------------------------------------------------------------------
    // Command builders — push intents onto `pending_dap_intents`.
    // These are invoked from dispatch arms (e.g. `:debug-start`).
    // ------------------------------------------------------------------

    /// Start a DAP session against `program` using `adapter_id` to pick
    /// a configured adapter. The binary maps `adapter_id` to the
    /// `DapSpawnConfig` (command, args) at drain time.
    ///
    /// Creates a fresh `DebugState` with target `Dap { adapter_name,
    /// program }` so the renderer/AI can immediately see "session
    /// starting" without waiting for the `SessionStarted` event.
    pub fn dap_start_session(
        &mut self,
        spawn: DapSpawnConfig,
        program: String,
        launch_args: serde_json::Value,
        attach: bool,
    ) {
        let adapter_name = spawn.adapter_id.clone();
        self.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: adapter_name.clone(),
            program,
        }));
        self.pending_dap_intents.push(DapIntent::StartSession {
            spawn,
            launch_args,
            attach,
        });
        self.set_status(format!("[DAP] starting {}...", adapter_name));
    }

    /// Toggle a breakpoint at the cursor's current line in the active
    /// buffer, then push a `SetBreakpoints` intent (if in a DAP session)
    /// with the resulting line set so the adapter replaces its view.
    ///
    /// Works even without an active session — the toggle is recorded
    /// locally, and the adapter sync happens when a session starts
    /// (via `dap_resync_breakpoints`).
    ///
    /// Source selection:
    /// - Prefer the buffer's file path when one is set.
    /// - In a DAP session without a file path, refuse (the adapter
    ///   can't resolve an unsaved buffer).
    /// - Otherwise fall back to the buffer name (self-debug workflow).
    pub fn dap_toggle_breakpoint_at_cursor(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let file_path = self.buffers[buf_idx]
            .file_path()
            .map(|p| p.to_string_lossy().into_owned());
        let is_dap = matches!(
            self.debug_state.as_ref().map(|s| &s.target),
            Some(DebugTarget::Dap { .. })
        );
        let source_path = match (file_path, is_dap) {
            (Some(p), _) => p,
            (None, true) => {
                self.set_status("[DAP] active buffer has no file path");
                return;
            }
            (None, false) => self.buffers[buf_idx].name.clone(),
        };

        // DAP lines are 1-based; editor rows are 0-based.
        let line = self.window_mgr.focused_window().cursor_row as i64 + 1;

        // Lazily create state so breakpoints can be set before a session starts.
        let state = self
            .debug_state
            .get_or_insert_with(|| DebugState::new(DebugTarget::SelfDebug));
        let remaining_lines = state.toggle_breakpoint_at(source_path.clone(), line);

        // Only forward to adapter if we're actually in a DAP session.
        if is_dap {
            self.pending_dap_intents.push(DapIntent::SetBreakpoints {
                source_path: source_path.clone(),
                lines: remaining_lines.clone(),
            });
        }

        if remaining_lines.contains(&line) {
            self.set_status(format!("Breakpoint set: {}:{}", source_path, line));
        } else {
            self.set_status(format!("Breakpoint removed: {}:{}", source_path, line));
        }
    }

    /// Push SetBreakpoints intents for every source in `debug_state`.
    /// Useful right after `SessionStarted` to hand the adapter our
    /// already-recorded breakpoint set.
    pub fn dap_resync_breakpoints(&mut self) {
        let Some(state) = self.debug_state.as_ref() else {
            return;
        };
        let entries: Vec<(String, Vec<i64>)> = state
            .breakpoints
            .iter()
            .map(|(src, bps)| (src.clone(), bps.iter().map(|b| b.line).collect()))
            .collect();
        for (source_path, lines) in entries {
            self.pending_dap_intents
                .push(DapIntent::SetBreakpoints { source_path, lines });
        }
    }

    /// Resume execution on the active thread.
    pub fn dap_continue(&mut self) {
        let tid = self.dap_active_thread_id();
        self.pending_dap_intents.push(DapIntent::Continue { thread_id: tid });
        self.set_status("[DAP] continue");
    }

    /// Step over on the active thread.
    pub fn dap_step_over(&mut self) {
        let tid = self.dap_active_thread_id();
        self.pending_dap_intents.push(DapIntent::Next { thread_id: tid });
        self.set_status("[DAP] step over");
    }

    /// Step into on the active thread.
    pub fn dap_step_into(&mut self) {
        let tid = self.dap_active_thread_id();
        self.pending_dap_intents.push(DapIntent::StepIn { thread_id: tid });
        self.set_status("[DAP] step into");
    }

    /// Step out on the active thread.
    pub fn dap_step_out(&mut self) {
        let tid = self.dap_active_thread_id();
        self.pending_dap_intents.push(DapIntent::StepOut { thread_id: tid });
        self.set_status("[DAP] step out");
    }

    /// Pull fresh threads + top-of-stack for the active thread.
    pub fn dap_refresh(&mut self) {
        let tid = self.debug_state.as_ref().map(|s| s.active_thread_id);
        self.pending_dap_intents
            .push(DapIntent::RefreshThreadsAndStack { thread_id: tid });
    }

    /// Request scopes for a stack frame.
    pub fn dap_request_scopes(&mut self, frame_id: i64) {
        self.pending_dap_intents
            .push(DapIntent::RequestScopes { frame_id });
    }

    /// Request variables for a variablesReference, tagged by scope_name.
    pub fn dap_request_variables(&mut self, scope_name: String, variables_reference: i64) {
        self.pending_dap_intents.push(DapIntent::RequestVariables {
            scope_name,
            variables_reference,
        });
    }

    /// Terminate (soft stop) the debuggee.
    pub fn dap_terminate(&mut self) {
        self.pending_dap_intents.push(DapIntent::Terminate);
        self.set_status("[DAP] terminating...");
    }

    /// Disconnect — kills the adapter process.
    pub fn dap_disconnect(&mut self, terminate_debuggee: bool) {
        self.pending_dap_intents.push(DapIntent::Disconnect {
            terminate_debuggee,
        });
        self.debug_state = None;
        self.set_status("[DAP] disconnected");
    }

    /// The thread id the UI is currently focused on, or 0 if no session.
    fn dap_active_thread_id(&self) -> i64 {
        self.debug_state.as_ref().map(|s| s.active_thread_id).unwrap_or(0)
    }

    // ------------------------------------------------------------------
    // Response handlers — called by main.rs when DapTaskEvents arrive.
    // They update DebugState so renderer + AI tools see the new picture.
    // ------------------------------------------------------------------

    /// Handle `SessionStarted` — mark state ready and re-sync any
    /// breakpoints the user recorded before the session came up.
    pub fn apply_dap_session_started(&mut self, adapter_id: String) {
        self.set_status(format!("[DAP] {} session started", adapter_id));
        self.dap_resync_breakpoints();
        // Refresh threads + stack so the UI shows something non-empty.
        self.dap_refresh();
    }

    /// Handle `SessionStartFailed` — clear state and surface the error.
    pub fn apply_dap_session_start_failed(&mut self, error: String) {
        self.debug_state = None;
        self.set_status(format!("[DAP] session start failed: {}", error));
    }

    /// Handle a `Stopped` event — record the location and trigger a
    /// thread/stack refresh so the UI repopulates.
    pub fn apply_dap_stopped(
        &mut self,
        reason: String,
        thread_id: Option<i64>,
        text: Option<String>,
    ) {
        if let Some(state) = self.debug_state.as_mut() {
            if let Some(tid) = thread_id {
                state.active_thread_id = tid;
            }
            // Mark all threads stopped (simple, matches most adapters).
            for t in state.threads.iter_mut() {
                t.stopped = true;
            }
        }
        let msg = match text {
            Some(t) if !t.is_empty() => format!("[DAP] stopped: {} ({})", reason, t),
            _ => format!("[DAP] stopped: {}", reason),
        };
        self.set_status(msg);
        self.dap_refresh();
    }

    /// Handle a `Continued` event — clear the stopped marker.
    pub fn apply_dap_continued(&mut self, thread_id: i64, all_threads: bool) {
        if let Some(state) = self.debug_state.as_mut() {
            state.clear_stopped_location();
            if all_threads {
                for t in state.threads.iter_mut() {
                    t.stopped = false;
                }
            } else {
                for t in state.threads.iter_mut() {
                    if t.id == thread_id {
                        t.stopped = false;
                    }
                }
            }
        }
        self.set_status("[DAP] running");
    }

    /// Handle an `Output` event — append to the debug output log.
    pub fn apply_dap_output(&mut self, category: String, output: String) {
        if let Some(state) = self.debug_state.as_mut() {
            state.log(format!("[{}] {}", category, output.trim_end()));
        }
    }

    /// Handle `Terminated` — the debuggee finished.
    pub fn apply_dap_terminated(&mut self) {
        if let Some(state) = self.debug_state.as_mut() {
            state.clear_stopped_location();
            for t in state.threads.iter_mut() {
                t.stopped = false;
            }
        }
        self.set_status("[DAP] program terminated");
    }

    /// Handle `AdapterExited` — drop the session entirely.
    pub fn apply_dap_adapter_exited(&mut self) {
        self.debug_state = None;
        self.set_status("[DAP] adapter exited");
    }

    /// Handle a `ThreadsResult` — replace the thread list.
    /// Threads are `(id, name)` pairs.
    pub fn apply_dap_threads(&mut self, threads: Vec<(i64, String)>) {
        let Some(state) = self.debug_state.as_mut() else {
            return;
        };
        // Preserve stopped flags for threads that already existed.
        let prior: std::collections::HashMap<i64, bool> = state
            .threads
            .iter()
            .map(|t| (t.id, t.stopped))
            .collect();
        let new_threads = threads
            .into_iter()
            .map(|(id, name)| DebugThread {
                id,
                name,
                stopped: prior.get(&id).copied().unwrap_or(true),
            })
            .collect();
        state.set_threads(new_threads);
    }

    /// Handle a `StackTraceResult` — replace the stack frames for the
    /// given thread. Also drive a scopes refresh for the top frame so
    /// the variables pane repopulates without extra user action.
    /// Frames are `(id, name, source, line, column)` tuples.
    pub fn apply_dap_stack_trace(
        &mut self,
        thread_id: i64,
        frames: Vec<(i64, String, Option<String>, i64, i64)>,
    ) {
        let Some(state) = self.debug_state.as_mut() else {
            return;
        };
        state.active_thread_id = thread_id;
        let top_frame = frames.first().cloned();
        let stack = frames
            .into_iter()
            .map(|(id, name, source, line, column)| StackFrame {
                id,
                name,
                source,
                line,
                column,
            })
            .collect();
        state.set_stack_frames(stack);

        // Update stopped_location from the top frame (if it has a source).
        if let Some((_, _, Some(src), line, _)) = top_frame.clone() {
            state.set_stopped_location(src, line);
        }

        // Queue a scopes request for the top frame.
        if let Some((frame_id, _, _, _, _)) = top_frame {
            self.dap_request_scopes(frame_id);
        }
    }

    /// Handle a `ScopesResult` — replace scope list and queue variables
    /// requests for each. Scopes are `(name, variables_reference, expensive)`.
    pub fn apply_dap_scopes(&mut self, _frame_id: i64, scopes: Vec<(String, i64, bool)>) {
        // Walk the input once: build the model list and collect the
        // non-expensive scope refs we need to fetch.
        let mut mapped = Vec::with_capacity(scopes.len());
        let mut scope_refs: Vec<(String, i64)> = Vec::new();
        for (name, variables_reference, expensive) in scopes {
            if !expensive {
                scope_refs.push((name.clone(), variables_reference));
            }
            mapped.push(Scope {
                name,
                variables_reference,
                expensive,
            });
        }

        if let Some(state) = self.debug_state.as_mut() {
            state.set_scopes(mapped);
        }

        for (name, vref) in scope_refs {
            self.dap_request_variables(name, vref);
        }
    }

    /// Handle a `VariablesResult` — replace variables for a scope.
    /// Variables are `(name, value, type, variables_reference)` tuples.
    pub fn apply_dap_variables(
        &mut self,
        scope_name: String,
        variables: Vec<(String, String, Option<String>, i64)>,
    ) {
        let Some(state) = self.debug_state.as_mut() else {
            return;
        };
        let mapped = variables
            .into_iter()
            .map(|(name, value, var_type, variables_reference)| Variable {
                name,
                value,
                var_type,
                variables_reference,
            })
            .collect();
        state.set_variables_for_scope(scope_name, mapped);
    }

    /// Handle a `BreakpointsSet` response — the adapter has verified
    /// our breakpoints. Replace the core-side list with verified status.
    /// Entries are `(id, verified, line)` tuples; `id` is the adapter-
    /// assigned id (may differ from the local id we had).
    pub fn apply_dap_breakpoints_set(&mut self, source_path: String, entries: Vec<(i64, bool, i64)>) {
        let Some(state) = self.debug_state.as_mut() else {
            return;
        };
        state.apply_verified_breakpoints(source_path, entries);
    }

    /// Handle a DAP error — surface in status line.
    pub fn apply_dap_error(&mut self, message: String) {
        self.set_status(format!("[DAP] {}", message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use std::path::PathBuf;

    fn editor_with_file(path: &str, text: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.set_file_path(PathBuf::from(path));
        if !text.is_empty() {
            buf.insert_text_at(0, text);
        }
        Editor::with_buffer(buf)
    }

    #[test]
    fn dap_start_session_queues_intent_and_sets_state() {
        let mut ed = Editor::new();
        let spawn = DapSpawnConfig {
            command: "lldb-dap".into(),
            args: vec![],
            adapter_id: "lldb".into(),
        };
        ed.dap_start_session(
            spawn.clone(),
            "/bin/ls".into(),
            serde_json::json!({"program": "/bin/ls"}),
            false,
        );
        assert_eq!(ed.pending_dap_intents.len(), 1);
        assert!(matches!(
            ed.pending_dap_intents[0],
            DapIntent::StartSession { attach: false, .. }
        ));
        let state = ed.debug_state.as_ref().unwrap();
        assert!(matches!(state.target, DebugTarget::Dap { .. }));
    }

    #[test]
    fn dap_toggle_breakpoint_requires_file_path_in_dap_session() {
        let mut ed = Editor::new();
        // Start a DAP session first so the "no file path" check kicks in.
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.dap_toggle_breakpoint_at_cursor();
        assert!(ed.pending_dap_intents.is_empty());
        assert!(ed.status_msg.contains("no file path"));
    }

    #[test]
    fn dap_toggle_breakpoint_falls_back_to_buffer_name_in_self_debug() {
        let mut ed = Editor::new();
        // No file path, no DAP session → self-debug falls back to buffer name
        ed.dap_toggle_breakpoint_at_cursor();
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.breakpoint_count(), 1);
    }

    #[test]
    fn dap_toggle_breakpoint_records_locally_without_session() {
        let mut ed = editor_with_file("/tmp/a.rs", "line1\nline2\nline3\n");
        // Move cursor to line 2 (row=1, line=2 in DAP 1-based)
        ed.window_mgr.focused_window_mut().cursor_row = 1;
        ed.dap_toggle_breakpoint_at_cursor();
        // No DAP session → no intent sent to adapter
        assert!(ed.pending_dap_intents.is_empty());
        // But state has the breakpoint
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.breakpoint_count(), 1);
        let bps = state.breakpoints.get("/tmp/a.rs").unwrap();
        assert_eq!(bps[0].line, 2);
    }

    #[test]
    fn dap_toggle_breakpoint_forwards_to_adapter_during_session() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\ny\nz\n");
        ed.dap_start_session(
            DapSpawnConfig {
                command: "lldb-dap".into(),
                args: vec![],
                adapter_id: "lldb".into(),
            },
            "/tmp/a.rs".into(),
            serde_json::json!({}),
            false,
        );
        // Clear the StartSession intent for clarity
        ed.pending_dap_intents.clear();
        ed.dap_toggle_breakpoint_at_cursor();
        assert_eq!(ed.pending_dap_intents.len(), 1);
        match &ed.pending_dap_intents[0] {
            DapIntent::SetBreakpoints { source_path, lines } => {
                assert_eq!(source_path, "/tmp/a.rs");
                assert_eq!(lines, &vec![1]);
            }
            other => panic!("expected SetBreakpoints, got {:?}", other),
        }
    }

    #[test]
    fn dap_toggle_twice_removes_breakpoint() {
        let mut ed = editor_with_file("/tmp/a.rs", "x\ny\n");
        ed.dap_toggle_breakpoint_at_cursor();
        ed.dap_toggle_breakpoint_at_cursor();
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.breakpoint_count(), 0);
    }

    #[test]
    fn dap_continue_step_queue_intents() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.debug_state.as_mut().unwrap().active_thread_id = 7;
        ed.dap_continue();
        ed.dap_step_over();
        ed.dap_step_into();
        ed.dap_step_out();
        assert_eq!(ed.pending_dap_intents.len(), 4);
        assert!(matches!(ed.pending_dap_intents[0], DapIntent::Continue { thread_id: 7 }));
        assert!(matches!(ed.pending_dap_intents[1], DapIntent::Next { thread_id: 7 }));
        assert!(matches!(ed.pending_dap_intents[2], DapIntent::StepIn { thread_id: 7 }));
        assert!(matches!(ed.pending_dap_intents[3], DapIntent::StepOut { thread_id: 7 }));
    }

    #[test]
    fn dap_resync_pushes_one_intent_per_source() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.add_breakpoint("/a.rs", 1);
        state.add_breakpoint("/a.rs", 5);
        state.add_breakpoint("/b.rs", 10);
        ed.debug_state = Some(state);
        ed.dap_resync_breakpoints();
        assert_eq!(ed.pending_dap_intents.len(), 2);
    }

    #[test]
    fn apply_stopped_marks_threads_and_refreshes() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.threads.push(DebugThread {
            id: 1,
            name: "main".into(),
            stopped: false,
        });
        ed.debug_state = Some(state);
        ed.apply_dap_stopped("breakpoint".into(), Some(1), None);
        let state = ed.debug_state.as_ref().unwrap();
        assert!(state.threads[0].stopped);
        assert_eq!(state.active_thread_id, 1);
        // A refresh intent should have been queued.
        assert!(matches!(
            ed.pending_dap_intents.last(),
            Some(DapIntent::RefreshThreadsAndStack { .. })
        ));
    }

    #[test]
    fn apply_continued_clears_stopped() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.threads.push(DebugThread {
            id: 1,
            name: "main".into(),
            stopped: true,
        });
        state.set_stopped_location("a.rs", 10);
        ed.debug_state = Some(state);
        ed.apply_dap_continued(1, true);
        let state = ed.debug_state.as_ref().unwrap();
        assert!(!state.is_stopped());
        assert!(!state.threads[0].stopped);
    }

    #[test]
    fn apply_threads_preserves_stopped_flags() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.threads.push(DebugThread {
            id: 1,
            name: "old".into(),
            stopped: false,
        });
        ed.debug_state = Some(state);
        ed.apply_dap_threads(vec![(1, "main".into()), (2, "worker".into())]);
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.threads.len(), 2);
        assert!(!state.threads[0].stopped); // preserved from prior
        assert!(state.threads[1].stopped); // new defaults to stopped
    }

    #[test]
    fn apply_stack_trace_sets_stopped_location_and_queues_scopes() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.apply_dap_stack_trace(
            1,
            vec![
                (100, "main".into(), Some("main.rs".into()), 42, 0),
                (101, "caller".into(), Some("lib.rs".into()), 10, 0),
            ],
        );
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.stack_frames.len(), 2);
        assert_eq!(state.stopped_location, Some(("main.rs".into(), 42)));
        // Scopes request should be queued for top frame (id=100).
        assert!(ed
            .pending_dap_intents
            .iter()
            .any(|i| matches!(i, DapIntent::RequestScopes { frame_id: 100 })));
    }

    #[test]
    fn apply_scopes_queues_variables_requests_skipping_expensive() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.apply_dap_scopes(
            1,
            vec![
                ("Locals".into(), 10, false),
                ("Globals".into(), 11, true), // expensive — skip
                ("Registers".into(), 12, false),
            ],
        );
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.scopes.len(), 3);
        // Two non-expensive scopes → two variable requests.
        let req_count = ed
            .pending_dap_intents
            .iter()
            .filter(|i| matches!(i, DapIntent::RequestVariables { .. }))
            .count();
        assert_eq!(req_count, 2);
    }

    #[test]
    fn apply_variables_stores_by_scope() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.apply_dap_variables(
            "Locals".into(),
            vec![
                ("x".into(), "42".into(), Some("i32".into()), 0),
                ("s".into(), "\"hi\"".into(), Some("String".into()), 0),
            ],
        );
        let state = ed.debug_state.as_ref().unwrap();
        let vars = state.variables.get("Locals").unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].name, "x");
        assert_eq!(vars[1].value, "\"hi\"");
    }

    #[test]
    fn apply_breakpoints_set_replaces_source_entries() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.add_breakpoint("/a.rs", 1);
        ed.debug_state = Some(state);
        ed.apply_dap_breakpoints_set(
            "/a.rs".into(),
            vec![(99, true, 1), (100, false, 5)],
        );
        let state = ed.debug_state.as_ref().unwrap();
        let bps = state.breakpoints.get("/a.rs").unwrap();
        assert_eq!(bps.len(), 2);
        assert_eq!(bps[0].id, 99);
        assert!(bps[0].verified);
        assert_eq!(bps[1].line, 5);
        assert!(!bps[1].verified);
    }

    #[test]
    fn apply_adapter_exited_drops_session() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.apply_dap_adapter_exited();
        assert!(ed.debug_state.is_none());
    }

    #[test]
    fn apply_output_appends_to_log() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.apply_dap_output("stdout".into(), "hello\n".into());
        ed.apply_dap_output("stderr".into(), "warn\n".into());
        let state = ed.debug_state.as_ref().unwrap();
        assert_eq!(state.output_log.len(), 2);
        assert!(state.output_log[0].contains("[stdout]"));
        assert!(state.output_log[0].contains("hello"));
    }

    #[test]
    fn apply_session_started_triggers_resync() {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        });
        state.add_breakpoint("/a.rs", 10);
        state.add_breakpoint("/b.rs", 20);
        ed.debug_state = Some(state);
        ed.apply_dap_session_started("lldb".into());
        // Two SetBreakpoints (one per source) + one RefreshThreadsAndStack.
        let bp_count = ed
            .pending_dap_intents
            .iter()
            .filter(|i| matches!(i, DapIntent::SetBreakpoints { .. }))
            .count();
        assert_eq!(bp_count, 2);
        assert!(ed
            .pending_dap_intents
            .iter()
            .any(|i| matches!(i, DapIntent::RefreshThreadsAndStack { .. })));
    }

    #[test]
    fn dap_disconnect_clears_debug_state() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "x".into(),
        }));
        ed.dap_disconnect(false);
        assert!(ed.debug_state.is_none());
        assert!(matches!(
            ed.pending_dap_intents[0],
            DapIntent::Disconnect { terminate_debuggee: false }
        ));
    }
}
