//! Editor -> Scheme state injection: `inject_editor_state` reads the
//! current `Editor` and defines/registers the Scheme globals and closures
//! that expose it (called before eval). This file covers scalar globals,
//! buffer introspection, selection/region, and the final SharedState
//! snapshot; `state_sync_inject_kb.rs` covers buffer-list/command/keymap
//! introspection, daemon capability parity, and sync/CRDT accessors.
//!
//! Split out of `runtime.rs` (CLAUDE.md architecture debt reduction pass)
//! -- pure code motion, no behavior change. See `state_sync_apply.rs` for
//! the companion `apply_to_editor` (Scheme -> Editor direction).

use mae_core::Editor;

use crate::ffi::arg_int;
use crate::lisp_error::Arity;
use crate::value::Value;

use super::SchemeRuntime;

impl SchemeRuntime {
    pub fn inject_editor_state(&mut self, editor: &Editor) {
        let (text, mode_str) = self.inject_scalar_globals(editor);
        self.inject_buffer_introspection_fns(editor);
        self.inject_selection_region_fns(editor);
        self.inject_buffer_list_and_command_fns(editor);
        self.inject_daemon_capability_fns(editor);
        self.inject_sync_crdt_fns(editor);
        self.inject_graph_view_state(editor);
        self.inject_shared_state_snapshot(editor, text, mode_str);
    }

    /// Scalar globals + buffer text/count/mode/language/file-path + shell
    /// buffer accessors + `*current-command*`. Returns `(buffer_text,
    /// mode_str)` since both are reused by `inject_shared_state_snapshot`
    /// and recomputing `buffer_text` would re-clone the whole buffer.
    fn inject_scalar_globals(&mut self, editor: &Editor) -> (String, &'static str) {
        // Keep the daemon control channel current so `(kb-share-p2p)` drives the
        // live backend (cheap Arc clone; None when no daemon is wired).
        self.shared.lock().daemon_control = editor.kb.daemon_control();

        let buf = editor.active_buffer();
        let win = editor.window_mgr.focused_window();

        // Scalar state
        self.vm
            .define_global("*buffer-name*", Value::string(buf.name.clone()));
        self.vm
            .define_global("*buffer-modified?*", Value::Bool(buf.modified));
        self.vm
            .define_global("*buffer-line-count*", Value::Int(buf.line_count() as i64));
        self.vm
            .define_global("*cursor-row*", Value::Int(win.cursor_row as i64));
        self.vm
            .define_global("*cursor-col*", Value::Int(win.cursor_col as i64));

        // Full buffer text
        let text = buf.text();
        self.vm
            .define_global("*buffer-text*", Value::string(text.clone()));

        // Number of open buffers
        self.vm
            .define_global("*buffer-count*", Value::Int(editor.buffers.len() as i64));

        // Current mode
        let mode_str = match editor.mode {
            mae_core::Mode::Normal => "normal",
            mae_core::Mode::Insert => "insert",
            mae_core::Mode::Visual(_) => "visual",
            mae_core::Mode::Command => "command",
            mae_core::Mode::ConversationInput => "conversation",
            mae_core::Mode::Search => "search",
            mae_core::Mode::FilePicker => "file-picker",
            mae_core::Mode::FileBrowser => "file-browser",
            mae_core::Mode::CommandPalette => "command-palette",
            mae_core::Mode::ShellInsert => "shell-insert",
        };
        self.vm.define_global("*mode*", Value::string(mode_str));

        // *buffer-language*
        let active_idx = editor.active_buffer_idx();
        let lang_str = editor
            .syntax
            .language_for(active_idx)
            .map(|l| l.id())
            .unwrap_or("text");
        self.vm
            .define_global("*buffer-language*", Value::string(lang_str));

        // *buffer-file-path*
        let file_path_str = buf
            .file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.vm
            .define_global("*buffer-file-path*", Value::string(file_path_str));

        // (buffer-line N)
        let lines: Vec<String> = (0..buf.line_count())
            .map(|i| buf.line_text(i).to_string())
            .collect();
        let lines = std::sync::Arc::new(lines);
        self.vm.register_fn(
            "buffer-line",
            "Read a specific line (0-indexed)",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let n = arg_int(args, 0, "buffer-line")?;
                Ok(Value::string(
                    lines.get(n.max(0) as usize).cloned().unwrap_or_default(),
                ))
            },
        );

        // *shell-buffers*
        let shell_indices: Vec<Value> = editor
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == mae_core::BufferKind::Shell)
            .map(|(i, _)| Value::Int(i as i64))
            .collect();
        self.vm
            .define_global("*shell-buffers*", Value::list(shell_indices));

        // (shell-cwd BUF-IDX)
        let cwds = editor.shell.viewport_cwds.clone();
        self.vm.register_fn(
            "shell-cwd",
            "Return cached CWD for a shell buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let idx = arg_int(args, 0, "shell-cwd")?;
                Ok(Value::string(
                    cwds.get(&(idx.max(0) as usize))
                        .cloned()
                        .unwrap_or_default(),
                ))
            },
        );

        // (shell-read-output BUF-IDX MAX-LINES)
        let viewports = editor.shell.viewports.clone();
        self.vm.register_fn(
            "shell-read-output",
            "Read viewport snapshot",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let idx = arg_int(args, 0, "shell-read-output")?.max(0) as usize;
                let max = arg_int(args, 1, "shell-read-output")?.max(1) as usize;
                Ok(Value::string(
                    viewports
                        .get(&idx)
                        .map(|lines| {
                            let start = lines.len().saturating_sub(max);
                            lines[start..].join("\n")
                        })
                        .unwrap_or_default(),
                ))
            },
        );

        // *current-command*
        self.vm.define_global(
            "*current-command*",
            Value::string(editor.current_command.clone()),
        );

        (text, mode_str)
    }

    /// Buffer introspection functions (current-buffer-name, point, line
    /// bounds, etc.).
    fn inject_buffer_introspection_fns(&mut self, editor: &Editor) {
        let buf = editor.active_buffer();
        let win = editor.window_mgr.focused_window();

        // --- Buffer introspection functions ---

        let buf_name = buf.name.clone();
        self.vm.register_fn(
            "current-buffer-name",
            "Name of current buffer",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(buf_name.clone())),
        );

        let file_path = buf.file_path().map(|p| p.display().to_string());
        self.vm.register_fn(
            "current-buffer-file",
            "File path of current buffer",
            Arity::Fixed(0),
            move |_args: &[Value]| match &file_path {
                Some(p) => Ok(Value::string(p.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let line_num = (win.cursor_row + 1) as i64;
        self.vm.register_fn(
            "current-line-number",
            "Current line number (1-indexed)",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(line_num)),
        );

        let col = win.cursor_col as i64;
        self.vm.register_fn(
            "current-column",
            "Current column",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(col)),
        );

        let cursor_offset = buf.char_offset_at(win.cursor_row, win.cursor_col) as i64;
        self.vm.register_fn(
            "point",
            "Cursor character offset",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(cursor_offset)),
        );

        self.vm.register_fn(
            "point-min",
            "Minimum point",
            Arity::Fixed(0),
            |_args: &[Value]| Ok(Value::Int(0)),
        );

        let max_chars = buf.rope().len_chars() as i64;
        self.vm.register_fn(
            "point-max",
            "Maximum point",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(max_chars)),
        );

        let clamped_row = win.cursor_row.min(buf.line_count().saturating_sub(1));
        let line_begin = buf.rope().line_to_char(clamped_row) as i64;
        self.vm.register_fn(
            "line-beginning-position",
            "Start of current line",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(line_begin)),
        );

        let line_end = if clamped_row + 1 < buf.line_count() {
            buf.rope().line_to_char(clamped_row + 1) as i64 - 1
        } else {
            buf.rope().len_chars() as i64
        };
        self.vm.register_fn(
            "line-end-position",
            "End of current line",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(line_end)),
        );
    }

    /// Selection/region primitives (region-active?, get-selection,
    /// buffer-text-range, etc.).
    fn inject_selection_region_fns(&mut self, editor: &Editor) {
        let buf = editor.active_buffer();
        let win = editor.window_mgr.focused_window();

        // --- Selection / region ---

        // --- Selection / region --- reads from SharedState for always-fresh data
        let s = self.shared.clone();
        self.vm.register_fn(
            "region-active?",
            "Whether visual selection is active",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().region_active)),
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "region-beginning",
            "Start of region",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().region_start as i64)),
        );
        let s = self.shared.clone();
        self.vm.register_fn(
            "region-end",
            "End of region",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().region_end as i64)),
        );

        let is_visual = matches!(editor.mode, mae_core::Mode::Visual(_));
        let selection_text = if is_visual {
            let anchor_offset =
                buf.char_offset_at(editor.vi.visual_anchor_row, editor.vi.visual_anchor_col);
            let cursor_off = buf.char_offset_at(win.cursor_row, win.cursor_col);
            let beg = anchor_offset.min(cursor_off);
            let end = anchor_offset.max(cursor_off) + 1;
            let end = end.min(buf.rope().len_chars());
            buf.rope().chars().skip(beg).take(end - beg).collect()
        } else {
            String::new()
        };
        let st = selection_text;
        self.vm.register_fn(
            "get-selection",
            "Get selected text",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(st.clone())),
        );

        // *buffer-char-count*
        self.vm.define_global(
            "*buffer-char-count*",
            Value::Int(buf.rope().len_chars() as i64),
        );

        // (buffer-text-range START END)
        let text_for_range = buf.text();
        self.vm.register_fn(
            "buffer-text-range",
            "Substring of buffer text",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let start = arg_int(args, 0, "buffer-text-range")?.max(0) as usize;
                let end = arg_int(args, 1, "buffer-text-range")?.max(0) as usize;
                Ok(Value::string(
                    text_for_range
                        .chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect::<String>(),
                ))
            },
        );
    }

    /// Snapshot the freshly-computed editor state into `SharedState` so
    /// SharedState-backed functions (buffer-string, region-active?,
    /// get-buffer-by-name, etc.) return fresh data on the next call.
    fn inject_shared_state_snapshot(&mut self, editor: &Editor, text: String, mode_str: &str) {
        let buf = editor.active_buffer();
        let win = editor.window_mgr.focused_window();
        let sync_enabled = buf.sync_doc.is_some();

        // Update SharedState so SharedState-backed functions (buffer-string,
        // region-active?, get-buffer-by-name, etc.) return fresh data.
        {
            let mut state = self.shared.lock();
            state.current_buffer_text = text;
            state.current_mode = mode_str.to_string();
            state.leader_active = editor.leader_active;
            state.which_key_count = editor.which_key_entries_for_current_keymap().len();
            state.cursor_row = win.cursor_row;
            state.cursor_col = win.cursor_col;
            state.last_status_message = editor.status_msg.clone();
            state.buffer_names = editor
                .buffers
                .iter()
                .enumerate()
                .map(|(i, b)| (i, b.name.clone()))
                .collect();
            state.sync_enabled = sync_enabled;
            state.pending_update_count = buf.pending_sync_updates.len();
            state.kb_store = editor.kb.store.clone();
            state.sync_content = buf.sync_doc.as_ref().map(|s| s.content());
            state.encoded_state = buf.sync_doc.as_ref().map(|s| {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(s.encode_state())
            });
            // Region state
            if matches!(editor.mode, mae_core::Mode::Visual(_)) {
                let rope = buf.rope();
                let anchor_offset =
                    buf.char_offset_at(editor.vi.visual_anchor_row, editor.vi.visual_anchor_col);
                let cursor_off = buf.char_offset_at(win.cursor_row, win.cursor_col);
                state.region_active = true;
                state.region_start = anchor_offset.min(cursor_off);
                state.region_end = (anchor_offset.max(cursor_off) + 1).min(rope.len_chars());
            } else {
                state.region_active = false;
                state.region_start = 0;
                state.region_end = 0;
            }
        }
    }
}
