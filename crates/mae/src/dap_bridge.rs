//! DAP bridge — translates between editor-side intents and DAP transport commands,
//! and handles incoming DAP events.

use mae_core::{DapIntent, Editor};
use mae_dap::{DapCommand, DapServerConfig, DapTaskEvent, SourceBreakpoint};
use tracing::{debug, info, warn};

/// Drain all pending DAP intents from the editor and forward them to the DAP task.
/// Safe to call every loop iteration — the Vec is cleared in place.
pub(crate) fn drain_dap_intents(
    editor: &mut Editor,
    dap_tx: &tokio::sync::mpsc::Sender<DapCommand>,
) {
    if editor.pending_dap_intents.is_empty() {
        return;
    }
    let intents = std::mem::take(&mut editor.pending_dap_intents);
    for intent in intents {
        let cmd = intent_to_dap_command(intent);
        let kind = dap_command_name(&cmd);
        if dap_tx.try_send(cmd).is_err() {
            warn!(kind, "DAP command channel full or closed — intent dropped");
        }
    }
}

/// Short name of a DAP command for logging — used only for diagnostics so
/// a dropped intent is attributable to a specific operation.
fn dap_command_name(cmd: &DapCommand) -> &'static str {
    match cmd {
        DapCommand::StartSession { .. } => "start-session",
        DapCommand::SetBreakpoints { .. } => "set-breakpoints",
        DapCommand::Continue { .. } => "continue",
        DapCommand::Next { .. } => "next",
        DapCommand::StepIn { .. } => "step-in",
        DapCommand::StepOut { .. } => "step-out",
        DapCommand::RefreshThreadsAndStack { .. } => "refresh-threads-and-stack",
        DapCommand::RequestScopes { .. } => "request-scopes",
        DapCommand::RequestVariables { .. } => "request-variables",
        DapCommand::Evaluate { .. } => "evaluate",
        DapCommand::Terminate => "terminate",
        DapCommand::Disconnect { .. } => "disconnect",
        DapCommand::Shutdown => "shutdown",
    }
}

/// Translate an editor-side `DapIntent` into a transport-layer `DapCommand`.
/// The core crate has no `mae-dap` dependency, so the binary performs the crosswalk.
fn intent_to_dap_command(intent: DapIntent) -> DapCommand {
    match intent {
        DapIntent::StartSession {
            spawn,
            launch_args,
            attach,
        } => DapCommand::StartSession {
            config: DapServerConfig {
                command: spawn.command,
                args: spawn.args,
                adapter_id: spawn.adapter_id,
            },
            launch_args,
            attach,
        },
        DapIntent::SetBreakpoints {
            source_path,
            breakpoints,
        } => DapCommand::SetBreakpoints {
            source_path,
            breakpoints: breakpoints
                .into_iter()
                .map(|bp| SourceBreakpoint {
                    line: bp.line,
                    condition: bp.condition,
                    hit_condition: bp.hit_condition,
                })
                .collect(),
        },
        DapIntent::Evaluate {
            expression,
            frame_id,
            context,
        } => DapCommand::Evaluate {
            expression,
            frame_id,
            context,
        },
        DapIntent::Continue { thread_id } => DapCommand::Continue { thread_id },
        DapIntent::Next { thread_id } => DapCommand::Next { thread_id },
        DapIntent::StepIn { thread_id } => DapCommand::StepIn { thread_id },
        DapIntent::StepOut { thread_id } => DapCommand::StepOut { thread_id },
        DapIntent::RefreshThreadsAndStack { thread_id } => {
            DapCommand::RefreshThreadsAndStack { thread_id }
        }
        DapIntent::RequestScopes { frame_id } => DapCommand::RequestScopes { frame_id },
        DapIntent::RequestVariables {
            scope_name,
            variables_reference,
        } => DapCommand::RequestVariables {
            scope_name,
            variables_reference,
        },
        DapIntent::Terminate => DapCommand::Terminate,
        DapIntent::Disconnect { terminate_debuggee } => {
            DapCommand::Disconnect { terminate_debuggee }
        }
    }
}

/// Handle an event from the DAP task — update editor state via `apply_dap_*`.
pub(crate) fn handle_dap_event(editor: &mut Editor, event: DapTaskEvent) {
    match event {
        DapTaskEvent::SessionStarted {
            adapter_id,
            capabilities: _,
        } => {
            info!(adapter = %adapter_id, "DAP session started");
            editor.apply_dap_session_started(adapter_id);
        }
        DapTaskEvent::SessionStartFailed { error } => {
            warn!(error = %error, "DAP session start failed");
            editor.apply_dap_session_start_failed(error);
        }
        DapTaskEvent::Stopped {
            reason,
            thread_id,
            text,
        } => {
            debug!(reason = %reason, thread_id = ?thread_id, "DAP stopped");
            editor.apply_dap_stopped(reason, thread_id, text);
        }
        DapTaskEvent::Continued {
            thread_id,
            all_threads,
        } => {
            editor.apply_dap_continued(thread_id, all_threads);
        }
        DapTaskEvent::ThreadEvent {
            reason: _,
            thread_id: _,
        } => {
            // Drive a thread-list refresh on any thread start/exit so the UI
            // stays in sync with reality.
            editor.dap_refresh();
        }
        DapTaskEvent::Output { category, output } => {
            editor.apply_dap_output(category, output);
        }
        DapTaskEvent::Terminated => {
            editor.apply_dap_terminated();
        }
        DapTaskEvent::AdapterExited => {
            editor.apply_dap_adapter_exited();
        }
        DapTaskEvent::Error { message } => {
            warn!(error = %message, "DAP error");
            editor.apply_dap_error(message);
        }
        DapTaskEvent::ThreadsResult { threads } => {
            let core_threads: Vec<(i64, String)> =
                threads.into_iter().map(|t| (t.id, t.name)).collect();
            editor.apply_dap_threads(core_threads);
        }
        DapTaskEvent::StackTraceResult { thread_id, frames } => {
            let core_frames: Vec<(i64, String, Option<String>, i64, i64)> = frames
                .into_iter()
                .map(|f| {
                    let src = f.source.and_then(|s| s.path.or(s.name));
                    (f.id, f.name, src, f.line, f.column)
                })
                .collect();
            editor.apply_dap_stack_trace(thread_id, core_frames);
        }
        DapTaskEvent::ScopesResult { frame_id, scopes } => {
            let core_scopes: Vec<(String, i64, bool)> = scopes
                .into_iter()
                .map(|s| (s.name, s.variables_reference, s.expensive))
                .collect();
            editor.apply_dap_scopes(frame_id, core_scopes);
        }
        DapTaskEvent::VariablesResult {
            scope_name,
            variables,
        } => {
            let core_vars: Vec<(String, String, Option<String>, i64)> = variables
                .into_iter()
                .map(|v| (v.name, v.value, v.type_field, v.variables_reference))
                .collect();
            editor.apply_dap_variables(scope_name, core_vars);
        }
        DapTaskEvent::BreakpointsSet {
            source_path,
            breakpoints,
        } => {
            let entries: Vec<(i64, bool, i64)> = breakpoints
                .into_iter()
                .filter_map(|b| b.line.map(|line| (b.id.unwrap_or(0), b.verified, line)))
                .collect();
            editor.apply_dap_breakpoints_set(source_path, entries);
        }
        DapTaskEvent::EvaluateResult {
            expression,
            result,
            type_field,
            variables_reference: _,
        } => {
            if let Some(ref mut ds) = editor.debug_state {
                ds.log(format!(
                    "eval: {} = {} ({})",
                    expression,
                    result,
                    type_field.as_deref().unwrap_or("?")
                ));
            }
            editor.set_status(format!("= {}", result));
        }
    }
}
