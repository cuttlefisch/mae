use std::path::Path;
use std::sync::{Arc, Mutex};

use steel::steel_vm::engine::Engine;
use steel::steel_vm::register_fn::RegisterFn;
use steel::SteelVal;
use tracing::{debug, error, info, warn};

use mae_core::{parse_key_seq_spaced, Editor};

/// Accumulated config data from Scheme evaluation.
/// Shared between Rust and Steel via Arc<Mutex<>>.
///
/// register_fn requires Send + Sync + 'static. Rc<RefCell<>> doesn't
/// satisfy those bounds. Arc<Mutex<>> does, and since Engine is
/// single-threaded (!Send), the mutex is never contended.
#[derive(Default)]
struct SharedState {
    /// (keymap_name, key_string, command_name)
    keymap_bindings: Vec<(String, String, String)>,
    /// (command_name, doc_string, scheme_function_name)
    command_defs: Vec<(String, String, String)>,
    /// Status messages set by Scheme code
    status_message: Option<String>,
    /// Hook registrations: (hook_name, fn_name)
    pending_hook_adds: Vec<(String, String)>,
    /// Hook removals: (hook_name, fn_name)
    pending_hook_removes: Vec<(String, String)>,
    /// Editor option changes: (key, value)
    pending_options: Vec<(String, String)>,
    /// Theme name requested by Scheme code via `(set-theme "name")`
    theme_request: Option<String>,

    // --- Write-side primitives (applied after eval) ---
    /// Text to insert at the cursor via `(buffer-insert TEXT)`.
    pending_insert: Option<String>,
    /// Cursor repositioning via `(cursor-goto ROW COL)`.
    pending_cursor: Option<(usize, usize)>,
    /// File to open via `(open-file PATH)`.
    pending_open_file: Option<String>,
    /// Commands to dispatch via `(run-command NAME)`.
    pending_commands: Vec<String>,
    /// Messages to append to the message log via `(message TEXT)`.
    pending_messages: Vec<String>,
    /// Shell inputs to send: (buffer_index, text).
    pending_shell_inputs: Vec<(usize, String)>,
}

/// A captured Scheme evaluation error for debugger introspection.
#[derive(Debug, Clone)]
pub struct SchemeErrorSnapshot {
    pub expression: String,
    pub error_message: String,
    pub seq: u64,
}

/// Wraps Steel's Engine and provides the Scheme extension API.
///
/// Design: the Engine and Editor live on the same thread. Scheme eval
/// blocks the event loop briefly — acceptable for config loading and
/// interactive REPL. Phase 3 will need a dedicated Scheme thread with
/// channel-based message passing for concurrent AI access.
pub struct SchemeRuntime {
    engine: Engine,
    shared: Arc<Mutex<SharedState>>,
    /// Ring buffer of recent eval errors for debugger introspection.
    error_history: Vec<SchemeErrorSnapshot>,
    /// Monotonic sequence counter for error ordering.
    error_seq: u64,
    /// Maximum errors to retain.
    max_errors: usize,
}

/// Error type for Scheme operations.
#[derive(Debug)]
pub struct SchemeError {
    pub message: String,
}

impl std::fmt::Display for SchemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SchemeError {}

impl From<steel::SteelErr> for SchemeError {
    fn from(err: steel::SteelErr) -> Self {
        SchemeError {
            message: format!("{}", err),
        }
    }
}

impl SchemeRuntime {
    pub fn new() -> Result<Self, SchemeError> {
        let mut engine = Engine::new();
        let shared = Arc::new(Mutex::new(SharedState::default()));

        // Register define-key: (define-key MAP KEY COMMAND)
        let s = shared.clone();
        engine.register_fn(
            "define-key",
            move |map: String, key: String, cmd: String| {
                s.lock().unwrap().keymap_bindings.push((map, key, cmd));
                SteelVal::Void
            },
        );

        // Register define-command: (define-command NAME DOC SCHEME-FN-NAME)
        let s = shared.clone();
        engine.register_fn(
            "define-command",
            move |name: String, doc: String, fn_name: String| {
                s.lock().unwrap().command_defs.push((name, doc, fn_name));
                SteelVal::Void
            },
        );

        // Register set-status: (set-status MSG)
        let s = shared.clone();
        engine.register_fn("set-status", move |msg: String| {
            s.lock().unwrap().status_message = Some(msg);
            SteelVal::Void
        });

        // Register set-theme: (set-theme NAME)
        let s = shared.clone();
        engine.register_fn("set-theme", move |name: String| {
            s.lock().unwrap().theme_request = Some(name);
            SteelVal::Void
        });

        // --- Live editing primitives ---

        // (buffer-insert TEXT) — insert text at the cursor position.
        let s = shared.clone();
        engine.register_fn("buffer-insert", move |text: String| {
            s.lock().unwrap().pending_insert = Some(text);
            SteelVal::Void
        });

        // (cursor-goto ROW COL) — move cursor to absolute position (0-indexed).
        let s = shared.clone();
        engine.register_fn("cursor-goto", move |row: isize, col: isize| {
            s.lock().unwrap().pending_cursor = Some((row.max(0) as usize, col.max(0) as usize));
            SteelVal::Void
        });

        // (open-file PATH) — open a file in a new buffer.
        let s = shared.clone();
        engine.register_fn("open-file", move |path: String| {
            s.lock().unwrap().pending_open_file = Some(path);
            SteelVal::Void
        });

        // (run-command NAME) — dispatch a registered command by name.
        let s = shared.clone();
        engine.register_fn("run-command", move |name: String| {
            s.lock().unwrap().pending_commands.push(name);
            SteelVal::Void
        });

        // (message TEXT) — append to the *Messages* log.
        let s = shared.clone();
        engine.register_fn("message", move |text: String| {
            s.lock().unwrap().pending_messages.push(text);
            SteelVal::Void
        });

        // --- Hook system ---

        // (add-hook! HOOK-NAME FN-NAME)
        let s = shared.clone();
        engine.register_fn("add-hook!", move |hook: String, fn_name: String| {
            s.lock().unwrap().pending_hook_adds.push((hook, fn_name));
            SteelVal::Void
        });

        // (remove-hook! HOOK-NAME FN-NAME)
        let s = shared.clone();
        engine.register_fn("remove-hook!", move |hook: String, fn_name: String| {
            s.lock().unwrap().pending_hook_removes.push((hook, fn_name));
            SteelVal::Void
        });

        // --- Editor options ---

        // (set-option! KEY VALUE)
        let s = shared.clone();
        engine.register_fn("set-option!", move |key: String, value: String| {
            s.lock().unwrap().pending_options.push((key, value));
            SteelVal::Void
        });

        // --- Shell terminal bindings ---

        // (shell-send-input BUF-IDX TEXT) — send text to a terminal PTY
        let s = shared.clone();
        engine.register_fn("shell-send-input", move |buf_idx: isize, text: String| {
            if buf_idx < 0 {
                return SteelVal::Void; // ignore negative indices
            }
            s.lock()
                .unwrap()
                .pending_shell_inputs
                .push((buf_idx as usize, text));
            SteelVal::Void
        });

        Ok(SchemeRuntime {
            engine,
            shared,
            error_history: Vec::new(),
            error_seq: 0,
            max_errors: 50,
        })
    }

    /// Evaluate a Scheme expression and return the result as a string.
    /// Errors are recorded in the error history for debugger introspection.
    pub fn eval(&mut self, code: &str) -> Result<String, SchemeError> {
        debug!(code_len = code.len(), "scheme eval");
        let results = self.engine.run(code.to_string()).map_err(|e| {
            let err = SchemeError::from(e);
            error!(error = %err.message, code_preview = &code[..code.len().min(100)], "scheme eval error");
            // Record error for debugger
            self.error_seq += 1;
            let snapshot = SchemeErrorSnapshot {
                expression: code[..code.len().min(200)].to_string(),
                error_message: err.message.clone(),
                seq: self.error_seq,
            };
            self.error_history.push(snapshot);
            if self.error_history.len() > self.max_errors {
                self.error_history.remove(0);
            }
            err
        })?;
        if results.is_empty() {
            Ok(String::new())
        } else {
            let last = &results[results.len() - 1];
            Ok(steel_val_to_string(last))
        }
    }

    /// Load and evaluate a Scheme file.
    pub fn load_file(&mut self, path: &Path) -> Result<(), SchemeError> {
        info!(path = %path.display(), "loading scheme file");
        let content = std::fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "failed to read scheme file");
            SchemeError {
                message: format!("Failed to read {}: {}", path.display(), e),
            }
        })?;
        self.engine.run(content).map_err(|e| {
            let err = SchemeError::from(e);
            error!(path = %path.display(), error = %err.message, "scheme file evaluation failed");
            err
        })?;
        Ok(())
    }

    /// Inject read-only buffer information as Scheme globals.
    /// Call this before eval to give Scheme access to current editor state.
    pub fn inject_editor_state(&mut self, editor: &Editor) {
        let buf = editor.active_buffer();
        let win = editor.window_mgr.focused_window();

        // Scalar state
        self.engine
            .register_value("*buffer-name*", SteelVal::StringV(buf.name.clone().into()));
        self.engine
            .register_value("*buffer-modified?*", SteelVal::BoolV(buf.modified));
        self.engine.register_value(
            "*buffer-line-count*",
            SteelVal::IntV(buf.line_count() as isize),
        );
        self.engine
            .register_value("*cursor-row*", SteelVal::IntV(win.cursor_row as isize));
        self.engine
            .register_value("*cursor-col*", SteelVal::IntV(win.cursor_col as isize));

        // Full buffer text — accessible as `*buffer-text*`
        let text = buf.text();
        self.engine
            .register_value("*buffer-text*", SteelVal::StringV(text.into()));

        // Number of open buffers
        self.engine.register_value(
            "*buffer-count*",
            SteelVal::IntV(editor.buffers.len() as isize),
        );

        // Current mode as a string
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
        self.engine
            .register_value("*mode*", SteelVal::StringV(mode_str.into()));

        // (buffer-line N) — read a specific line (0-indexed). Capture
        // a snapshot of all lines so the closure is self-contained.
        let lines: Vec<String> = (0..buf.line_count())
            .map(|i| buf.line_text(i).to_string())
            .collect();
        let lines = std::sync::Arc::new(lines);
        self.engine.register_fn("buffer-line", move |n: isize| {
            lines.get(n.max(0) as usize).cloned().unwrap_or_default()
        });
    }

    /// Apply accumulated config changes to the editor.
    /// Call this after loading init.scm or after REPL eval.
    pub fn apply_to_editor(&mut self, editor: &mut Editor) {
        let mut state = self.shared.lock().unwrap();

        // Apply keymap bindings
        let binding_count = state.keymap_bindings.len();
        for (map_name, key_str, cmd_name) in state.keymap_bindings.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&key_str);
                if !seq.is_empty() {
                    debug!(keymap = %map_name, key = %key_str, command = %cmd_name, "applying scheme keybinding");
                    keymap.bind(seq, &cmd_name);
                }
            } else {
                warn!(keymap = %map_name, key = %key_str, command = %cmd_name, "scheme keybinding targets unknown keymap");
            }
        }

        // Register Scheme-defined commands
        let cmd_count = state.command_defs.len();
        for (name, doc, scheme_fn) in state.command_defs.drain(..) {
            debug!(command = %name, scheme_fn = %scheme_fn, "registering scheme command");
            editor.commands.register_scheme(&name, &doc, &scheme_fn);
        }

        // Apply hook registrations
        for (hook, fn_name) in state.pending_hook_adds.drain(..) {
            if editor.hooks.add(&hook, &fn_name) {
                debug!(hook = %hook, fn_name = %fn_name, "hook registered");
            } else {
                warn!(hook = %hook, "unknown hook name in add-hook!");
                editor.set_status(format!("Unknown hook: {}", hook));
            }
        }
        for (hook, fn_name) in state.pending_hook_removes.drain(..) {
            if editor.hooks.remove(&hook, &fn_name) {
                debug!(hook = %hook, fn_name = %fn_name, "hook removed");
            }
        }

        // Apply editor options
        for (key, value) in state.pending_options.drain(..) {
            match key.as_str() {
                "line-numbers" | "show-line-numbers" => {
                    editor.show_line_numbers = parse_bool(&value);
                }
                "relative-line-numbers" => {
                    editor.relative_line_numbers = parse_bool(&value);
                }
                "word-wrap" => {
                    editor.word_wrap = parse_bool(&value);
                }
                "break-indent" => {
                    editor.break_indent = parse_bool(&value);
                }
                "show-break" => {
                    editor.show_break = value;
                }
                "theme" => {
                    editor.set_theme_by_name(&value);
                }
                other => {
                    warn!(key = other, "unknown set-option! key");
                    editor.set_status(format!("Unknown option: {}", other));
                }
            }
        }

        // Apply status message
        if let Some(msg) = state.status_message.take() {
            editor.set_status(msg);
        }

        // Apply theme change
        if let Some(theme_name) = state.theme_request.take() {
            info!(theme = %theme_name, "applying scheme theme request");
            editor.set_theme_by_name(&theme_name);
        }

        // --- Live editing primitives ---

        // (buffer-insert TEXT)
        if let Some(text) = state.pending_insert.take() {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window();
            let offset = editor.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
            editor.buffers[idx].insert_text_at(offset, &text);
            // Advance cursor past inserted text.
            let end = offset + text.chars().count();
            let rope = editor.buffers[idx].rope();
            let new_row = rope.char_to_line(end.min(rope.len_chars()));
            let line_start = rope.line_to_char(new_row);
            let win = editor.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = end.saturating_sub(line_start);
        }

        // (cursor-goto ROW COL)
        if let Some((row, col)) = state.pending_cursor.take() {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
            win.clamp_cursor(&editor.buffers[idx]);
        }

        // (open-file PATH)
        if let Some(path) = state.pending_open_file.take() {
            editor.open_file(&path);
        }

        // (run-command NAME) — dispatch each queued command.
        // We drain them outside the lock since dispatch_builtin
        // may re-enter shared state.
        let commands: Vec<String> = state.pending_commands.drain(..).collect();

        // (message TEXT) — append to message log
        for msg in state.pending_messages.drain(..) {
            info!("[scheme] {}", msg);
        }

        // (shell-send-input BUF-IDX TEXT) — queue shell terminal input.
        for (buf_idx, text) in state.pending_shell_inputs.drain(..) {
            editor.pending_shell_inputs.push((buf_idx, text));
        }

        // Drop the lock before dispatching commands (which may call
        // back into Scheme via user-defined commands).
        drop(state);

        for name in commands {
            editor.dispatch_builtin(&name);
        }

        if binding_count > 0 || cmd_count > 0 {
            info!(
                keybindings = binding_count,
                commands = cmd_count,
                "scheme config applied to editor"
            );
        }
    }

    /// Call a named Scheme function (for executing Scheme-backed commands).
    pub fn call_function(&mut self, name: &str) -> Result<String, SchemeError> {
        let code = format!("({})", name);
        self.eval(&code)
    }

    // --- Debugger introspection methods ---

    /// List all Scheme-defined commands accumulated via `(define-command ...)`.
    /// Returns (name, doc, scheme_fn_name) triples.
    pub fn list_user_commands(&self) -> Vec<(String, String, String)> {
        self.shared.lock().unwrap().command_defs.clone()
    }

    /// List all keybindings accumulated via `(define-key ...)`.
    /// Returns (keymap_name, key_string, command_name) triples.
    pub fn list_keybindings(&self) -> Vec<(String, String, String)> {
        self.shared.lock().unwrap().keymap_bindings.clone()
    }

    /// Return recent eval errors for debugger display.
    pub fn last_errors(&self) -> Vec<SchemeErrorSnapshot> {
        self.error_history.clone()
    }

    /// Evaluate a Scheme expression for the debugger.
    /// Same as `eval` but intended for debugger inspect/watch expressions.
    pub fn eval_for_debug(&mut self, expr: &str) -> Result<String, SchemeError> {
        self.eval(expr)
    }

    /// Set a Scheme global variable (for injecting hook context, etc.).
    pub fn inject_value(&mut self, name: &str, value: &str) {
        self.engine
            .register_value(name, SteelVal::StringV(value.into()));
    }

    /// Evaluate code and append input + result to a REPL output string.
    /// Returns the formatted output (prompt + result or error) suitable
    /// for appending to the `*Scheme*` buffer.
    pub fn eval_for_repl(&mut self, code: &str, editor: &mut Editor) -> String {
        self.inject_editor_state(editor);
        let result = match self.eval(code) {
            Ok(val) => {
                self.apply_to_editor(editor);
                if val.is_empty() {
                    "; => (void)".to_string()
                } else {
                    format!("; => {}", val)
                }
            }
            Err(e) => format!("; error: {}", e),
        };
        format!("> {}\n{}\n", code.trim(), result)
    }
}

/// Parse a string value as a boolean (for `set-option!`).
fn parse_bool(s: &str) -> bool {
    matches!(s, "true" | "#t" | "1" | "yes" | "on")
}

fn steel_val_to_string(val: &SteelVal) -> String {
    match val {
        SteelVal::Void => String::new(),
        SteelVal::BoolV(b) => if *b { "#t" } else { "#f" }.to_string(),
        SteelVal::IntV(n) => n.to_string(),
        SteelVal::NumV(n) => format!("{}", n),
        SteelVal::StringV(s) => s.to_string(),
        SteelVal::CharV(c) => format!("#\\{}", c),
        other => format!("{}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::{parse_key_seq, CommandSource, Editor};

    #[test]
    fn new_runtime_creates_successfully() {
        let rt = SchemeRuntime::new();
        assert!(rt.is_ok());
    }

    #[test]
    fn eval_arithmetic() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval("(+ 1 2 3)").unwrap();
        assert_eq!(result, "6");
    }

    #[test]
    fn eval_string_ops() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval(r#"(string-append "hello" " " "world")"#).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn eval_boolean() {
        let mut rt = SchemeRuntime::new().unwrap();
        assert_eq!(rt.eval("(= 1 1)").unwrap(), "#t");
        assert_eq!(rt.eval("(= 1 2)").unwrap(), "#f");
    }

    #[test]
    fn define_key_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        let keymap = editor.keymaps.get("normal").unwrap();
        let seq = parse_key_seq("Q");
        assert_eq!(keymap.lookup(&seq), mae_core::LookupResult::Exact("quit"));
    }

    #[test]
    fn define_command_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(define-command "greet" "Say hello" "greet-fn")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        let cmd = editor.commands.get("greet").unwrap();
        assert_eq!(cmd.doc, "Say hello");
        assert_eq!(cmd.source, CommandSource::Scheme("greet-fn".into()));
    }

    #[test]
    fn set_status_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-status "Hello from Scheme!")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.status_msg, "Hello from Scheme!");
    }

    #[test]
    fn inject_and_read_editor_state() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        // Insert some text so we have state to read
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'h');
        }
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'i');
        }

        rt.inject_editor_state(&editor);
        let result = rt.eval("*cursor-col*").unwrap();
        assert_eq!(result, "2");

        let result = rt.eval("*buffer-line-count*").unwrap();
        assert_eq!(result, "1");
    }

    #[test]
    fn load_file_works() {
        let dir = std::env::temp_dir().join("mae_test_scheme_load");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.scm");
        std::fs::write(&path, r#"(define-key "normal" "Z" "save-and-quit")"#).unwrap();

        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.load_file(&path).unwrap();
        rt.apply_to_editor(&mut editor);

        let keymap = editor.keymaps.get("normal").unwrap();
        assert_eq!(
            keymap.lookup(&parse_key_seq("Z")),
            mae_core::LookupResult::Exact("save-and-quit")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn define_key_spaced_sequence_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(define-key "normal" "SPC t t" "my-custom-cmd")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        let keymap = editor.keymaps.get("normal").unwrap();
        let seq = mae_core::parse_key_seq_spaced("SPC t t");
        assert_eq!(
            keymap.lookup(&seq),
            mae_core::LookupResult::Exact("my-custom-cmd")
        );
    }

    #[test]
    fn eval_error_returns_scheme_error() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval("(undefined-function)");
        assert!(result.is_err());
    }

    #[test]
    fn eval_error_recorded_in_history() {
        let mut rt = SchemeRuntime::new().unwrap();
        let _ = rt.eval("(undefined-function)");
        let errors = rt.last_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].expression.contains("undefined-function"));
        assert!(!errors[0].error_message.is_empty());
        assert_eq!(errors[0].seq, 1);
    }

    #[test]
    fn set_theme_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-theme "gruvbox-dark")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.theme.name, "gruvbox-dark");
    }

    #[test]
    fn list_user_commands_after_define() {
        let mut rt = SchemeRuntime::new().unwrap();
        rt.eval(r#"(define-command "greet" "Say hello" "greet-fn")"#)
            .unwrap();
        let cmds = rt.list_user_commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].0, "greet");
    }

    #[test]
    fn list_keybindings_after_define() {
        let mut rt = SchemeRuntime::new().unwrap();
        rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
        let bindings = rt.list_keybindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], ("normal".into(), "Q".into(), "quit".into()));
    }

    #[test]
    fn eval_for_debug_works() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval_for_debug("(+ 10 20)").unwrap();
        assert_eq!(result, "30");
    }

    // --- New API surface tests ---

    #[test]
    fn buffer_text_global_available() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'A');
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'B');
        }
        rt.inject_editor_state(&editor);
        let result = rt.eval("*buffer-text*").unwrap();
        assert_eq!(result, "AB");
    }

    #[test]
    fn mode_global_available() {
        let mut rt = SchemeRuntime::new().unwrap();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        assert_eq!(rt.eval("*mode*").unwrap(), "normal");
    }

    #[test]
    fn buffer_line_function_works() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            for ch in "hello\nworld".chars() {
                editor.buffers[0].insert_char(win, ch);
            }
        }
        rt.inject_editor_state(&editor);
        let line0 = rt.eval("(buffer-line 0)").unwrap();
        assert!(line0.contains("hello"));
        let line1 = rt.eval("(buffer-line 1)").unwrap();
        assert!(line1.contains("world"));
        // Out-of-range returns empty string
        let line99 = rt.eval("(buffer-line 99)").unwrap();
        assert_eq!(line99, "");
    }

    #[test]
    fn buffer_insert_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        rt.eval(r#"(buffer-insert "hello")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "hello");
    }

    #[test]
    fn cursor_goto_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            for ch in "abc\ndef\nghi".chars() {
                editor.buffers[0].insert_char(win, ch);
            }
        }
        rt.eval("(cursor-goto 1 2)").unwrap();
        rt.apply_to_editor(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn run_command_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        // search-forward-start switches to Search mode.
        rt.eval(r#"(run-command "search-forward-start")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.mode, mae_core::Mode::Search);
    }

    #[test]
    fn eval_for_repl_formats_output() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        let output = rt.eval_for_repl("(+ 1 2)", &mut editor);
        assert!(output.contains("> (+ 1 2)"));
        assert!(output.contains("; => 3"));
    }

    #[test]
    fn eval_for_repl_formats_error() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        let output = rt.eval_for_repl("(undefined-fn)", &mut editor);
        assert!(output.contains("> (undefined-fn)"));
        assert!(output.contains("; error:"));
    }

    #[test]
    fn multiple_define_keys_in_sequence() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(
            r#"
            (define-key "normal" "j" "move-down")
            (define-key "normal" "k" "move-up")
            (define-key "normal" "dd" "delete-line")
        "#,
        )
        .unwrap();
        rt.apply_to_editor(&mut editor);

        let km = editor.keymaps.get("normal").unwrap();
        assert_eq!(
            km.lookup(&parse_key_seq("j")),
            mae_core::LookupResult::Exact("move-down")
        );
        assert_eq!(
            km.lookup(&parse_key_seq("k")),
            mae_core::LookupResult::Exact("move-up")
        );
        assert_eq!(
            km.lookup(&parse_key_seq("dd")),
            mae_core::LookupResult::Exact("delete-line")
        );
    }

    // --- Hook system tests ---

    #[test]
    fn add_hook_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(add-hook! "before-save" "my-save-fn")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.hooks.get("before-save"), &["my-save-fn"]);
    }

    #[test]
    fn remove_hook_from_scheme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(add-hook! "after-save" "fn-a")"#).unwrap();
        rt.eval(r#"(add-hook! "after-save" "fn-b")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.hooks.get("after-save").len(), 2);

        rt.eval(r#"(remove-hook! "after-save" "fn-a")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.hooks.get("after-save"), &["fn-b"]);
    }

    #[test]
    fn add_hook_invalid_name_warns() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(add-hook! "nonexistent" "fn")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        // Should have set a warning in status
        assert!(editor.status_msg.contains("Unknown hook"));
    }

    // --- set-option! tests ---

    #[test]
    fn set_option_line_numbers() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        assert!(editor.show_line_numbers); // default true

        rt.eval(r#"(set-option! "line-numbers" "false")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(!editor.show_line_numbers);
    }

    #[test]
    fn set_option_word_wrap() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        assert!(!editor.word_wrap); // default false

        rt.eval(r#"(set-option! "word-wrap" "true")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.word_wrap);
    }

    #[test]
    fn set_option_relative_line_numbers() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "relative-line-numbers" "on")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.relative_line_numbers);
    }

    #[test]
    fn set_option_theme() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "theme" "gruvbox-dark")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.theme.name, "gruvbox-dark");
    }

    #[test]
    fn set_option_show_break() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "show-break" ">> ")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.show_break, ">> ");
    }

    #[test]
    fn set_option_unknown_warns() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "nonexistent" "value")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.status_msg.contains("Unknown option"));
    }
}
