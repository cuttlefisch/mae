use super::super::Editor;

impl Editor {
    /// Dispatch debug/DAP commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_dap(&mut self, name: &str) -> Option<bool> {
        match name {
            "debug-self" => {
                self.start_self_debug();
            }
            "debug-start" => {
                self.set_mode(crate::Mode::Command);
                self.command_line = "debug-start ".to_string();
                self.command_cursor = self.command_line.len();
            }
            "debug-stop" => {
                if self.debug_state.is_some() {
                    let is_dap = matches!(
                        self.debug_state.as_ref().map(|s| &s.target),
                        Some(crate::debug::DebugTarget::Dap { .. })
                    );
                    if is_dap {
                        self.dap_disconnect(true);
                    } else {
                        self.debug_state = None;
                        self.set_status("Debug session ended");
                    }
                } else {
                    self.set_status("No active debug session");
                }
            }
            "debug-continue" | "debug-step-over" | "debug-step-into" | "debug-step-out" => {
                if self.debug_state.is_none() {
                    self.set_status("No active debug session");
                } else {
                    match name {
                        "debug-continue" => self.dap_continue(),
                        "debug-step-over" => self.dap_step(crate::StepKind::Over),
                        "debug-step-into" => self.dap_step(crate::StepKind::In),
                        "debug-step-out" => self.dap_step(crate::StepKind::Out),
                        _ => unreachable!("unhandled debug command: {name}"),
                    }
                }
            }
            "debug-toggle-breakpoint" => {
                self.dap_toggle_breakpoint_at_cursor();
            }
            "debug-inspect" => {
                if let Some(state) = &self.debug_state {
                    let thread_info = if state.threads.is_empty() {
                        "no threads".to_string()
                    } else {
                        let stopped: Vec<&str> = state
                            .threads
                            .iter()
                            .filter(|t| t.stopped)
                            .map(|t| t.name.as_str())
                            .collect();
                        if stopped.is_empty() {
                            format!("{} threads (all running)", state.threads.len())
                        } else {
                            format!("{} stopped: {}", stopped.len(), stopped.join(", "))
                        }
                    };
                    let frame_info = state
                        .stack_frames
                        .first()
                        .map(|f| format!(" | top: {}:{}", f.name, f.line))
                        .unwrap_or_default();
                    let var_count: usize = state.variables.values().map(|v| v.len()).sum();
                    self.set_status(format!(
                        "Debug: {}{}  | {} vars across {} scopes",
                        thread_info,
                        frame_info,
                        var_count,
                        state.scopes.len()
                    ));
                } else {
                    self.set_status("No active debug session");
                }
            }
            "debug-panel" => {
                self.toggle_debug_panel();
            }
            "debug-panel-select" => {
                self.debug_panel_select();
            }
            "close-debug-panel" => {
                self.close_debug_panel();
            }
            "debug-toggle-output" => {
                self.debug_toggle_output();
            }
            "debug-move-down" => {
                let idx = self.active_buffer_idx();
                if let Some(view) = self.buffers[idx].debug_view_mut() {
                    view.move_down();
                }
            }
            "debug-move-up" => {
                let idx = self.active_buffer_idx();
                if let Some(view) = self.buffers[idx].debug_view_mut() {
                    view.move_up();
                }
            }
            "dap-refresh" => {
                self.dap_refresh();
                self.debug_panel_refresh_if_open();
            }
            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }
}
