use std::path::Path;

use crate::buffer::Buffer;
use crate::debug::{DebugState, DebugTarget, Scope, StackFrame, Variable};
use crate::theme::{bundled_theme_names, BundledResolver, Theme};

use super::Editor;

impl Editor {
    pub(crate) fn save_current_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        match self.buffers[idx].save() {
            Ok(()) => {
                let name = self.buffers[idx].name.clone();
                self.set_status(format!("\"{}\" written", name));
                // Notify any running LSP server that the file was saved.
                self.lsp_notify_did_save();
            }
            Err(e) => {
                self.set_status(format!("Error saving: {}", e));
            }
        }
    }

    /// Set the editor theme by name. Looks up bundled themes.
    pub fn set_theme_by_name(&mut self, name: &str) {
        match Theme::load(name, &BundledResolver) {
            Ok(theme) => {
                self.set_status(format!("Theme: {}", theme.name));
                self.theme = theme;
            }
            Err(e) => {
                self.set_status(format!("Failed to load theme '{}': {}", name, e));
            }
        }
    }

    /// Cycle to the next bundled theme.
    pub fn cycle_theme(&mut self) {
        let names = bundled_theme_names();
        if names.is_empty() {
            return;
        }
        let current_idx = names
            .iter()
            .position(|n| n == &self.theme.name)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % names.len();
        self.set_theme_by_name(&names[next_idx]);
    }

    /// Open the *Messages* buffer showing the in-editor log.
    /// Uses `BufferKind::Messages` — the renderer reads live from `editor.message_log`.
    /// No rope copy needed; the buffer is just a view marker.
    pub fn open_messages_buffer(&mut self) {
        let existing_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Messages);

        if let Some(idx) = existing_idx {
            self.window_mgr.focused_window_mut().buffer_idx = idx;
        } else {
            self.buffers.push(Buffer::new_messages());
            let new_idx = self.buffers.len() - 1;
            self.window_mgr.focused_window_mut().buffer_idx = new_idx;
        }
        let count = self.message_log.len();
        self.set_status(format!("{} log entries", count));
    }

    /// Open (or focus) the *AI* conversation buffer and enter ConversationInput mode.
    pub fn open_conversation_buffer(&mut self) {
        let conv_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Conversation);
        let idx = if let Some(i) = conv_idx {
            i
        } else {
            self.buffers.push(Buffer::new_conversation("*AI*"));
            self.buffers.len() - 1
        };
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.mode = crate::Mode::ConversationInput;
    }

    /// Start a self-debug session, populating DebugState with the editor's
    /// current Rust state. Scheme state is populated separately when the
    /// binary calls `populate_scheme_debug_state()` (since core doesn't own SchemeRuntime).
    pub fn start_self_debug(&mut self) {
        let mut state = DebugState::new_self_debug();

        // Synthetic stack frame for the Rust core event loop
        state.stack_frames.push(StackFrame {
            id: 1,
            name: format!("event_loop [mode={:?}]", self.mode),
            source: Some("crates/mae/src/main.rs".into()),
            line: 0,
            column: 0,
        });

        // Scopes for Rust Core thread
        state.scopes.push(Scope {
            name: "Editor State".into(),
            variables_reference: 1,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Active Buffer".into(),
            variables_reference: 2,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Active Window".into(),
            variables_reference: 3,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "All Buffers".into(),
            variables_reference: 4,
            expensive: false,
        });

        // Editor State variables
        let buf = self.active_buffer();
        let win = self.window_mgr.focused_window();
        state.variables.insert(
            "Editor State".into(),
            vec![
                Variable {
                    name: "mode".into(),
                    value: format!("{:?}", self.mode),
                    var_type: Some("Mode".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "running".into(),
                    value: format!("{}", self.running),
                    var_type: Some("bool".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "status_msg".into(),
                    value: self.status_msg.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "command_line".into(),
                    value: self.command_line.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "buffer_count".into(),
                    value: format!("{}", self.buffers.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "window_count".into(),
                    value: format!("{}", self.window_mgr.window_count()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "theme".into(),
                    value: self.theme.name.clone(),
                    var_type: Some("Theme".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "command_count".into(),
                    value: format!("{}", self.commands.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "message_log_entries".into(),
                    value: format!("{}", self.message_log.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
            ],
        );

        // Active Buffer variables
        state.variables.insert(
            "Active Buffer".into(),
            vec![
                Variable {
                    name: "name".into(),
                    value: buf.name.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "kind".into(),
                    value: format!("{:?}", buf.kind),
                    var_type: Some("BufferKind".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "modified".into(),
                    value: format!("{}", buf.modified),
                    var_type: Some("bool".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "line_count".into(),
                    value: format!("{}", buf.line_count()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "file_path".into(),
                    value: buf
                        .file_path()
                        .map_or("None".to_string(), |p| p.display().to_string()),
                    var_type: Some("Option<PathBuf>".into()),
                    variables_reference: 0,
                },
            ],
        );

        // Active Window variables
        state.variables.insert(
            "Active Window".into(),
            vec![
                Variable {
                    name: "cursor_row".into(),
                    value: format!("{}", win.cursor_row),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "cursor_col".into(),
                    value: format!("{}", win.cursor_col),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "scroll_offset".into(),
                    value: format!("{}", win.scroll_offset),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "buffer_idx".into(),
                    value: format!("{}", win.buffer_idx),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
            ],
        );

        // All Buffers (expandable summary)
        let all_bufs: Vec<Variable> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| Variable {
                name: format!("[{}]", i),
                value: format!(
                    "{} ({:?}{})",
                    b.name,
                    b.kind,
                    if b.modified { ", modified" } else { "" }
                ),
                var_type: Some("Buffer".into()),
                variables_reference: 0,
            })
            .collect();
        state.variables.insert("All Buffers".into(), all_bufs);

        // Mark as stopped (self-debug is always "stopped" — it's a snapshot)
        state.stopped_location = Some(("crates/mae/src/main.rs".into(), 0));

        self.debug_state = Some(state);
        self.set_status("Self-debug: Rust state captured. Use SPC d v to inspect.");
    }

    /// Refresh the Rust portion of the self-debug state (call on each debug render).
    pub fn refresh_self_debug(&mut self) {
        if let Some(ref state) = self.debug_state {
            if state.target == DebugTarget::SelfDebug {
                // Re-capture by starting fresh
                self.start_self_debug();
            }
        }
    }

    /// Push a command to the command history (skipping consecutive duplicates).
    pub fn push_command_history(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return;
        }
        if self.command_history.last().map(|s| s.as_str()) == Some(cmd) {
            return; // skip consecutive duplicate
        }
        self.command_history.push(cmd.to_string());
        // Bound history to 500 entries
        if self.command_history.len() > 500 {
            self.command_history
                .drain(..self.command_history.len() - 500);
        }
        self.command_history_idx = None;
    }

    /// Recall previous command from history (Up arrow in command mode).
    pub fn command_history_prev(&mut self) {
        if self.command_history.is_empty() {
            return;
        }
        let idx = match self.command_history_idx {
            Some(0) => return, // already at oldest
            Some(i) => i - 1,
            None => self.command_history.len() - 1,
        };
        self.command_history_idx = Some(idx);
        self.command_line = self.command_history[idx].clone();
    }

    /// Recall next command from history (Down arrow in command mode).
    pub fn command_history_next(&mut self) {
        let idx = match self.command_history_idx {
            Some(i) => i + 1,
            None => return,
        };
        if idx >= self.command_history.len() {
            self.command_history_idx = None;
            self.command_line.clear();
        } else {
            self.command_history_idx = Some(idx);
            self.command_line = self.command_history[idx].clone();
        }
    }

    pub fn open_file(&mut self, path: &str) {
        match Buffer::from_file(Path::new(path)) {
            Ok(buf) => {
                let name = buf.name.clone();
                let prev_idx = self.active_buffer_idx();
                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;
                self.alternate_buffer_idx = Some(prev_idx);
                self.window_mgr.focused_window_mut().buffer_idx = new_idx;
                self.set_status(format!("\"{}\" opened", name));
                // Notify any running LSP server that this buffer is open.
                self.lsp_notify_did_open();
            }
            Err(e) => {
                self.set_status(format!("Error opening: {}", e));
            }
        }
    }
}
