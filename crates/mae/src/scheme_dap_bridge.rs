//! Scheme DAP bridge — handles debug intents for mae-scheme in-process.
//!
//! When the debug target is `Dap { adapter_name: "mae-scheme", .. }`, this
//! bridge intercepts DAP intents before they reach the external DAP task.
//! Breakpoints, stepping, and frame inspection are handled synchronously
//! by the embedded Scheme VM.

use mae_core::debug::{Breakpoint, DebugTarget, DebugThread, Scope, StackFrame, Variable};
use mae_core::{DapIntent, Editor};
use mae_scheme::vm::{EvalResult, StepMode, YieldRequest};
use mae_scheme::SchemeRuntime;
use tracing::debug;

/// Returns true if the current debug session is a mae-scheme session.
fn is_scheme_dap(editor: &Editor) -> bool {
    matches!(
        editor.dap.state.as_ref().map(|s| &s.target),
        Some(DebugTarget::Dap {
            adapter_name,
            ..
        }) if adapter_name == "mae-scheme"
    )
}

/// Drain scheme-specific DAP intents from the editor and handle them in-process.
///
/// Call this BEFORE `drain_dap_intents` so scheme DAP intents never reach the
/// external DAP task. Returns true if any intent was handled (needs redraw).
pub(crate) fn drain_scheme_dap_intents(editor: &mut Editor, scheme: &mut SchemeRuntime) -> bool {
    if editor.dap.pending_intents.is_empty() || !is_scheme_dap(editor) {
        return false;
    }

    // Take all intents — they're all ours since the target is mae-scheme
    let intents = std::mem::take(&mut editor.dap.pending_intents);

    if intents.is_empty() {
        return false;
    }

    let mut needs_redraw = false;

    for intent in intents {
        match intent {
            DapIntent::StartSession { spawn, .. } => {
                debug!(adapter = %spawn.adapter_id, "scheme DAP session starting");
                scheme.vm_mut().debug_mode = true;
                sync_breakpoints_to_vm(editor, scheme);

                if let Some(state) = editor.dap.state.as_mut() {
                    state.threads = vec![DebugThread {
                        id: 1,
                        name: "Scheme Main".into(),
                        stopped: false,
                    }];
                    state.active_thread_id = 1;
                }

                // Read and evaluate the program file
                let program = editor.dap.state.as_ref().and_then(|s| match &s.target {
                    DebugTarget::Dap { program, .. } => Some(program.clone()),
                    _ => None,
                });

                if let Some(file) = program {
                    if let Ok(content) = std::fs::read_to_string(&file) {
                        let vm = scheme.vm_mut();
                        match vm.eval_with_file_yielding(&content, &file) {
                            Ok(EvalResult::Yield(YieldRequest::Breakpoint(info))) => {
                                apply_breakpoint_info(editor, &info);
                                editor.set_status(format!(
                                    "[Scheme DAP] breakpoint hit: {}:{}",
                                    info.file, info.line
                                ));
                            }
                            Ok(EvalResult::Yield(_)) => {
                                editor.set_status("[Scheme DAP] program yielded (non-breakpoint)");
                            }
                            Ok(EvalResult::Done(val)) => {
                                editor
                                    .set_status(format!("[Scheme DAP] program completed: {}", val));
                                // Program ran without hitting any breakpoints
                                scheme.vm_mut().debug_mode = false;
                                editor.dap.state = None;
                            }
                            Err(e) => {
                                editor.set_status(format!("[Scheme DAP] error: {}", e.message()));
                                scheme.vm_mut().debug_mode = false;
                                editor.dap.state = None;
                            }
                        }
                    } else {
                        editor.set_status(format!("[Scheme DAP] cannot read file: {}", file));
                    }
                } else {
                    editor.set_status("[Scheme DAP] session started (in-process)");
                }
                needs_redraw = true;
            }

            DapIntent::SetBreakpoints {
                source_path,
                breakpoints,
            } => {
                // Update VM breakpoints for this file
                let line_set: std::collections::HashSet<u32> =
                    breakpoints.iter().map(|bp| bp.line as u32).collect();
                if line_set.is_empty() {
                    scheme.vm_mut().breakpoints.remove(&source_path);
                } else {
                    scheme
                        .vm_mut()
                        .breakpoints
                        .insert(source_path.clone(), line_set);
                }

                // Mark all breakpoints as verified in editor state
                if let Some(state) = editor.dap.state.as_mut() {
                    let verified: Vec<Breakpoint> = breakpoints
                        .iter()
                        .enumerate()
                        .map(|(i, bp)| Breakpoint {
                            id: i as i64 + 1,
                            verified: true,
                            source: source_path.clone(),
                            line: bp.line,
                            condition: bp.condition.clone(),
                            hit_condition: bp.hit_condition.clone(),
                        })
                        .collect();
                    state.breakpoints.insert(source_path, verified);
                }
                debug!("scheme DAP: breakpoints synced to VM");
                needs_redraw = true;
            }

            DapIntent::Continue { .. } => {
                {
                    let vm = scheme.vm_mut();
                    vm.step_mode = StepMode::Run;
                    vm.last_break_line_clear();
                }
                resume_and_apply(editor, scheme);
                needs_redraw = true;
            }

            DapIntent::Next { .. } => {
                {
                    let depth = scheme.vm().frame_count();
                    let vm = scheme.vm_mut();
                    vm.step_mode = StepMode::StepOver(depth);
                    vm.last_break_line_clear();
                }
                resume_and_apply(editor, scheme);
                needs_redraw = true;
            }

            DapIntent::StepIn { .. } => {
                {
                    let vm = scheme.vm_mut();
                    vm.step_mode = StepMode::StepIn;
                    vm.last_break_line_clear();
                }
                resume_and_apply(editor, scheme);
                needs_redraw = true;
            }

            DapIntent::StepOut { .. } => {
                {
                    let depth = scheme.vm().frame_count();
                    let vm = scheme.vm_mut();
                    vm.step_mode = StepMode::StepOut(depth);
                    vm.last_break_line_clear();
                }
                resume_and_apply(editor, scheme);
                needs_redraw = true;
            }

            DapIntent::Evaluate { expression, .. } => {
                let result = scheme.eval(&expression);
                let output = match result {
                    Ok(val) => val,
                    Err(e) => format!("Error: {}", e.message),
                };
                if let Some(state) = editor.dap.state.as_mut() {
                    state
                        .output_log
                        .push(format!("eval> {} => {}", expression, output));
                }
                editor.set_status(format!("[Scheme DAP] {}", output));
                needs_redraw = true;
            }

            DapIntent::Terminate | DapIntent::Disconnect { .. } => {
                let vm = scheme.vm_mut();
                vm.debug_mode = false;
                vm.breakpoints.clear();
                vm.step_mode = StepMode::Run;
                editor.dap.state = None;
                editor.set_status("[Scheme DAP] session ended");
                needs_redraw = true;
            }

            DapIntent::RefreshThreadsAndStack { .. } => {
                needs_redraw = true;
            }

            DapIntent::RequestScopes { frame_id } => {
                if let Some(state) = editor.dap.state.as_mut() {
                    state.scopes = vec![Scope {
                        name: "Locals".into(),
                        variables_reference: frame_id,
                        expensive: false,
                    }];
                }
                needs_redraw = true;
            }

            DapIntent::RequestVariables { .. } => {
                // Variables already populated by apply_breakpoint_info
                needs_redraw = true;
            }

            _ => {
                debug!("scheme DAP: unhandled intent");
            }
        }
    }

    needs_redraw
}

/// Sync breakpoints from editor's DebugState to the VM's breakpoint map.
fn sync_breakpoints_to_vm(editor: &Editor, scheme: &mut SchemeRuntime) {
    if let Some(state) = editor.dap.state.as_ref() {
        let vm = scheme.vm_mut();
        for (source, bps) in &state.breakpoints {
            let lines: std::collections::HashSet<u32> = bps.iter().map(|b| b.line as u32).collect();
            if !lines.is_empty() {
                vm.breakpoints.insert(source.clone(), lines);
            }
        }
    }
}

/// Resume the VM after a continue/step, and apply the result to the editor.
fn resume_and_apply(editor: &mut Editor, scheme: &mut SchemeRuntime) {
    match scheme.resume_yield(mae_scheme::value::Value::Bool(true)) {
        Ok(result) => match result {
            mae_scheme::SchemeEvalResult::Yield(YieldRequest::Breakpoint(info)) => {
                apply_breakpoint_info(editor, &info);
                editor.set_status(format!(
                    "[Scheme DAP] breakpoint hit: {}:{}",
                    info.file, info.line
                ));
            }
            mae_scheme::SchemeEvalResult::Yield(other) => {
                debug!(?other, "scheme DAP: non-breakpoint yield during resume");
                editor.set_status("[Scheme DAP] program yielded (non-breakpoint)");
            }
            mae_scheme::SchemeEvalResult::Done(result) => {
                if let Some(state) = editor.dap.state.as_mut() {
                    state.stopped_location = None;
                    state.last_stop_reason = None;
                    state.stack_frames.clear();
                    state.scopes.clear();
                    state.variables.clear();
                    for t in state.threads.iter_mut() {
                        t.stopped = false;
                    }
                    state
                        .output_log
                        .push(format!("Program finished: {}", result));
                }
                editor.set_status(format!("[Scheme DAP] program finished: {}", result));
            }
        },
        Err(e) => {
            editor.set_status(format!("[Scheme DAP] error: {}", e.message));
            if let Some(state) = editor.dap.state.as_mut() {
                state.output_log.push(format!("Error: {}", e.message));
            }
        }
    }
}

/// Apply breakpoint info from the VM to the editor's DebugState.
fn apply_breakpoint_info(editor: &mut Editor, info: &mae_scheme::vm::BreakpointInfo) {
    let Some(state) = editor.dap.state.as_mut() else {
        return;
    };

    state.stopped_location = Some((info.file.clone(), info.line as i64));
    state.last_stop_reason = Some("breakpoint".into());

    for t in state.threads.iter_mut() {
        t.stopped = true;
    }

    state.stack_frames = info
        .frames
        .iter()
        .enumerate()
        .map(|(i, f)| StackFrame {
            id: i as i64,
            name: f.name.clone(),
            source: Some(f.file.clone()),
            line: f.line as i64,
            column: 1,
        })
        .collect();

    if !info.frames.is_empty() {
        state.scopes = vec![Scope {
            name: "Locals".into(),
            variables_reference: 0,
            expensive: false,
        }];
    }

    if let Some(top_frame) = info.frames.first() {
        let vars: Vec<Variable> = top_frame
            .locals
            .iter()
            .map(|(name, value)| Variable {
                name: name.clone(),
                value: value.clone(),
                var_type: Some("scheme".into()),
                variables_reference: 0,
            })
            .collect();
        state.variables.clear();
        state.variables.insert("Locals".into(), vars);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::debug::DebugState;

    fn test_editor() -> Editor {
        Editor::new()
    }

    #[test]
    fn is_scheme_dap_false_when_no_session() {
        let editor = test_editor();
        assert!(!is_scheme_dap(&editor));
    }

    #[test]
    fn is_scheme_dap_true_for_mae_scheme_adapter() {
        let mut editor = test_editor();
        editor.dap.state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "mae-scheme".into(),
            program: "test.scm".into(),
        }));
        assert!(is_scheme_dap(&editor));
    }

    #[test]
    fn is_scheme_dap_false_for_lldb_adapter() {
        let mut editor = test_editor();
        editor.dap.state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "test".into(),
        }));
        assert!(!is_scheme_dap(&editor));
    }

    #[test]
    fn sync_breakpoints_to_vm_works() {
        let mut editor = test_editor();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "mae-scheme".into(),
            program: "test.scm".into(),
        });
        state.breakpoints.insert(
            "test.scm".into(),
            vec![
                Breakpoint {
                    id: 1,
                    verified: true,
                    source: "test.scm".into(),
                    line: 5,
                    condition: None,
                    hit_condition: None,
                },
                Breakpoint {
                    id: 2,
                    verified: true,
                    source: "test.scm".into(),
                    line: 10,
                    condition: None,
                    hit_condition: None,
                },
            ],
        );
        editor.dap.state = Some(state);

        let mut runtime = SchemeRuntime::new().unwrap();
        sync_breakpoints_to_vm(&editor, &mut runtime);

        let vm = runtime.vm();
        let lines = vm.breakpoints.get("test.scm").unwrap();
        assert!(lines.contains(&5));
        assert!(lines.contains(&10));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn apply_breakpoint_info_populates_state() {
        let mut editor = test_editor();
        editor.dap.state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "mae-scheme".into(),
            program: "test.scm".into(),
        }));

        let info = mae_scheme::vm::BreakpointInfo {
            file: "test.scm".into(),
            line: 5,
            frames: vec![mae_scheme::vm::DebugFrame {
                name: "foo".into(),
                file: "test.scm".into(),
                line: 5,
                locals: vec![("x".into(), "42".into()), ("y".into(), "#t".into())],
            }],
        };

        apply_breakpoint_info(&mut editor, &info);

        let state = editor.dap.state.as_ref().unwrap();
        assert_eq!(state.stopped_location, Some(("test.scm".into(), 5)));
        assert_eq!(state.last_stop_reason.as_deref(), Some("breakpoint"));
        assert_eq!(state.stack_frames.len(), 1);
        assert_eq!(state.stack_frames[0].name, "foo");

        let locals = state.variables.get("Locals").unwrap();
        assert_eq!(locals.len(), 2);
        assert_eq!(locals[0].name, "x");
        assert_eq!(locals[0].value, "42");
    }
}
