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

use crate::dap_intent::{DapIntent, DapSpawnConfig, StepKind};
use crate::debug::{DebugState, DebugTarget, DebugThread, Scope, StackFrame, Variable};

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
        // Format the status before moving `spawn.adapter_id` into the state,
        // so we can do a single clone instead of two.
        self.set_status(format!("[DAP] starting {}...", spawn.adapter_id));
        self.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: spawn.adapter_id.clone(),
            program,
        }));
        self.pending_dap_intents.push(DapIntent::StartSession {
            spawn,
            launch_args,
            attach,
        });
    }

    /// Start a DAP session using a named adapter preset (e.g. `"lldb"`,
    /// `"debugpy"`, `"codelldb"`). Looks up the `DapSpawnConfig` and
    /// launch args for the preset, then delegates to `dap_start_session`.
    ///
    /// Returns `Err(msg)` if the adapter name is unknown or if a debug
    /// session is already active. Both `:debug-start` and the AI
    /// `dap_start` tool go through this so the preconditions are checked
    /// in one place and both surfaces refuse to stack sessions.
    pub fn dap_start_with_adapter(
        &mut self,
        adapter: &str,
        program: &str,
        extra_args: &[String],
    ) -> Result<(), String> {
        if self.debug_state.is_some() {
            return Err("A debug session is already active".into());
        }
        let spawn = default_spawn_for_adapter(adapter).ok_or_else(|| {
            format!(
                "Unknown adapter: {} (known: lldb, debugpy, codelldb)",
                adapter
            )
        })?;
        let launch_args = default_launch_args(adapter, program, extra_args);
        self.dap_start_session(spawn, program.to_string(), launch_args, false);
        Ok(())
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
        let was_set = remaining_lines.contains(&line);

        // Status uses borrowed `source_path` before moving into the intent.
        let status = if was_set {
            format!("Breakpoint set: {}:{}", source_path, line)
        } else {
            format!("Breakpoint removed: {}:{}", source_path, line)
        };

        if is_dap {
            self.pending_dap_intents.push(DapIntent::SetBreakpoints {
                source_path,
                lines: remaining_lines,
            });
        }
        self.set_status(status);
    }

    /// Idempotently set a breakpoint at `(source_path, line)`. Returns
    /// the full line set for that source after the operation.
    ///
    /// - No-op if a breakpoint already exists at that line (does not
    ///   duplicate, does not re-notify the adapter).
    /// - Works without an active session (lazy state creation).
    /// - In a DAP session, pushes a `SetBreakpoints` intent with the
    ///   new full line set.
    ///
    /// This is the programmatic entry point used by the AI tool and by
    /// Scheme callers that need deterministic "ensure breakpoint is set"
    /// semantics, as opposed to the cursor-driven toggle.
    pub fn dap_set_breakpoint(&mut self, source_path: String, line: i64) -> Vec<i64> {
        // Lazy-create state so breakpoints can be recorded before a session.
        self.debug_state
            .get_or_insert_with(|| DebugState::new(DebugTarget::SelfDebug));
        self.mutate_breakpoint(source_path, line, /* ensure_present = */ true)
    }

    /// Idempotently remove the breakpoint at `(source_path, line)`.
    /// Returns the remaining line set for that source. No-op if absent
    /// or if no `debug_state` exists.
    pub fn dap_remove_breakpoint(&mut self, source_path: String, line: i64) -> Vec<i64> {
        if self.debug_state.is_none() {
            return Vec::new();
        }
        self.mutate_breakpoint(source_path, line, /* ensure_present = */ false)
    }

    /// Shared body for `dap_set_breakpoint`/`dap_remove_breakpoint`.
    /// Precondition: `self.debug_state` is `Some`. Returns the full
    /// line set for the source after the op (idempotent — unchanged if
    /// the breakpoint was already in the requested state).
    fn mutate_breakpoint(
        &mut self,
        source_path: String,
        line: i64,
        ensure_present: bool,
    ) -> Vec<i64> {
        let state = self
            .debug_state
            .as_mut()
            .expect("mutate_breakpoint called without debug_state");
        let current: Vec<i64> = state
            .breakpoints
            .get(&source_path)
            .map(|bps| bps.iter().map(|b| b.line).collect())
            .unwrap_or_default();
        if current.contains(&line) == ensure_present {
            // Already in the desired state — no mutation, no adapter notify.
            return current;
        }
        let lines = state.toggle_breakpoint_at(source_path.clone(), line);
        self.push_set_breakpoints_if_dap(source_path, lines.clone());
        lines
    }

    /// Push a `SetBreakpoints` intent iff the current session is DAP.
    /// Extracted so set/remove share one place to decide whether the
    /// adapter needs to hear about a breakpoint change.
    fn push_set_breakpoints_if_dap(&mut self, source_path: String, lines: Vec<i64>) {
        if matches!(
            self.debug_state.as_ref().map(|s| &s.target),
            Some(DebugTarget::Dap { .. })
        ) {
            self.pending_dap_intents
                .push(DapIntent::SetBreakpoints { source_path, lines });
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

    /// Resume execution on the active thread. No-op if no session.
    pub fn dap_continue(&mut self) {
        let Some(tid) = self.dap_active_thread_id() else {
            return;
        };
        self.pending_dap_intents
            .push(DapIntent::Continue { thread_id: tid });
        self.set_status("[DAP] continue");
    }

    /// Step on the active thread with the given step kind. No-op if no
    /// session. One method replaces three near-identical helpers so the
    /// AI tool, the `:debug-step-*` commands, and any future Scheme
    /// caller share the same dispatch.
    pub fn dap_step(&mut self, kind: StepKind) {
        let Some(tid) = self.dap_active_thread_id() else {
            return;
        };
        let intent = match kind {
            StepKind::Over => DapIntent::Next { thread_id: tid },
            StepKind::In => DapIntent::StepIn { thread_id: tid },
            StepKind::Out => DapIntent::StepOut { thread_id: tid },
        };
        self.pending_dap_intents.push(intent);
        self.set_status(format!("[DAP] step {}", kind.as_str()));
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
        self.pending_dap_intents
            .push(DapIntent::Disconnect { terminate_debuggee });
        self.debug_state = None;
        self.set_status("[DAP] disconnected");
    }

    /// The thread id the UI is currently focused on, or `None` if there
    /// is no active session. Callers must early-out on `None` rather than
    /// forwarding a sentinel thread id to the adapter.
    fn dap_active_thread_id(&self) -> Option<i64> {
        self.debug_state.as_ref().map(|s| s.active_thread_id)
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
            for t in state.threads.iter_mut() {
                if all_threads || t.id == thread_id {
                    t.stopped = false;
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
        let prior: std::collections::HashMap<i64, bool> =
            state.threads.iter().map(|t| (t.id, t.stopped)).collect();
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
        // Peek the fields we need from the top frame before consuming `frames`.
        // Cloning just the source string avoids two clones of the full 5-tuple.
        let top = frames
            .first()
            .map(|(id, _name, src, line, _col)| (*id, src.clone(), *line));
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

        if let Some((frame_id, src, line)) = top {
            if let Some(src) = src {
                state.set_stopped_location(src, line);
            }
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
    pub fn apply_dap_breakpoints_set(
        &mut self,
        source_path: String,
        entries: Vec<(i64, bool, i64)>,
    ) {
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

/// Read an env var, falling back to a default string. Keeps the adapter
/// preset table below compact and overridable without touching source.
fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.into())
}

/// Default adapter spawn config for a short adapter name. Returns None
/// for unknown adapters. Callers can override the preset binary via
/// environment variable if the preset doesn't match the user's setup.
fn default_spawn_for_adapter(adapter: &str) -> Option<DapSpawnConfig> {
    match adapter {
        "lldb" | "lldb-dap" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_LLDB", "lldb-dap"),
            args: vec![],
            adapter_id: "lldb".into(),
        }),
        "codelldb" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_CODELLDB", "codelldb"),
            args: vec!["--port".into(), "0".into()],
            adapter_id: "codelldb".into(),
        }),
        "debugpy" | "python" => Some(DapSpawnConfig {
            command: env_or("MAE_DAP_DEBUGPY", "python"),
            args: vec!["-m".into(), "debugpy.adapter".into()],
            adapter_id: "debugpy".into(),
        }),
        _ => None,
    }
}

/// Build the adapter-specific launch args JSON for a `program` path.
/// Keeps the preset minimal so most real programs just work.
fn default_launch_args(adapter: &str, program: &str, extra: &[String]) -> serde_json::Value {
    let base_args: Vec<String> = extra.to_vec();
    match adapter {
        "debugpy" | "python" => serde_json::json!({
            "request": "launch",
            "type": "python",
            "program": program,
            "args": base_args,
            "console": "internalConsole",
            "stopOnEntry": false,
        }),
        _ => serde_json::json!({
            "request": "launch",
            "program": program,
            "args": base_args,
            "stopOnEntry": false,
        }),
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
        ed.dap_step(StepKind::Over);
        ed.dap_step(StepKind::In);
        ed.dap_step(StepKind::Out);
        assert_eq!(ed.pending_dap_intents.len(), 4);
        assert!(matches!(
            ed.pending_dap_intents[0],
            DapIntent::Continue { thread_id: 7 }
        ));
        assert!(matches!(
            ed.pending_dap_intents[1],
            DapIntent::Next { thread_id: 7 }
        ));
        assert!(matches!(
            ed.pending_dap_intents[2],
            DapIntent::StepIn { thread_id: 7 }
        ));
        assert!(matches!(
            ed.pending_dap_intents[3],
            DapIntent::StepOut { thread_id: 7 }
        ));
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
        ed.apply_dap_breakpoints_set("/a.rs".into(), vec![(99, true, 1), (100, false, 5)]);
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
            DapIntent::Disconnect {
                terminate_debuggee: false
            }
        ));
    }

    #[test]
    fn dap_set_breakpoint_adds_and_is_idempotent() {
        let mut ed = Editor::new();
        let lines = ed.dap_set_breakpoint("/a.rs".into(), 10);
        assert_eq!(lines, vec![10]);
        // Idempotent — calling again does not duplicate or re-queue.
        let intents_before = ed.pending_dap_intents.len();
        let lines2 = ed.dap_set_breakpoint("/a.rs".into(), 10);
        assert_eq!(lines2, vec![10]);
        assert_eq!(ed.pending_dap_intents.len(), intents_before);
        assert_eq!(
            ed.debug_state.as_ref().unwrap().breakpoints["/a.rs"].len(),
            1
        );
    }

    #[test]
    fn dap_set_breakpoint_queues_intent_in_dap_session() {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "/bin/ls".into(),
        }));
        ed.dap_set_breakpoint("/a.rs".into(), 10);
        assert!(matches!(
            ed.pending_dap_intents[0],
            DapIntent::SetBreakpoints { .. }
        ));
    }

    #[test]
    fn dap_set_breakpoint_multiple_lines_same_source() {
        let mut ed = Editor::new();
        ed.dap_set_breakpoint("/a.rs".into(), 10);
        ed.dap_set_breakpoint("/a.rs".into(), 20);
        let lines = ed.dap_set_breakpoint("/a.rs".into(), 30);
        assert_eq!(lines.len(), 3);
        assert!(lines.contains(&10));
        assert!(lines.contains(&20));
        assert!(lines.contains(&30));
    }

    #[test]
    fn dap_remove_breakpoint_removes_and_is_idempotent() {
        let mut ed = Editor::new();
        ed.dap_set_breakpoint("/a.rs".into(), 10);
        ed.dap_set_breakpoint("/a.rs".into(), 20);
        let lines = ed.dap_remove_breakpoint("/a.rs".into(), 10);
        assert_eq!(lines, vec![20]);
        // Removing again is a no-op.
        let lines2 = ed.dap_remove_breakpoint("/a.rs".into(), 10);
        assert_eq!(lines2, vec![20]);
    }

    #[test]
    fn dap_remove_breakpoint_no_state_is_noop() {
        let mut ed = Editor::new();
        let lines = ed.dap_remove_breakpoint("/a.rs".into(), 10);
        assert!(lines.is_empty());
        assert!(ed.debug_state.is_none());
    }

    #[test]
    fn dap_continue_without_session_is_noop() {
        let mut ed = Editor::new();
        ed.dap_continue();
        assert!(ed.pending_dap_intents.is_empty());
    }

    #[test]
    fn dap_step_without_session_is_noop() {
        let mut ed = Editor::new();
        ed.dap_step(StepKind::Over);
        ed.dap_step(StepKind::In);
        ed.dap_step(StepKind::Out);
        assert!(ed.pending_dap_intents.is_empty());
    }

    #[test]
    fn default_spawn_for_lldb() {
        let spawn = default_spawn_for_adapter("lldb").unwrap();
        assert_eq!(spawn.adapter_id, "lldb");
    }

    #[test]
    fn default_spawn_unknown_adapter() {
        assert!(default_spawn_for_adapter("nonexistent").is_none());
    }

    #[test]
    fn default_launch_args_python_shape() {
        let v = default_launch_args("debugpy", "/tmp/x.py", &[]);
        assert_eq!(v["type"], "python");
        assert_eq!(v["program"], "/tmp/x.py");
    }

    #[test]
    fn default_launch_args_lldb_shape() {
        let v = default_launch_args("lldb", "/bin/ls", &["--help".to_string()]);
        assert_eq!(v["program"], "/bin/ls");
        assert_eq!(v["args"][0], "--help");
    }

    #[test]
    fn dap_start_with_adapter_queues_intent() {
        let mut ed = Editor::new();
        ed.dap_start_with_adapter("lldb", "/bin/ls", &[]).unwrap();
        assert_eq!(ed.pending_dap_intents.len(), 1);
        assert!(matches!(
            ed.pending_dap_intents[0],
            DapIntent::StartSession { attach: false, .. }
        ));
    }

    #[test]
    fn dap_start_with_adapter_unknown_returns_err() {
        let mut ed = Editor::new();
        let err = ed
            .dap_start_with_adapter("bogus", "/bin/ls", &[])
            .unwrap_err();
        assert!(err.contains("Unknown adapter"));
        assert!(ed.pending_dap_intents.is_empty());
        assert!(ed.debug_state.is_none());
    }

    #[test]
    fn dap_start_with_adapter_rejects_concurrent_session() {
        let mut ed = Editor::new();
        ed.dap_start_with_adapter("lldb", "/bin/ls", &[]).unwrap();
        let intents_before = ed.pending_dap_intents.len();
        let err = ed
            .dap_start_with_adapter("lldb", "/bin/sh", &[])
            .unwrap_err();
        assert!(err.contains("already active"));
        // No extra intent should have been queued by the rejected call.
        assert_eq!(ed.pending_dap_intents.len(), intents_before);
    }
}
