//! Debug-adapter request intents.
//!
//! Mirrors `lsp_intent.rs`: the editor's synchronous dispatch layer cannot
//! call the async DAP client directly, so commands that drive a debug
//! session push a `DapIntent` onto the editor's queue. The binary drains
//! the queue each event-loop iteration and forwards each intent to
//! `run_dap_task`.
//!
//! Keeping this type in `mae-core` lets `mae-dap` depend on nothing from
//! core (except the debug-state data types). The conversion from intent
//! to `DapCommand` happens at the binary boundary.

/// Configuration needed to spawn a debug adapter. Mirrors the shape of
/// `mae_dap::DapServerConfig` without depending on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DapSpawnConfig {
    /// Adapter executable (e.g. "lldb-dap", "debugpy-adapter").
    pub command: String,
    /// Arguments to the adapter.
    pub args: Vec<String>,
    /// Adapter id reported to the adapter at initialize time.
    pub adapter_id: String,
}

/// A debug-adapter request pending dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum DapIntent {
    /// Spawn a new session: initialize → (launch|attach) → configurationDone.
    /// `launch_args` is the adapter-specific JSON payload (program, cwd, etc.).
    StartSession {
        spawn: DapSpawnConfig,
        launch_args: serde_json::Value,
        attach: bool,
    },
    /// Replace breakpoints for a source file (full-file replace per DAP spec).
    SetBreakpoints {
        source_path: String,
        lines: Vec<i64>,
    },
    /// Resume execution.
    Continue { thread_id: i64 },
    /// Step over.
    Next { thread_id: i64 },
    /// Step into.
    StepIn { thread_id: i64 },
    /// Step out.
    StepOut { thread_id: i64 },
    /// Request current threads + stack for a thread (or the first if None).
    RefreshThreadsAndStack { thread_id: Option<i64> },
    /// Request scopes for a stack frame.
    RequestScopes { frame_id: i64 },
    /// Request variables for a variablesReference.
    RequestVariables {
        scope_name: String,
        variables_reference: i64,
    },
    /// Soft terminate the debuggee (if the adapter supports it).
    Terminate,
    /// Hard disconnect (ends the adapter subprocess).
    Disconnect { terminate_debuggee: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dap_spawn_config_is_clonable_and_eq() {
        let a = DapSpawnConfig {
            command: "lldb-dap".into(),
            args: vec!["--port".into(), "0".into()],
            adapter_id: "lldb".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn start_session_intent_round_trip_values() {
        let intent = DapIntent::StartSession {
            spawn: DapSpawnConfig {
                command: "debugpy-adapter".into(),
                args: vec![],
                adapter_id: "debugpy".into(),
            },
            launch_args: serde_json::json!({"program": "/tmp/x.py"}),
            attach: false,
        };
        match intent {
            DapIntent::StartSession { spawn, launch_args, attach } => {
                assert_eq!(spawn.adapter_id, "debugpy");
                assert_eq!(launch_args["program"], "/tmp/x.py");
                assert!(!attach);
            }
            other => panic!("expected StartSession, got: {:?}", other),
        }
    }

    #[test]
    fn set_breakpoints_intent_stores_lines() {
        let intent = DapIntent::SetBreakpoints {
            source_path: "/tmp/a.rs".into(),
            lines: vec![10, 20, 30],
        };
        match intent {
            DapIntent::SetBreakpoints { source_path, lines } => {
                assert_eq!(source_path, "/tmp/a.rs");
                assert_eq!(lines, vec![10, 20, 30]);
            }
            other => panic!("expected SetBreakpoints, got: {:?}", other),
        }
    }
}
