use std::path::Path;

use crate::buffer::Buffer;
use crate::debug::{DebugState, DebugTarget, Scope, StackFrame, Variable};
use crate::theme::{bundled_theme_names, BundledResolver, Theme};

use super::Editor;

impl Editor {
    pub(crate) fn save_current_buffer(&mut self) {
        self.fire_hook("before-save");
        let idx = self.active_buffer_idx();
        match self.buffers[idx].save() {
            Ok(()) => {
                let name = self.buffers[idx].name.clone();
                self.set_status(format!("\"{}\" written", name));
                // Notify any running LSP server that the file was saved.
                self.lsp_notify_did_save();
                self.fire_hook("after-save");
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
    /// The rope is also synced so standard vim operations (yank, visual, search) work.
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
        self.sync_messages_rope();
        // Scroll to bottom so newest entries are visible.
        // scroll_offset = first visible entry index.
        let total = self.message_log.len();
        let vh = self.viewport_height;
        self.window_mgr.focused_window_mut().scroll_offset = total.saturating_sub(vh);
        // Also position cursor at the last line so yank etc. work from the bottom.
        let buf_idx = self.active_buffer_idx();
        let last_line = self.buffers[buf_idx].line_count().saturating_sub(1);
        self.window_mgr.focused_window_mut().cursor_row = last_line;
        self.window_mgr.focused_window_mut().cursor_col = 0;
        self.set_status(format!("{} log entries", total));
    }

    /// Sync message_log entries into the *Messages* buffer's rope.
    /// This enables standard vim operations (yank, visual select, search)
    /// on the messages content while the renderer still uses message_log
    /// directly for styled output.
    pub fn sync_messages_rope(&mut self) {
        let buf_idx = match self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Messages)
        {
            Some(idx) => idx,
            None => return,
        };

        let entries = self.message_log.entries();
        let text: String = entries
            .iter()
            .map(|e| format!("[{}] [{}] {}", e.level, e.target, e.message))
            .collect::<Vec<_>>()
            .join("\n");

        // Temporarily clear read_only to allow rope replacement.
        self.buffers[buf_idx].read_only = false;
        self.buffers[buf_idx].replace_contents(&text);
        self.buffers[buf_idx].read_only = true;
    }

    /// Save the message log to an XDG-compliant path.
    /// Called on editor exit when messages exist.
    /// Path: `$XDG_DATA_HOME/mae/messages/` (default: `~/.local/share/mae/messages/`)
    pub fn save_message_log(&self) -> Result<std::path::PathBuf, String> {
        let entries = self.message_log.entries();
        if entries.is_empty() {
            return Err("No messages to save".into());
        }

        let base = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|_| {
                std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })
            .map_err(|_| "Cannot determine data directory")?;

        let dir = base.join("mae").join("messages");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create {}: {}", dir.display(), e))?;

        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let path = dir.join(format!("messages-{}.log", epoch));

        let mut content = String::new();
        for e in &entries {
            content.push_str(&format!(
                "[{}] [{}] {}: {}\n",
                e.seq, e.level, e.target, e.message
            ));
        }

        std::fs::write(&path, &content)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;

        Ok(path)
    }

    /// Open (or focus) the *AI* conversation buffer and enter ConversationInput mode.
    pub fn open_conversation_buffer(&mut self) {
        let idx = self.ensure_conversation_buffer_idx();
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.set_mode(crate::Mode::ConversationInput);
    }

    /// Persist the AI conversation to a JSON file (`:ai-save <path>`).
    /// Errors if no conversation buffer exists yet — the user hasn't
    /// started an AI session, so there's nothing to save.
    pub fn ai_save(&self, path: &Path) -> Result<usize, String> {
        let conv = self
            .conversation()
            .ok_or_else(|| "No conversation buffer to save".to_string())?;
        let json = conv.to_json()?;
        std::fs::write(path, &json)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        Ok(conv.entries.len())
    }

    /// Load a JSON conversation transcript (`:ai-load <path>`). Creates
    /// the conversation buffer if it doesn't exist yet; replaces the
    /// existing entries otherwise. Unlike `open_conversation_buffer`,
    /// loading is an I/O operation, not a UX one — we don't switch focus
    /// or change mode.
    pub fn ai_load(&mut self, path: &Path) -> Result<usize, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let idx = self.ensure_conversation_buffer_idx();
        // Conversation buffers always carry a Conversation (invariant of
        // `Buffer::new_conversation`), so unwrap here is sound.
        let conv = self.buffers[idx]
            .conversation
            .as_mut()
            .expect("conversation buffer missing its Conversation");
        conv.load_json(&contents)?;
        Ok(conv.entries.len())
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
        state.scopes.push(Scope {
            name: "Performance".into(),
            variables_reference: 5,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Lock State".into(),
            variables_reference: 6,
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

        // Performance variables
        state.variables.insert(
            "Performance".into(),
            vec![
                Variable {
                    name: "frame_time_us".into(),
                    value: format!("{}", self.perf_stats.frame_time_us),
                    var_type: Some("u64".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "avg_frame_time_us".into(),
                    value: format!("{}", self.perf_stats.avg_frame_time_us),
                    var_type: Some("u64".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "fps".into(),
                    value: format!("{:.1}", self.perf_stats.fps()),
                    var_type: Some("f64".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "rss_bytes".into(),
                    value: format!("{}", self.perf_stats.rss_bytes),
                    var_type: Some("u64".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "cpu_percent".into(),
                    value: format!("{:.1}", self.perf_stats.cpu_percent),
                    var_type: Some("f32".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "stall_count".into(),
                    value: format!("{}", self.perf_stats.stall_count),
                    var_type: Some("u64".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "jank_count".into(),
                    value: format!("{}", self.perf_stats.jank_count),
                    var_type: Some("u64".into()),
                    variables_reference: 0,
                },
            ],
        );

        // Lock State variables from global contention tracker
        let lock_snapshot = crate::lock_stats::snapshot();
        let lock_vars: Vec<Variable> = lock_snapshot
            .iter()
            .map(|(name, entry)| Variable {
                name: name.clone(),
                value: format!(
                    "acq={} total_wait={}us max_wait={}us held={}",
                    entry.acquisitions,
                    entry.total_wait_us,
                    entry.max_wait_us,
                    entry.currently_held,
                ),
                var_type: Some("LockEntry".into()),
                variables_reference: 0,
            })
            .collect();
        state.variables.insert(
            "Lock State".into(),
            if lock_vars.is_empty() {
                vec![Variable {
                    name: "(none)".into(),
                    value: "No lock sites instrumented yet".into(),
                    var_type: Some("info".into()),
                    variables_reference: 0,
                }]
            } else {
                lock_vars
            },
        );

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

    /// Recall previous command from history (Up arrow / C-p in command mode).
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
        self.command_cursor = self.command_line.len(); // end of recalled line
    }

    /// Recall next command from history (Down arrow / C-n in command mode).
    pub fn command_history_next(&mut self) {
        let idx = match self.command_history_idx {
            Some(i) => i + 1,
            None => return,
        };
        if idx >= self.command_history.len() {
            self.command_history_idx = None;
            self.command_line.clear();
            self.command_cursor = 0;
        } else {
            self.command_history_idx = Some(idx);
            self.command_line = self.command_history[idx].clone();
            self.command_cursor = self.command_line.len();
        }
    }

    // --- Readline-style command-line editing ---
    //
    // `command_cursor` is a *byte offset* into `command_line`. All mutating
    // helpers keep it on a valid UTF-8 char boundary.

    /// Insert `ch` at the current cursor position and advance the cursor.
    pub fn cmdline_insert_char(&mut self, ch: char) {
        let pos = self.command_cursor.min(self.command_line.len());
        self.command_line.insert(pos, ch);
        self.command_cursor = pos + ch.len_utf8();
        self.command_history_idx = None;
        self.tab_completions.clear();
    }

    /// Delete the char immediately before the cursor (Backspace / C-h).
    pub fn cmdline_backspace(&mut self) {
        if self.command_cursor == 0 {
            return;
        }
        // Walk back to the previous char boundary.
        let mut pos = self.command_cursor;
        loop {
            pos -= 1;
            if self.command_line.is_char_boundary(pos) {
                break;
            }
        }
        self.command_line.remove(pos);
        self.command_cursor = pos;
        self.command_history_idx = None;
        self.tab_completions.clear();
    }

    /// Delete the char at the cursor (C-d / DEL).
    pub fn cmdline_delete_forward(&mut self) {
        if self.command_cursor >= self.command_line.len() {
            return;
        }
        self.command_line.remove(self.command_cursor);
        self.tab_completions.clear();
    }

    /// Move cursor to beginning of line (C-a / Home).
    pub fn cmdline_move_home(&mut self) {
        self.command_cursor = 0;
    }

    /// Move cursor to end of line (C-e / End).
    pub fn cmdline_move_end(&mut self) {
        self.command_cursor = self.command_line.len();
    }

    /// Move cursor one character backward (C-b / Left).
    pub fn cmdline_move_backward(&mut self) {
        if self.command_cursor == 0 {
            return;
        }
        let mut pos = self.command_cursor;
        loop {
            pos -= 1;
            if self.command_line.is_char_boundary(pos) {
                break;
            }
        }
        self.command_cursor = pos;
    }

    /// Move cursor one character forward (C-f / Right).
    pub fn cmdline_move_forward(&mut self) {
        if self.command_cursor >= self.command_line.len() {
            return;
        }
        let ch = self.command_line[self.command_cursor..]
            .chars()
            .next()
            .unwrap();
        self.command_cursor += ch.len_utf8();
    }

    /// Delete backward to the previous whitespace token boundary (C-w).
    pub fn cmdline_delete_word_backward(&mut self) {
        if self.command_cursor == 0 {
            return;
        }
        let s = &self.command_line[..self.command_cursor];
        // Strip trailing whitespace, then strip the word.
        let trimmed = s.trim_end_matches(|c: char| c.is_whitespace());
        let word_start = trimmed
            .rfind(|c: char| c.is_whitespace())
            .map(|i| i + 1) // byte after the space
            .unwrap_or(0);
        self.command_line.drain(word_start..self.command_cursor);
        self.command_cursor = word_start;
        self.tab_completions.clear();
    }

    /// Delete from cursor to beginning of line (C-u).
    pub fn cmdline_kill_to_start(&mut self) {
        self.command_line.drain(..self.command_cursor);
        self.command_cursor = 0;
        self.tab_completions.clear();
    }

    /// Delete from cursor to end of line (C-k).
    pub fn cmdline_kill_to_end(&mut self) {
        self.command_line.truncate(self.command_cursor);
        self.tab_completions.clear();
    }

    /// Compute tab completions for the current command line content.
    /// Returns candidates for command names (no space yet) or arguments.
    pub fn cmdline_completions(&self) -> Vec<String> {
        let line = &self.command_line;
        if let Some(space_pos) = line.find(' ') {
            // After a space: complete arguments for known commands.
            let cmd = &line[..space_pos];
            let arg_prefix = &line[space_pos + 1..];
            self.complete_command_arg(cmd, arg_prefix)
        } else {
            // No space: complete command names.
            self.complete_command_name(line)
        }
    }

    fn complete_command_name(&self, prefix: &str) -> Vec<String> {
        use std::collections::HashSet;
        // Built-in ex commands
        let ex_cmds = [
            "w",
            "q",
            "q!",
            "wq",
            "x",
            "e",
            "vsplit",
            "split",
            "close",
            "messages",
            "help",
            "diagnostics",
            "changes",
            "registers",
            "eval",
            "ai",
            "ai-status",
        ];
        let mut seen = HashSet::new();
        let mut matches: Vec<String> = ex_cmds
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| {
                seen.insert(c.to_string());
                c.to_string()
            })
            .collect();
        // Registered commands
        for name in self.commands.list_names() {
            let name_s = name.to_string();
            if name.starts_with(prefix) && seen.insert(name_s.clone()) {
                matches.push(name_s);
            }
        }
        matches.sort();
        matches
    }

    fn complete_command_arg(&self, cmd: &str, prefix: &str) -> Vec<String> {
        match cmd {
            "e" => crate::file_picker::complete_path(prefix),
            "help" | "describe-command" => {
                let mut matches: Vec<String> = self
                    .commands
                    .list_names()
                    .into_iter()
                    .filter(|n| n.starts_with(prefix))
                    .map(|n| n.to_string())
                    .collect();
                matches.sort();
                matches
            }
            "set-theme" | "theme" => bundled_theme_names()
                .into_iter()
                .filter(|n| n.starts_with(prefix))
                .collect(),
            "set-splash-art" => ["bat"]
                .iter()
                .filter(|n| n.starts_with(prefix))
                .map(|n| n.to_string())
                .collect(),
            "set" => {
                let mut matches: Vec<String> = self
                    .option_registry
                    .all_names()
                    .into_iter()
                    .filter(|n| n.starts_with(prefix))
                    .collect();
                matches.sort();
                matches
            }
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn cmdline_text(&self) -> &str {
        &self.command_line
    }

    /// Check if a buffer's backing file changed on disk and reload if clean,
    /// or warn if the buffer has unsaved modifications.
    pub fn check_and_reload_buffer(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        if !self.buffers[idx].check_disk_changed() {
            return;
        }
        let name = self.buffers[idx].name.clone();
        if self.buffers[idx].modified {
            self.set_status(format!(
                "Warning: {} changed on disk (buffer has unsaved changes)",
                name
            ));
        } else {
            match self.buffers[idx].reload_from_disk() {
                Ok(()) => {
                    self.set_status(format!("Reloaded: {}", name));
                }
                Err(e) => {
                    self.set_status(format!("Reload failed for {}: {}", name, e));
                }
            }
        }
        self.fire_hook("file-changed-on-disk");
    }

    pub fn open_file(&mut self, path: impl AsRef<Path>) {
        if let Some(new_idx) = self.open_file_hidden(path) {
            let prev_idx = self.active_buffer_idx();
            self.alternate_buffer_idx = Some(prev_idx);
            self.window_mgr.focused_window_mut().buffer_idx = new_idx;
        }
    }

    /// Opens a file and returns its buffer index without modifying the window manager focus.
    /// If the file is already open, it just returns that buffer's index.
    pub fn open_file_hidden(&mut self, path: impl AsRef<Path>) -> Option<usize> {
        let path = path.as_ref();

        // Check if file is already open
        if let Ok(canonical) = path.canonicalize() {
            if let Some((idx, _)) = self.buffers.iter().enumerate().find(|(_, b)| {
                b.file_path().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&canonical)
            }) {
                return Some(idx);
            }
        }

        match Buffer::from_file(path) {
            Ok(buf) => {
                let name = buf.name.clone();
                let detected_lang = buf.file_path().and_then(crate::syntax::language_for_path);

                // Track recent files
                if let Some(canonical) = buf.file_path().and_then(|p| p.canonicalize().ok()) {
                    self.recent_files.push(canonical.clone());
                    // Auto-detect project root from the opened file's location
                    if let Some(root) = crate::project::detect_project_root(&canonical) {
                        self.recent_projects.push(root.clone());
                        // Switch project context if it differs from current
                        let should_switch = self
                            .project
                            .as_ref()
                            .map(|p| p.root != root)
                            .unwrap_or(true);
                        if should_switch {
                            self.project = Some(crate::project::Project::from_root(root));
                            self.refresh_git_branch();
                        }
                    }
                    // Ingest project as KB node
                    if let Some(ref proj) = self.project {
                        let config_body = proj
                            .config
                            .as_ref()
                            .map(|c| {
                                format!(
                                    "Workspaces: {:?}\nResources: {:?}",
                                    c.workspaces, c.required_resources
                                )
                            })
                            .unwrap_or_default();
                        self.kb.ingest_project(&proj.name, &proj.root, &config_body);
                    }
                }

                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;

                if let Some(lang) = detected_lang {
                    self.syntax.set_language(new_idx, lang);
                }
                self.set_status(format!("\"{}\" opened", name));
                // Notify any running LSP server that this buffer is open.
                self.lsp_notify_did_open();
                self.fire_hook("buffer-open");
                Some(new_idx)
            }
            Err(e) => {
                self.set_status(format!("Error opening: {}", e));
                None
            }
        }
    }

    /// `gf` — open the filename under the cursor.
    ///
    /// Extracts a filename-like run via [`filename_at_offset`] and tries
    /// to open it.  `~/...` is expanded via `$HOME`. Resolution order:
    ///   1. As-is (absolute path, or relative to cwd).
    ///   2. Relative to the active buffer's parent directory.
    ///
    /// Pushes a jump before opening so `Ctrl-o` returns to the reference.
    pub fn goto_file_under_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        let rope = self.buffers[idx].rope();
        let Some(raw) = filename_at_offset(rope, offset) else {
            self.set_status("gf: no filename under cursor");
            return;
        };

        // Expand ~ to $HOME.
        let expanded = if let Some(rest) = raw.strip_prefix("~/") {
            match std::env::var("HOME") {
                Ok(home) => Path::new(&home).join(rest),
                Err(_) => Path::new(&raw).to_path_buf(),
            }
        } else if raw == "~" {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| Path::new(&raw).to_path_buf())
        } else {
            Path::new(&raw).to_path_buf()
        };

        // Try the literal path first, then relative to the buffer's dir.
        let candidate = if expanded.exists() {
            Some(expanded)
        } else if !expanded.is_absolute() {
            self.buffers[idx]
                .file_path()
                .and_then(|p| p.parent())
                .map(|dir| dir.join(&expanded))
                .filter(|p| p.exists())
        } else {
            None
        };

        match candidate {
            Some(path) => {
                self.record_jump();
                self.open_file(&path);
            }
            None => self.set_status(format!("gf: file not found: {}", raw)),
        }
    }
}

/// Extract the filename/path-like run containing `pos`. Used by `gf`
/// (go-to-file-under-cursor).
///
/// "Filename chars" here are a superset of vi's `isfname`: alphanumerics,
/// `_`, `-`, `.`, `/`, `~`, `+`, `:`, `@`. Wide enough to catch absolute
/// paths, URL-ish strings, and `mod::path` references, but narrow enough
/// to terminate at whitespace, quotes, and most punctuation so we don't
/// swallow trailing commas or parentheses.
///
/// Uses streaming char iteration (`chars_at`) rather than indexed
/// `rope.char(i)` access to avoid O(log N) per-char cost on long
/// buffers — same tradeoff `word::first_non_blank_col` documents.
///
/// Returns `None` if `pos` is past-EOF or not on a filename char.
pub fn filename_at_offset(rope: &ropey::Rope, pos: usize) -> Option<String> {
    let len = rope.len_chars();
    if len == 0 || pos >= len {
        return None;
    }
    let is_filename_char =
        |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~' | '+' | ':' | '@');
    if !is_filename_char(rope.char(pos)) {
        return None;
    }
    // Backward scan: step the cursor back through chars_at, stopping at
    // the first non-filename char or buffer start.
    let mut start = pos;
    let mut back_iter = rope.chars_at(pos);
    while start > 0 {
        match back_iter.prev() {
            Some(c) if is_filename_char(c) => start -= 1,
            _ => break,
        }
    }
    // Forward scan: step through chars_at(pos + 1) until a non-filename
    // char is found.
    let mut end = pos + 1;
    let mut fwd_iter = rope.chars_at(end);
    while end < len {
        match fwd_iter.next() {
            Some(c) if is_filename_char(c) => end += 1,
            _ => break,
        }
    }
    Some(rope.slice(start..end).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed() -> Editor {
        let mut e = Editor::new();
        // prime command line
        e.command_line = "hello world".to_string();
        e.command_cursor = e.command_line.len();
        e
    }

    #[test]
    fn cmdline_insert_char_at_end() {
        let mut e = Editor::new();
        e.cmdline_insert_char('a');
        e.cmdline_insert_char('b');
        assert_eq!(e.command_line, "ab");
        assert_eq!(e.command_cursor, 2);
    }

    #[test]
    fn cmdline_insert_char_in_middle() {
        let mut e = ed();
        e.command_cursor = 5; // after "hello"
        e.cmdline_insert_char('!');
        assert_eq!(e.command_line, "hello! world");
        assert_eq!(e.command_cursor, 6);
    }

    #[test]
    fn cmdline_backspace_removes_char() {
        let mut e = ed();
        e.cmdline_backspace(); // removes 'd'
        assert_eq!(e.command_line, "hello worl");
        assert_eq!(e.command_cursor, 10);
    }

    #[test]
    fn cmdline_backspace_at_start_is_noop() {
        let mut e = ed();
        e.command_cursor = 0;
        e.cmdline_backspace();
        assert_eq!(e.command_line, "hello world");
    }

    #[test]
    fn cmdline_delete_forward_removes_char_at_cursor() {
        let mut e = ed();
        e.command_cursor = 0;
        e.cmdline_delete_forward(); // removes 'h'
        assert_eq!(e.command_line, "ello world");
    }

    #[test]
    fn cmdline_move_home_end() {
        let mut e = ed();
        e.cmdline_move_home();
        assert_eq!(e.command_cursor, 0);
        e.cmdline_move_end();
        assert_eq!(e.command_cursor, 11);
    }

    #[test]
    fn cmdline_move_backward_forward() {
        let mut e = ed();
        e.command_cursor = 5;
        e.cmdline_move_backward();
        assert_eq!(e.command_cursor, 4);
        e.cmdline_move_forward();
        assert_eq!(e.command_cursor, 5);
    }

    #[test]
    fn cmdline_delete_word_backward() {
        let mut e = ed();
        e.cmdline_delete_word_backward(); // deletes "world"
        assert_eq!(e.command_line, "hello ");
        assert_eq!(e.command_cursor, 6);
    }

    #[test]
    fn cmdline_kill_to_start() {
        let mut e = ed();
        e.command_cursor = 5; // after "hello"
        e.cmdline_kill_to_start();
        assert_eq!(e.command_line, " world");
        assert_eq!(e.command_cursor, 0);
    }

    #[test]
    fn cmdline_kill_to_end() {
        let mut e = ed();
        e.command_cursor = 5; // after "hello"
        e.cmdline_kill_to_end();
        assert_eq!(e.command_line, "hello");
        assert_eq!(e.command_cursor, 5);
    }

    #[test]
    fn cmdline_kill_to_end_at_end_is_noop() {
        let mut e = ed();
        e.cmdline_kill_to_end();
        assert_eq!(e.command_line, "hello world");
    }

    #[test]
    fn command_history_prev_sets_cursor_to_end() {
        let mut e = Editor::new();
        e.push_command_history("first");
        e.command_history_prev();
        assert_eq!(e.command_line, "first");
        assert_eq!(e.command_cursor, 5);
    }

    #[test]
    fn command_history_next_clears_cursor() {
        let mut e = Editor::new();
        e.push_command_history("first");
        e.command_history_prev();
        e.command_history_next();
        assert_eq!(e.command_line, "");
        assert_eq!(e.command_cursor, 0);
    }

    #[test]
    fn filename_at_offset_extracts_simple_word() {
        let rope = ropey::Rope::from_str("see main.rs for details");
        assert_eq!(filename_at_offset(&rope, 4).as_deref(), Some("main.rs"));
    }

    #[test]
    fn filename_at_offset_extracts_absolute_path() {
        let rope = ropey::Rope::from_str("open /usr/local/bin/foo now");
        assert_eq!(
            filename_at_offset(&rope, 8).as_deref(),
            Some("/usr/local/bin/foo")
        );
    }

    #[test]
    fn filename_at_offset_returns_none_on_whitespace() {
        let rope = ropey::Rope::from_str("a b");
        assert_eq!(filename_at_offset(&rope, 1), None);
    }

    #[test]
    fn filename_at_offset_returns_none_past_eof() {
        let rope = ropey::Rope::from_str("abc");
        assert_eq!(filename_at_offset(&rope, 3), None);
        assert_eq!(filename_at_offset(&rope, 100), None);
    }

    #[test]
    fn filename_at_offset_stops_at_quotes_and_parens() {
        let rope = ropey::Rope::from_str("include(\"foo/bar.h\")");
        // Offset inside "foo/bar.h" — should not include the quote.
        let offset = "include(\"".len() + 2; // inside "foo"
        assert_eq!(
            filename_at_offset(&rope, offset).as_deref(),
            Some("foo/bar.h")
        );
    }
}
