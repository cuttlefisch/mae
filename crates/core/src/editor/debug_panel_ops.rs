//! Debug panel operations — commands that manipulate the `*Debug*` buffer.
//!
//! Mirrors `help_ops.rs`: the panel is a read-only buffer populated from
//! `DebugState`. The dispatch layer calls these methods; the renderer reads
//! the buffer's rope + `debug_view.line_map` for styling.

use crate::buffer::{Buffer, BufferKind};
use crate::debug_view::DebugLineItem;

use super::Editor;

impl Editor {
    /// Open the debug panel. If it already exists, switch to it.
    /// If no debug session is active, open with a "no session" message.
    pub fn open_debug_panel(&mut self) {
        let buf_idx = self.ensure_debug_buffer_idx();
        self.debug_populate_buffer(buf_idx);
        let prev = self.active_buffer_idx();
        if prev != buf_idx {
            self.alternate_buffer_idx = Some(prev);
        }
        let win = self.window_mgr.focused_window_mut();
        win.buffer_idx = buf_idx;
        win.cursor_row = 0;
        win.cursor_col = 0;
        self.set_mode(crate::Mode::Normal);
    }

    /// Close the debug panel buffer and switch to alternate.
    pub fn close_debug_panel(&mut self) {
        let maybe_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug);
        let Some(debug_idx) = maybe_idx else {
            return;
        };

        // Switch away first.
        let alt = self.alternate_buffer_idx.unwrap_or(0);
        let target = if alt < self.buffers.len() && alt != debug_idx {
            alt
        } else {
            // Find any non-debug buffer.
            self.buffers
                .iter()
                .position(|b| b.kind != BufferKind::Debug)
                .unwrap_or(0)
        };
        self.switch_to_buffer(target);

        // Remove the debug buffer.
        self.buffers.remove(debug_idx);
        self.adjust_ai_target_after_remove(debug_idx);

        // Fix up all window buffer_idx references.
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == debug_idx {
                win.buffer_idx = target.min(self.buffers.len().saturating_sub(1));
            } else if win.buffer_idx > debug_idx {
                win.buffer_idx -= 1;
            }
        }
        // Fix up alternate.
        if let Some(ref mut alt_idx) = self.alternate_buffer_idx {
            if *alt_idx == debug_idx {
                *alt_idx = 0;
            } else if *alt_idx > debug_idx {
                *alt_idx -= 1;
            }
        }
    }

    /// Toggle the debug panel open/closed.
    pub fn toggle_debug_panel(&mut self) {
        let is_open = self.buffers.iter().any(|b| b.kind == BufferKind::Debug);
        if is_open {
            self.close_debug_panel();
        } else {
            self.open_debug_panel();
        }
    }

    /// Find or create the `*Debug*` buffer. Returns buffer index.
    fn ensure_debug_buffer_idx(&mut self) -> usize {
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
        {
            return idx;
        }
        self.buffers.push(Buffer::new_debug());
        self.buffers.len() - 1
    }

    /// Populate (or re-populate) the debug buffer's rope from the current
    /// `DebugState`. Rebuilds the `line_map` for the renderer and key handler.
    pub fn debug_populate_buffer(&mut self, buf_idx: usize) {
        let (text, line_map) = self.render_debug_state(buf_idx);

        // Temporarily clear read_only to allow rope replacement.
        self.buffers[buf_idx].read_only = false;
        self.buffers[buf_idx].replace_contents(&text);
        self.buffers[buf_idx].read_only = true;

        if let Some(view) = self.buffers[buf_idx].debug_view_mut() {
            view.line_map = line_map;
            view.clamp_cursor();
        }
    }

    /// Render the debug state into text + line_map. Separate from
    /// `debug_populate_buffer` so it can borrow `self` immutably for
    /// the state while building the string.
    fn render_debug_state(&self, buf_idx: usize) -> (String, Vec<DebugLineItem>) {
        let mut text = String::new();
        let mut line_map: Vec<DebugLineItem> = Vec::new();

        let show_output = self.buffers[buf_idx]
            .debug_view()
            .map(|v| v.show_output)
            .unwrap_or(false);

        let Some(state) = &self.debug_state else {
            text.push_str("No active debug session.\n");
            text.push_str("\nStart one with :debug-start <adapter> <program>\n");
            text.push_str("or SPC d d\n");
            line_map.push(DebugLineItem::Blank);
            line_map.push(DebugLineItem::Blank);
            line_map.push(DebugLineItem::Blank);
            return (text, line_map);
        };

        if show_output {
            return self.render_output_view(state);
        }

        // --- Threads section ---
        text.push_str(" Threads ──────────────────────\n");
        line_map.push(DebugLineItem::SectionHeader("Threads".into()));

        for thread in &state.threads {
            let marker = if thread.stopped { "●" } else { "○" };
            let active = if thread.id == state.active_thread_id {
                " "
            } else {
                "  "
            };
            let status = if thread.stopped { "stopped" } else { "running" };
            text.push_str(&format!(
                "{}{} {} ({})\n",
                active, marker, thread.name, status
            ));
            line_map.push(DebugLineItem::Thread(thread.id));
        }

        if state.threads.is_empty() {
            text.push_str("  (no threads)\n");
            line_map.push(DebugLineItem::Blank);
        }

        // --- Blank separator ---
        text.push('\n');
        line_map.push(DebugLineItem::Blank);

        // --- Call Stack section ---
        text.push_str(" Call Stack ────────────────────\n");
        line_map.push(DebugLineItem::SectionHeader("Call Stack".into()));

        let selected_frame = self.buffers[buf_idx]
            .debug_view()
            .and_then(|v| v.selected_frame_id);

        for frame in &state.stack_frames {
            let marker = if Some(frame.id) == selected_frame {
                " → "
            } else {
                "   "
            };
            let source_info = match &frame.source {
                Some(src) => {
                    let short = src.rsplit('/').next().unwrap_or(src);
                    format!("{}:{}", short, frame.line)
                }
                None => String::new(),
            };
            if source_info.is_empty() {
                text.push_str(&format!("{}{}()\n", marker, frame.name));
            } else {
                // Right-align source info with padding
                let name_part = format!("{}()", frame.name);
                let padding = 30usize.saturating_sub(name_part.len());
                text.push_str(&format!(
                    "{}{}{}{}\n",
                    marker,
                    name_part,
                    " ".repeat(padding),
                    source_info
                ));
            }
            line_map.push(DebugLineItem::Frame(frame.id));
        }

        if state.stack_frames.is_empty() {
            text.push_str("   (no frames)\n");
            line_map.push(DebugLineItem::Blank);
        }

        // --- Blank separator ---
        text.push('\n');
        line_map.push(DebugLineItem::Blank);

        // --- Scopes/Variables sections ---
        let expanded_vars = self.buffers[buf_idx]
            .debug_view()
            .map(|v| v.expanded_vars.clone())
            .unwrap_or_default();
        let child_vars = self.buffers[buf_idx]
            .debug_view()
            .map(|v| v.child_variables.clone())
            .unwrap_or_default();

        for scope in &state.scopes {
            text.push_str(&format!(" {} ───────────────────────\n", scope.name));
            line_map.push(DebugLineItem::SectionHeader(scope.name.clone()));

            if let Some(vars) = state.variables.get(&scope.name) {
                self.render_variables(
                    vars,
                    &scope.name,
                    0,
                    &expanded_vars,
                    &child_vars,
                    &mut text,
                    &mut line_map,
                );
            } else if scope.expensive {
                text.push_str("   (expensive scope — select to load)\n");
                line_map.push(DebugLineItem::Blank);
            } else {
                text.push_str("   (loading...)\n");
                line_map.push(DebugLineItem::Blank);
            }
        }

        if state.scopes.is_empty() && !state.variables.is_empty() {
            // Variables exist but no scopes — show them flat.
            for (scope_name, vars) in &state.variables {
                text.push_str(&format!(" {} ───────────────────────\n", scope_name));
                line_map.push(DebugLineItem::SectionHeader(scope_name.clone()));
                self.render_variables(
                    vars,
                    scope_name,
                    0,
                    &expanded_vars,
                    &child_vars,
                    &mut text,
                    &mut line_map,
                );
            }
        }

        // --- Footer ---
        text.push('\n');
        line_map.push(DebugLineItem::Blank);
        let stop_info = match &state.stopped_location {
            Some((src, line)) => format!("Stopped at {}:{}", src, line),
            None => "Running".to_string(),
        };
        text.push_str(&format!(" {}\n", stop_info));
        line_map.push(DebugLineItem::Blank);
        text.push_str(" j/k:navigate  Enter:select  c:continue  n:next  s:step-in  S:step-out  o:output  q:close\n");
        line_map.push(DebugLineItem::Blank);

        (text, line_map)
    }

    /// Render variables at a given depth, recursing into expanded children.
    #[allow(clippy::too_many_arguments)]
    fn render_variables(
        &self,
        vars: &[crate::debug::Variable],
        scope_name: &str,
        depth: usize,
        expanded: &std::collections::HashSet<i64>,
        children: &std::collections::HashMap<i64, Vec<crate::debug::Variable>>,
        text: &mut String,
        line_map: &mut Vec<DebugLineItem>,
    ) {
        let indent = "   ".repeat(depth + 1);
        for var in vars {
            let expandable = var.variables_reference > 0;
            let marker = if expandable {
                if expanded.contains(&var.variables_reference) {
                    "▼ "
                } else {
                    "▶ "
                }
            } else {
                "  "
            };
            let type_str = var
                .var_type
                .as_deref()
                .map(|t| format!(": {}", t))
                .unwrap_or_default();
            text.push_str(&format!(
                "{}{}{}{} = {}\n",
                indent, marker, var.name, type_str, var.value
            ));
            line_map.push(DebugLineItem::Variable {
                scope: scope_name.to_string(),
                name: var.name.clone(),
                depth,
                variables_reference: var.variables_reference,
            });

            // Recurse into expanded children.
            if expandable && expanded.contains(&var.variables_reference) {
                if let Some(child_list) = children.get(&var.variables_reference) {
                    self.render_variables(
                        child_list,
                        scope_name,
                        depth + 1,
                        expanded,
                        children,
                        text,
                        line_map,
                    );
                } else {
                    // Children requested but not yet loaded.
                    let child_indent = "   ".repeat(depth + 2);
                    text.push_str(&format!("{}(loading...)\n", child_indent));
                    line_map.push(DebugLineItem::Blank);
                }
            }
        }
    }

    /// Render the output log view.
    fn render_output_view(&self, state: &crate::debug::DebugState) -> (String, Vec<DebugLineItem>) {
        let mut text = String::new();
        let mut line_map: Vec<DebugLineItem> = Vec::new();

        text.push_str(" Debug Output ─────────────────\n");
        line_map.push(DebugLineItem::SectionHeader("Output".into()));

        if state.output_log.is_empty() {
            text.push_str("  (no output)\n");
            line_map.push(DebugLineItem::Blank);
        } else {
            for (i, line) in state.output_log.iter().enumerate() {
                text.push_str(&format!("  {}\n", line));
                line_map.push(DebugLineItem::OutputLine(i));
            }
        }

        text.push('\n');
        line_map.push(DebugLineItem::Blank);
        text.push_str(" o:state view  q:close\n");
        line_map.push(DebugLineItem::Blank);

        (text, line_map)
    }

    /// Handle "select" action on the current cursor item in the debug panel.
    /// - Thread → set active thread, refresh
    /// - Frame → select frame, navigate to source, request scopes
    /// - Variable → toggle expansion
    pub fn debug_panel_select(&mut self) {
        let debug_idx = match self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
        {
            Some(i) => i,
            None => return,
        };

        let item = match self.buffers[debug_idx]
            .debug_view()
            .and_then(|v: &crate::debug_view::DebugView| v.cursor_item().cloned())
        {
            Some(item) => item,
            None => return,
        };

        match item {
            DebugLineItem::Thread(tid) => {
                if let Some(state) = self.debug_state.as_mut() {
                    state.set_active_thread(tid);
                }
                self.dap_refresh();
                self.debug_populate_buffer(debug_idx);
            }
            DebugLineItem::Frame(fid) => {
                if let Some(view) = self.buffers[debug_idx].debug_view_mut() {
                    view.selected_frame_id = Some(fid);
                }
                // Request scopes for this frame.
                self.dap_request_scopes(fid);
                // Navigate to source.
                self.debug_navigate_to_frame_source(fid);
                self.debug_populate_buffer(debug_idx);
            }
            DebugLineItem::Variable {
                variables_reference,
                scope,
                ..
            } if variables_reference > 0 => {
                let now_expanded = self.buffers[debug_idx]
                    .debug_view_mut()
                    .map(|v: &mut crate::debug_view::DebugView| {
                        v.toggle_expand(variables_reference)
                    })
                    .unwrap_or(false);

                if now_expanded {
                    // Check if children are already cached.
                    let has_children = self.buffers[debug_idx]
                        .debug_view()
                        .map(|v: &crate::debug_view::DebugView| {
                            v.child_variables.contains_key(&variables_reference)
                        })
                        .unwrap_or(false);
                    if !has_children {
                        self.dap_request_variables(scope, variables_reference);
                    }
                }
                self.debug_populate_buffer(debug_idx);
            }
            _ => {}
        }
    }

    /// Navigate to the source file/line of a stack frame.
    fn debug_navigate_to_frame_source(&mut self, frame_id: i64) {
        let frame = self
            .debug_state
            .as_ref()
            .and_then(|s| s.stack_frames.iter().find(|f| f.id == frame_id))
            .cloned();

        let Some(frame) = frame else { return };
        let Some(ref source) = frame.source else {
            return;
        };

        let path = std::path::Path::new(source);
        if !path.exists() {
            self.set_status(format!("Source not found: {}", source));
            return;
        }

        // Open the file (find existing or load).
        self.open_file(path);

        // Jump to line (DAP lines are 1-based, editor rows are 0-based).
        let row = (frame.line - 1).max(0) as usize;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = 0;
    }

    /// Toggle between state view and output log view.
    pub fn debug_toggle_output(&mut self) {
        let debug_idx = match self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
        {
            Some(i) => i,
            None => return,
        };

        if let Some(view) = self.buffers[debug_idx].debug_view_mut() {
            view.show_output = !view.show_output;
            view.cursor_index = 0;
        }
        self.debug_populate_buffer(debug_idx);
    }

    /// Refresh the debug panel if it exists.
    pub fn debug_panel_refresh_if_open(&mut self) {
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
        {
            self.debug_populate_buffer(idx);
        }
    }

    /// Store child variables from a DAP response into the debug view.
    pub fn debug_panel_store_children(
        &mut self,
        variables_reference: i64,
        children: Vec<crate::debug::Variable>,
    ) {
        if let Some(buf) = self
            .buffers
            .iter_mut()
            .find(|b| b.kind == BufferKind::Debug)
        {
            if let Some(view) = buf.debug_view_mut() {
                view.child_variables.insert(variables_reference, children);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer::BufferKind;
    use crate::debug::{DebugState, DebugTarget, DebugThread, Scope, StackFrame, Variable};
    use crate::editor::Editor;

    fn ed_with_debug_state() -> Editor {
        let mut ed = Editor::new();
        let mut state = DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "/bin/test".into(),
        });
        state.threads.push(DebugThread {
            id: 1,
            name: "main".into(),
            stopped: true,
        });
        state.threads.push(DebugThread {
            id: 2,
            name: "worker".into(),
            stopped: false,
        });
        state.active_thread_id = 1;
        state.stack_frames.push(StackFrame {
            id: 100,
            name: "main".into(),
            source: Some("main.rs".into()),
            line: 42,
            column: 0,
        });
        state.stack_frames.push(StackFrame {
            id: 101,
            name: "caller".into(),
            source: Some("lib.rs".into()),
            line: 10,
            column: 0,
        });
        state.scopes.push(Scope {
            name: "Locals".into(),
            variables_reference: 1,
            expensive: false,
        });
        state.variables.insert(
            "Locals".into(),
            vec![
                Variable {
                    name: "x".into(),
                    value: "42".into(),
                    var_type: Some("i32".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "editor".into(),
                    value: "Editor { ... }".into(),
                    var_type: Some("Editor".into()),
                    variables_reference: 50,
                },
            ],
        );
        state.set_stopped_location("main.rs", 42);
        ed.debug_state = Some(state);
        ed
    }

    #[test]
    fn open_creates_debug_buffer() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let debug_buf = ed.buffers.iter().find(|b| b.kind == BufferKind::Debug);
        assert!(debug_buf.is_some());
        let buf = debug_buf.unwrap();
        assert_eq!(buf.name, "*Debug*");
        assert!(buf.read_only);
        assert!(buf.debug_view().is_some());
    }

    #[test]
    fn open_populates_with_sections() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();
        let text: String = ed.buffers[idx].rope().chars().collect();
        assert!(text.contains("Threads"));
        assert!(text.contains("Call Stack"));
        assert!(text.contains("main"));
        assert!(text.contains("Locals"));
        assert!(text.contains("x: i32 = 42"));
    }

    #[test]
    fn open_populates_line_map() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();
        let view = ed.buffers[idx].debug_view().unwrap();
        // Should have section headers, threads, frames, variables, blanks.
        assert!(!view.line_map.is_empty());
        // Check specific items exist.
        assert!(view
            .line_map
            .iter()
            .any(|item| matches!(item, crate::debug_view::DebugLineItem::Thread(1))));
        assert!(view
            .line_map
            .iter()
            .any(|item| matches!(item, crate::debug_view::DebugLineItem::Frame(100))));
        assert!(view.line_map.iter().any(|item| matches!(
            item,
            crate::debug_view::DebugLineItem::Variable { ref name, .. } if name == "x"
        )));
    }

    #[test]
    fn toggle_opens_and_closes() {
        let mut ed = ed_with_debug_state();
        assert!(!ed.buffers.iter().any(|b| b.kind == BufferKind::Debug));

        ed.toggle_debug_panel();
        assert!(ed.buffers.iter().any(|b| b.kind == BufferKind::Debug));

        ed.toggle_debug_panel();
        assert!(!ed.buffers.iter().any(|b| b.kind == BufferKind::Debug));
    }

    #[test]
    fn close_removes_buffer() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        assert!(ed.buffers.iter().any(|b| b.kind == BufferKind::Debug));
        ed.close_debug_panel();
        assert!(!ed.buffers.iter().any(|b| b.kind == BufferKind::Debug));
    }

    #[test]
    fn no_session_shows_message() {
        let mut ed = Editor::new();
        ed.open_debug_panel();
        let idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();
        let text: String = ed.buffers[idx].rope().chars().collect();
        assert!(text.contains("No active debug session"));
    }

    #[test]
    fn select_frame_updates_view() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let debug_idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();

        // Move cursor to a frame line.
        let frame_line = ed.buffers[debug_idx]
            .debug_view()
            .unwrap()
            .line_map
            .iter()
            .position(|item| matches!(item, crate::debug_view::DebugLineItem::Frame(100)))
            .unwrap();
        ed.buffers[debug_idx].debug_view_mut().unwrap().cursor_index = frame_line;

        ed.debug_panel_select();

        let _view = ed
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Debug)
            .and_then(|b| b.debug_view());
        // Frame may have been selected (scopes request queued).
        assert!(ed.pending_dap_intents.iter().any(|i| matches!(
            i,
            crate::dap_intent::DapIntent::RequestScopes { frame_id: 100 }
        )));
    }

    #[test]
    fn expand_variable_toggles_and_queues() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let debug_idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();

        // Find the expandable variable (editor, var_ref=50).
        let var_line = ed.buffers[debug_idx]
            .debug_view()
            .unwrap()
            .line_map
            .iter()
            .position(|item| {
                matches!(
                    item,
                    crate::debug_view::DebugLineItem::Variable { ref name, .. } if name == "editor"
                )
            })
            .unwrap();
        ed.buffers[debug_idx].debug_view_mut().unwrap().cursor_index = var_line;

        ed.debug_panel_select();

        // Should have expanded and queued a variables request.
        let view: &crate::debug_view::DebugView = ed
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Debug)
            .and_then(|b| b.debug_view())
            .unwrap();
        assert!(view.is_expanded(50));
        assert!(ed.pending_dap_intents.iter().any(|i| matches!(
            i,
            crate::dap_intent::DapIntent::RequestVariables {
                variables_reference: 50,
                ..
            }
        )));
    }

    #[test]
    fn toggle_output_view() {
        let mut ed = ed_with_debug_state();
        ed.debug_state.as_mut().unwrap().log("hello world");
        ed.open_debug_panel();

        let debug_idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();

        // Initially in state view.
        let text: String = ed.buffers[debug_idx].rope().chars().collect();
        assert!(text.contains("Threads"));

        // Toggle to output.
        ed.debug_toggle_output();
        let text: String = ed.buffers[debug_idx].rope().chars().collect();
        assert!(text.contains("Debug Output"));
        assert!(text.contains("hello world"));

        // Toggle back.
        ed.debug_toggle_output();
        let text: String = ed.buffers[debug_idx].rope().chars().collect();
        assert!(text.contains("Threads"));
    }

    #[test]
    fn refresh_if_open_updates_content() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();

        // Add a new thread to state.
        ed.debug_state.as_mut().unwrap().threads.push(DebugThread {
            id: 3,
            name: "new-thread".into(),
            stopped: true,
        });

        ed.debug_panel_refresh_if_open();

        let debug_idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();
        let text: String = ed.buffers[debug_idx].rope().chars().collect();
        assert!(text.contains("new-thread"));
    }

    #[test]
    fn store_children() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();

        let children = vec![Variable {
            name: "mode".into(),
            value: "Normal".into(),
            var_type: Some("Mode".into()),
            variables_reference: 0,
        }];
        ed.debug_panel_store_children(50, children);

        let view = ed
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Debug)
            .and_then(|b| b.debug_view())
            .unwrap();
        assert_eq!(view.child_variables.get(&50).unwrap().len(), 1);
    }

    #[test]
    fn expandable_variable_shows_marker() {
        let mut ed = ed_with_debug_state();
        ed.open_debug_panel();
        let debug_idx = ed
            .buffers
            .iter()
            .position(|b| b.kind == BufferKind::Debug)
            .unwrap();
        let text: String = ed.buffers[debug_idx].rope().chars().collect();
        // The "editor" variable (var_ref=50) should have ▶ marker.
        assert!(text.contains("▶ editor"));
        // The "x" variable (var_ref=0) should NOT have ▶.
        assert!(text.contains("  x: i32 = 42"));
    }
}
