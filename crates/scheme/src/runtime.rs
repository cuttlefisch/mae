use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use tracing::{debug, error, info, warn};

use mae_core::{parse_key_seq_spaced, Editor};

use crate::ffi::{
    arg_bool, arg_float, arg_int, arg_opt_string, arg_string, list_to_strings, value_to_display,
};
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

/// Accumulated config data from Scheme evaluation.
/// Shared between Rust and Scheme VM via Arc<Mutex<>>.
///
/// Foreign functions require Send + Sync + 'static closures.
/// Arc<Mutex<>> satisfies these bounds, and since the VM is
/// single-threaded, the mutex is never contended.
#[derive(Default)]
struct SharedState {
    /// (keymap_name, key_string, command_name)
    keymap_bindings: Vec<(String, String, String)>,
    /// (keymap_name, parent_name) — new keymaps to create
    keymap_defs: Vec<(String, String)>,
    /// (selector_type, selector_value, keymap) — route a buffer context (kind or
    /// language) to a context keymap. Drains into `Editor::keymap_registry`.
    context_bindings: Vec<(String, String, String)>,
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
    /// Recent files to add: (path).
    pending_recent_files: Vec<String>,
    /// Recent projects to add: (path).
    pending_recent_projects: Vec<String>,
    /// Visual buffer operations.
    pending_visual_ops: Vec<VisualOp>,
    /// Buffer-local option changes: (key, value).
    pending_local_options: Vec<(String, String)>,

    // --- Round 2: extended buffer/window/command API ---
    /// Pending delete range: (start_offset, end_offset)
    pending_delete_range: Option<(usize, usize)>,
    /// Pending replace range: (start_offset, end_offset, replacement_text)
    pending_replace_range: Option<(usize, usize, String)>,
    /// Pending undo
    pending_undo: bool,
    /// Pending redo
    pending_redo: bool,
    /// Pending undo boundary (sync_undo_boundary)
    pending_undo_boundary: bool,
    /// Pending switch-to-buffer index
    pending_switch_buffer: Option<usize>,
    /// Key removals: (keymap_name, key_string)
    pending_key_removals: Vec<(String, String)>,
    /// Group name assignments: (keymap_name, prefix_key_string, label)
    pending_group_names: Vec<(String, String, String)>,

    // --- Package infrastructure ---
    /// Features that have been `provide`d.
    loaded_features: HashSet<String>,
    /// Directories to search for `(require FEATURE)`.
    load_path: Vec<PathBuf>,
    /// Pending `(require FEATURE)` calls to resolve after eval.
    pending_requires: Vec<String>,
    /// Pending `(autoload CMD FEATURE DOC)` registrations.
    pending_autoloads: Vec<(String, String, String)>,
    /// Pending display-rule overrides: (kind_name, action_string).
    pending_display_rules: Vec<(String, String)>,
    /// Pending replaceable-kind changes: (kind_name, enable).
    pending_replaceable_kinds: Vec<(String, bool)>,
    /// Paths to add to org agenda files via `(agenda-add! PATH)`.
    pending_agenda_adds: Vec<String>,
    /// Paths to remove from org agenda files via `(agenda-remove! PATH)`.
    pending_agenda_removes: Vec<String>,
    /// Request to display agenda file list via `(agenda-list)`.
    pending_agenda_list: bool,
    /// Dynamic option registrations from modules: (name, kind, default, doc).
    pending_dynamic_options: Vec<(String, String, String, String)>,
    /// Active modules: name → version.
    active_modules: HashMap<String, String>,
    /// Declared modules from `(mae! ...)`: name → enabled flags.
    declared_modules: HashMap<String, Vec<String>>,
    /// Declared packages from `(package! ...)`.
    declared_packages: Vec<DeclaredPackage>,
    /// KB nodes registered from Scheme via `(define-kb-node! ID TITLE BODY)`.
    pending_kb_nodes: Vec<(String, String, String)>,
    /// Pending KB typed links: (source, target, rel_type).
    pending_kb_links: Vec<(String, String, String)>,
    /// Pending KB link removals: (source, target).
    pending_kb_link_removals: Vec<(String, String)>,
    /// Pending KB meta-member additions: (meta_id, member_id, role).
    pending_kb_meta_adds: Vec<(String, String, String)>,
    /// Pending KB meta-member removals: (meta_id, member_id).
    pending_kb_meta_removes: Vec<(String, String)>,
    /// Pending KB collaboration lifecycle actions from `(kb-share)` etc. — lowered
    /// to `CollabIntent`s editor-side via `Editor::queue_kb_collab_action`.
    pending_kb_collab_actions: Vec<mae_core::KbCollabAction>,
    /// Daemon control channel (cloned from `editor.kb` on each state sync) so
    /// synchronous-return primitives like `(kb-share-p2p)` can drive the same
    /// backend as the command + MCP tool (ADR-025 §"Driving surfaces").
    daemon_control: Option<std::sync::Arc<dyn mae_core::DaemonControl>>,
    /// Pending buffer creation: (name).
    pending_create_buffer: Option<String>,
    /// Pending buffer kill by name.
    pending_kill_buffer: Option<String>,
    /// Pending advice-add: (command, kind, fn_name).
    pending_advice_adds: Vec<(String, String, String)>,
    /// Pending advice-remove: (command, fn_name).
    pending_advice_removes: Vec<(String, String)>,
    /// Pending command unregistrations (for module unload).
    pending_command_unregisters: Vec<String>,
    /// Pending option unregistrations (for module unload).
    pending_option_unregisters: Vec<String>,
    /// Deprecated function warnings: old_name → (new_name, since_version).
    /// Warnings emitted on first call.
    deprecated_functions: HashMap<String, (String, String)>,
    /// Already-warned deprecated function names (to warn only once).
    deprecated_warned: HashSet<String>,
    /// Pending AI tool registrations from Scheme.
    pending_ai_tools: Vec<mae_core::SchemeToolDef>,
    /// Param accumulator for `ai-tool-param!` calls (tool_name → params).
    pending_ai_tool_params: HashMap<String, Vec<(String, String, String)>>,
    /// Required param accumulator for `ai-tool-require!` calls.
    pending_ai_tool_required: HashMap<String, Vec<String>>,
    /// Pending custom splash art registrations: (name, art, image_path).
    pending_splash_arts: Vec<(String, String, Option<PathBuf>)>,
    /// Current module directory (set before loading each module's autoloads).
    /// Used by `register-splash-art-image!` to resolve relative paths.
    current_module_dir: Option<PathBuf>,

    // --- Test framework primitives ---
    /// Pending exit code from `(exit CODE)`.
    pending_exit_code: Option<i32>,
    /// Pending file writes from `(write-file PATH CONTENT)`.
    pending_write_files: Vec<(String, String)>,
    /// Ex-commands to dispatch via `(execute-ex CMD-STRING)`.
    /// Routes through `execute_command()` which handles argument parsing.
    pending_ex_commands: Vec<String>,
    /// Raw key sequences to feed through the real `handle_key` pipeline via
    /// `(feed-keys "C-; b s")` — the E2E key-injection test primitive. Drained
    /// by the test runner (which owns `handle_key`); each string is parsed and
    /// each key dispatched against the real loaded keymaps + event loop.
    pending_feed_keys: Vec<String>,
    /// Whether the transient leader keypad is active (from inject_editor_state).
    leader_active: bool,
    /// Count of which-key entries for the current keymap (from inject_editor_state).
    which_key_count: usize,

    // --- CRDT/sync test primitives ---
    /// Pending enable-sync: client_id for active buffer.
    pending_enable_sync: Option<u64>,
    /// Pending disable-sync on active buffer.
    pending_disable_sync: bool,
    /// Pending sync updates to apply: (buffer_name, base64-encoded update).
    pending_sync_applies: Vec<(String, Vec<u8>)>,
    /// Pending load-sync-state: (base64-decoded state bytes, client_id).
    pending_load_sync_state: Option<(Vec<u8>, u64)>,
    /// Accumulated sync updates from pending_sync_updates (base64-encoded).
    /// Always captured after each apply cycle; drained by `(buffer-drain-updates)`.
    accumulated_sync_updates: Vec<String>,
    /// Current mode string (updated by inject_editor_state).
    current_mode: String,
    /// Active buffer text (updated by inject_editor_state).
    current_buffer_text: String,
    /// All buffer texts for (buffer-text NAME) (updated by inject_editor_state).
    all_buffer_texts: Vec<(String, String)>,
    /// Whether sync is enabled on active buffer (updated by inject_editor_state).
    sync_enabled: bool,
    /// Number of pending sync updates (updated by inject_editor_state).
    pending_update_count: usize,
    /// Sync doc content (None if sync not enabled) (updated by inject_editor_state).
    sync_content: Option<String>,
    /// Encoded sync state (None if sync not enabled) (updated by inject_editor_state).
    encoded_state: Option<String>,
    /// Buffer name→index mapping (updated by inject_editor_state).
    buffer_names: Vec<(usize, String)>,

    /// Snapshot of option values: (name, value_string).
    option_values: Vec<(String, String)>,

    /// KB store reference for read-only queries (updated by inject_editor_state).
    kb_store: Option<Arc<dyn mae_kb::KbStore>>,

    // --- Visual/region state (updated by inject_editor_state) ---
    /// Whether a visual selection is active.
    region_active: bool,
    /// Start offset of the visual selection.
    region_start: usize,
    /// End offset of the visual selection.
    region_end: usize,

    // --- Cursor state (updated by inject_editor_state) ---
    /// Cursor row (0-indexed).
    cursor_row: usize,
    /// Cursor column (0-indexed).
    cursor_col: usize,
    /// Last status message set by the editor.
    last_status_message: String,

    // --- State vector / reconcile (new CRDT test primitives) ---
    /// Pending state vector encode request.
    pending_encode_state_vector: bool,
    /// Encoded state vector result (base64).
    encoded_state_vector: Option<String>,
    /// Pending compute-diff: (remote_state_vector_base64).
    pending_compute_diff: Option<String>,
    /// Computed diff result (base64).
    computed_diff: Option<String>,
    /// Pending reconcile-to: target text.
    pending_reconcile_to: Option<String>,
    /// Reconcile result (base64 update).
    reconcile_result: Option<String>,

    // --- Introspection (Phase 13h) ---
    /// Cached GC stats snapshot (updated each eval cycle).
    gc_stats_snapshot: crate::vm::GcStats,
}

#[derive(Debug, Clone)]
pub enum VisualOp {
    AddRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<String>,
        stroke: Option<String>,
    },
    AddLine {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: String,
        thickness: f32,
    },
    AddCircle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Option<String>,
        stroke: Option<String>,
    },
    AddText {
        x: f32,
        y: f32,
        text: String,
        font_size: f32,
        color: String,
    },
    Clear,
}

/// A declared third-party package from `(package! ...)` in init.scm.
#[derive(Debug, Clone)]
pub struct DeclaredPackage {
    pub name: String,
    pub source: Option<String>,
    pub pin: Option<String>,
    pub disable: bool,
}

/// A captured Scheme evaluation error for debugger introspection.
#[derive(Debug, Clone)]
pub struct SchemeErrorSnapshot {
    pub expression: String,
    pub error_message: String,
    pub seq: u64,
}

/// Wraps the mae-scheme VM and provides the Scheme extension API.
///
/// Design: the VM and Editor live on the same thread. Scheme eval
/// blocks the event loop briefly — acceptable for config loading and
/// interactive REPL.
pub struct SchemeRuntime {
    vm: Vm,
    shared: Arc<Mutex<SharedState>>,
    /// Ring buffer of recent eval errors for debugger introspection.
    error_history: Vec<SchemeErrorSnapshot>,
    /// Monotonic sequence counter for error ordering.
    error_seq: u64,
    /// Maximum errors to retain.
    max_errors: usize,
    /// Directories to search for `(require FEATURE)`.
    pub load_path: Vec<PathBuf>,
    /// Features that have been successfully loaded via `require`/`provide`.
    pub loaded_features: HashSet<String>,
}

/// Result of a yielding eval — either completed or suspended.
#[derive(Debug)]
pub enum SchemeEvalResult {
    /// Evaluation completed, result is a display string.
    Done(String),
    /// VM yielded, caller must handle the request and call `resume_yield`.
    Yield(crate::vm::YieldRequest),
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

impl From<LispError> for SchemeError {
    fn from(err: LispError) -> Self {
        SchemeError {
            message: err.message(),
        }
    }
}

impl SchemeRuntime {
    /// Read-only access to the VM for LSP introspection.
    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    /// Mutable access to the VM for DAP debugging (breakpoints, step mode, debug mode).
    pub fn vm_mut(&mut self) -> &mut Vm {
        &mut self.vm
    }

    pub fn new() -> Result<Self, SchemeError> {
        let mut vm = Vm::new();
        let shared = Arc::new(Mutex::new(SharedState::default()));

        // Install R7RS standard library + library facades + mae libraries + introspection
        crate::stdlib::register_stdlib(&mut vm);
        crate::stdlib::register_r7rs_libraries(&mut vm);
        crate::stdlib::register_mae_libs(&mut vm);
        crate::introspect::register_introspection(&mut vm);

        // (gc-stats) — reads cached stats from SharedState
        let s = shared.clone();
        vm.register_fn(
            "gc-stats",
            "Return GC statistics as an association list.",
            Arity::Fixed(0),
            move |_args| {
                let st = s.lock();
                let stats = &st.gc_stats_snapshot;
                Ok(Value::list(vec![
                    Value::cons(
                        Value::symbol("eval-count"),
                        Value::Int(stats.eval_count as i64),
                    ),
                    Value::cons(
                        Value::symbol("collections"),
                        Value::Int(stats.collections_count as i64),
                    ),
                    Value::cons(
                        Value::symbol("globals-count"),
                        Value::Int(stats.globals_count as i64),
                    ),
                    Value::cons(
                        Value::symbol("stack-hwm"),
                        Value::Int(stats.stack_hwm as i64),
                    ),
                    Value::cons(
                        Value::symbol("frame-hwm"),
                        Value::Int(stats.frame_hwm as i64),
                    ),
                ]))
            },
        );

        // --- Keybinding registration ---

        // (define-key MAP KEY COMMAND)
        let s = shared.clone();
        vm.register_fn(
            "define-key",
            "Bind KEY to COMMAND in keymap MAP",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let map = arg_string(args, 0, "define-key")?;
                let key = arg_string(args, 1, "define-key")?;
                let cmd = arg_string(args, 2, "define-key")?;
                s.lock().keymap_bindings.push((map, key, cmd));
                Ok(Value::Void)
            },
        );

        // (define-keymap NAME PARENT)
        let s = shared.clone();
        vm.register_fn(
            "define-keymap",
            "Create a new keymap NAME with PARENT",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "define-keymap")?;
                let parent = arg_string(args, 1, "define-keymap")?;
                s.lock().keymap_defs.push((name, parent));
                Ok(Value::Void)
            },
        );

        // (bind-context-keymap SELECTOR-TYPE SELECTOR-VALUE KEYMAP)
        // Route a buffer context to a context keymap (overlays the modality keymap
        // in the resolution chain). SELECTOR-TYPE is "kind" (e.g. "dashboard",
        // "file-tree") or "language" (e.g. "org"). This is how a module wires a
        // new buffer kind / a shared "navigation" context without a kernel patch.
        let s = shared.clone();
        vm.register_fn(
            "bind-context-keymap",
            "Route a buffer context (kind/language) to a context keymap",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let selector_type = arg_string(args, 0, "bind-context-keymap")?;
                let selector_value = arg_string(args, 1, "bind-context-keymap")?;
                let keymap = arg_string(args, 2, "bind-context-keymap")?;
                s.lock()
                    .context_bindings
                    .push((selector_type, selector_value, keymap));
                Ok(Value::Void)
            },
        );

        // (define-command NAME DOC SCHEME-FN-NAME)
        let s = shared.clone();
        vm.register_fn(
            "define-command",
            "Register a command NAME with doc and handler",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "define-command")?;
                let doc = arg_string(args, 1, "define-command")?;
                let fn_name = arg_string(args, 2, "define-command")?;
                s.lock().command_defs.push((name, doc, fn_name));
                Ok(Value::Void)
            },
        );

        // (set-status MSG)
        let s = shared.clone();
        vm.register_fn(
            "set-status",
            "Set the status bar message",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let msg = arg_string(args, 0, "set-status")?;
                s.lock().status_message = Some(msg);
                Ok(Value::Void)
            },
        );

        // (set-theme NAME)
        let s = shared.clone();
        vm.register_fn(
            "set-theme",
            "Set the color theme",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "set-theme")?;
                s.lock().theme_request = Some(name);
                Ok(Value::Void)
            },
        );

        // --- Live editing primitives ---

        // (buffer-insert TEXT)
        let s = shared.clone();
        vm.register_fn(
            "buffer-insert",
            "Insert text at cursor",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let text = arg_string(args, 0, "buffer-insert")?;
                s.lock().pending_insert = Some(text);
                Ok(Value::Void)
            },
        );

        // (cursor-goto ROW COL)
        let s = shared.clone();
        vm.register_fn(
            "cursor-goto",
            "Move cursor to absolute position (0-indexed)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let row = arg_int(args, 0, "cursor-goto")?;
                let col = arg_int(args, 1, "cursor-goto")?;
                s.lock().pending_cursor = Some((row.max(0) as usize, col.max(0) as usize));
                Ok(Value::Void)
            },
        );

        // (open-file PATH)
        let s = shared.clone();
        vm.register_fn(
            "open-file",
            "Open a file in a new buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "open-file")?;
                s.lock().pending_open_file = Some(path);
                Ok(Value::Void)
            },
        );

        // (run-command NAME)
        let s = shared.clone();
        vm.register_fn(
            "run-command",
            "Dispatch a registered command by name",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "run-command")?;
                s.lock().pending_commands.push(name);
                Ok(Value::Void)
            },
        );

        // (execute-ex CMD-STRING)
        let s = shared.clone();
        vm.register_fn(
            "execute-ex",
            "Route through ex-command parser",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let cmd = arg_string(args, 0, "execute-ex")?;
                s.lock().pending_ex_commands.push(cmd);
                Ok(Value::Void)
            },
        );

        // (message TEXT)
        let s = shared.clone();
        vm.register_fn(
            "message",
            "Append to the *Messages* log",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let text = arg_string(args, 0, "message")?;
                s.lock().pending_messages.push(text);
                Ok(Value::Void)
            },
        );

        // --- Hook system ---

        // (add-hook! HOOK-NAME FN-NAME)
        let s = shared.clone();
        vm.register_fn(
            "add-hook!",
            "Register a hook callback",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let hook = arg_string(args, 0, "add-hook!")?;
                let fn_name = arg_string(args, 1, "add-hook!")?;
                s.lock().pending_hook_adds.push((hook, fn_name));
                Ok(Value::Void)
            },
        );

        // (remove-hook! HOOK-NAME FN-NAME)
        let s = shared.clone();
        vm.register_fn(
            "remove-hook!",
            "Remove a hook callback",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let hook = arg_string(args, 0, "remove-hook!")?;
                let fn_name = arg_string(args, 1, "remove-hook!")?;
                s.lock().pending_hook_removes.push((hook, fn_name));
                Ok(Value::Void)
            },
        );

        // --- Editor options ---

        // (set-option! KEY VALUE)
        let s = shared.clone();
        vm.register_fn(
            "set-option!",
            "Set an editor option",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let key = arg_string(args, 0, "set-option!")?;
                let value = arg_string(args, 1, "set-option!")?;
                s.lock().pending_options.push((key, value));
                Ok(Value::Void)
            },
        );

        // (set-local-option! KEY VALUE)
        let s = shared.clone();
        vm.register_fn(
            "set-local-option!",
            "Set a buffer-local option",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let key = arg_string(args, 0, "set-local-option!")?;
                let value = arg_string(args, 1, "set-local-option!")?;
                s.lock().pending_local_options.push((key, value));
                Ok(Value::Void)
            },
        );

        // (display-buffer-policy KIND)
        vm.register_fn(
            "display-buffer-policy",
            "Query active display rule for a BufferKind",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let kind = arg_string(args, 0, "display-buffer-policy")?;
                use mae_core::display_policy::{
                    action_to_string, parse_buffer_kind, DisplayPolicy,
                };
                match parse_buffer_kind(&kind) {
                    Some(bk) => {
                        let policy = DisplayPolicy::default();
                        Ok(Value::string(action_to_string(&policy.action_for(bk))))
                    }
                    None => Ok(Value::string(format!("unknown kind: {}", kind))),
                }
            },
        );

        // (set-display-rule! KIND ACTION)
        let s = shared.clone();
        vm.register_fn(
            "set-display-rule!",
            "Override display policy",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kind = arg_string(args, 0, "set-display-rule!")?;
                let action = arg_string(args, 1, "set-display-rule!")?;
                s.lock().pending_display_rules.push((kind, action));
                Ok(Value::Void)
            },
        );

        // (set-buffer-kind-replaceable! KIND ENABLE)
        let s = shared.clone();
        vm.register_fn(
            "set-buffer-kind-replaceable!",
            "Mark a buffer kind as replaceable",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kind = arg_string(args, 0, "set-buffer-kind-replaceable!")?;
                let enable = arg_bool(args, 1, "set-buffer-kind-replaceable!")?;
                s.lock().pending_replaceable_kinds.push((kind, enable));
                Ok(Value::Void)
            },
        );

        // --- Shell terminal bindings ---

        // (shell-send-input BUF-IDX TEXT)
        let s = shared.clone();
        vm.register_fn(
            "shell-send-input",
            "Send text to a terminal PTY",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let buf_idx = arg_int(args, 0, "shell-send-input")?;
                let text = arg_string(args, 1, "shell-send-input")?;
                if buf_idx >= 0 {
                    s.lock().pending_shell_inputs.push((buf_idx as usize, text));
                }
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "recent-files-add!",
            "Add a file to recent files",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "recent-files-add!")?;
                s.lock().pending_recent_files.push(path);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "recent-projects-add!",
            "Add a project to recent projects",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "recent-projects-add!")?;
                s.lock().pending_recent_projects.push(path);
                Ok(Value::Void)
            },
        );

        // --- Agenda file management ---

        let s = shared.clone();
        vm.register_fn(
            "agenda-add!",
            "Add a path to org agenda files",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "agenda-add!")?;
                s.lock().pending_agenda_adds.push(path);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "agenda-remove!",
            "Remove a path from org agenda files",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "agenda-remove!")?;
                s.lock().pending_agenda_removes.push(path);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "agenda-list",
            "Display agenda file list",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_agenda_list = true;
                Ok(Value::Void)
            },
        );

        // --- Visual buffer operations ---

        let s = shared.clone();
        vm.register_fn(
            "visual-buffer-add-rect!",
            "Add a rectangle to visual buffer",
            Arity::Variadic(4),
            move |args: &[Value]| {
                let x = arg_float(args, 0, "visual-buffer-add-rect!")? as f32;
                let y = arg_float(args, 1, "visual-buffer-add-rect!")? as f32;
                let w = arg_float(args, 2, "visual-buffer-add-rect!")? as f32;
                let h = arg_float(args, 3, "visual-buffer-add-rect!")? as f32;
                let fill = arg_opt_string(args, 4, "visual-buffer-add-rect!");
                let stroke = arg_opt_string(args, 5, "visual-buffer-add-rect!");
                s.lock().pending_visual_ops.push(VisualOp::AddRect {
                    x,
                    y,
                    w,
                    h,
                    fill,
                    stroke,
                });
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "visual-buffer-clear!",
            "Clear all visual elements",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_visual_ops.push(VisualOp::Clear);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "visual-buffer-add-line!",
            "Add a line to visual buffer",
            Arity::Fixed(6),
            move |args: &[Value]| {
                let x1 = arg_float(args, 0, "visual-buffer-add-line!")? as f32;
                let y1 = arg_float(args, 1, "visual-buffer-add-line!")? as f32;
                let x2 = arg_float(args, 2, "visual-buffer-add-line!")? as f32;
                let y2 = arg_float(args, 3, "visual-buffer-add-line!")? as f32;
                let color = arg_string(args, 4, "visual-buffer-add-line!")?;
                let thickness = arg_float(args, 5, "visual-buffer-add-line!")? as f32;
                s.lock().pending_visual_ops.push(VisualOp::AddLine {
                    x1,
                    y1,
                    x2,
                    y2,
                    color,
                    thickness,
                });
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "visual-buffer-add-circle!",
            "Add a circle to visual buffer",
            Arity::Variadic(3),
            move |args: &[Value]| {
                let cx = arg_float(args, 0, "visual-buffer-add-circle!")? as f32;
                let cy = arg_float(args, 1, "visual-buffer-add-circle!")? as f32;
                let r = arg_float(args, 2, "visual-buffer-add-circle!")? as f32;
                let fill = arg_opt_string(args, 3, "visual-buffer-add-circle!");
                let stroke = arg_opt_string(args, 4, "visual-buffer-add-circle!");
                s.lock().pending_visual_ops.push(VisualOp::AddCircle {
                    cx,
                    cy,
                    r,
                    fill,
                    stroke,
                });
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "visual-buffer-add-text!",
            "Add text to visual buffer",
            Arity::Fixed(5),
            move |args: &[Value]| {
                let x = arg_float(args, 0, "visual-buffer-add-text!")? as f32;
                let y = arg_float(args, 1, "visual-buffer-add-text!")? as f32;
                let text = arg_string(args, 2, "visual-buffer-add-text!")?;
                let font_size = arg_float(args, 3, "visual-buffer-add-text!")? as f32;
                let color = arg_string(args, 4, "visual-buffer-add-text!")?;
                s.lock().pending_visual_ops.push(VisualOp::AddText {
                    x,
                    y,
                    text,
                    font_size,
                    color,
                });
                Ok(Value::Void)
            },
        );

        // --- Round 2: buffer editing, buffer list, keymap introspection ---

        // (buffer-delete-range START END)
        let s = shared.clone();
        vm.register_fn(
            "buffer-delete-range",
            "Delete text in range",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let start = arg_int(args, 0, "buffer-delete-range")?;
                let end = arg_int(args, 1, "buffer-delete-range")?;
                s.lock().pending_delete_range = Some((start.max(0) as usize, end.max(0) as usize));
                Ok(Value::Void)
            },
        );

        // (buffer-replace-range START END TEXT)
        let s = shared.clone();
        vm.register_fn(
            "buffer-replace-range",
            "Replace text in range",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let start = arg_int(args, 0, "buffer-replace-range")?;
                let end = arg_int(args, 1, "buffer-replace-range")?;
                let text = arg_string(args, 2, "buffer-replace-range")?;
                s.lock().pending_replace_range =
                    Some((start.max(0) as usize, end.max(0) as usize, text));
                Ok(Value::Void)
            },
        );

        // (buffer-undo)
        let s = shared.clone();
        vm.register_fn(
            "buffer-undo",
            "Undo the last edit",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_undo = true;
                Ok(Value::Void)
            },
        );

        // (buffer-redo)
        let s = shared.clone();
        vm.register_fn(
            "buffer-redo",
            "Redo the last undone edit",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_redo = true;
                Ok(Value::Void)
            },
        );

        // (buffer-undo-boundary)
        let s = shared.clone();
        vm.register_fn(
            "buffer-undo-boundary",
            "Mark an undo boundary",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_undo_boundary = true;
                Ok(Value::Void)
            },
        );

        // (switch-to-buffer IDX)
        let s = shared.clone();
        vm.register_fn(
            "switch-to-buffer",
            "Switch to buffer by index",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let idx = arg_int(args, 0, "switch-to-buffer")?;
                s.lock().pending_switch_buffer = Some(idx.max(0) as usize);
                Ok(Value::Void)
            },
        );

        // (undefine-key! MAP KEY)
        let s = shared.clone();
        vm.register_fn(
            "undefine-key!",
            "Remove a keybinding",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let map = arg_string(args, 0, "undefine-key!")?;
                let key = arg_string(args, 1, "undefine-key!")?;
                s.lock().pending_key_removals.push((map, key));
                Ok(Value::Void)
            },
        );

        // (set-group-name MAP PREFIX LABEL)
        let s = shared.clone();
        vm.register_fn(
            "set-group-name",
            "Set which-key group label",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let map = arg_string(args, 0, "set-group-name")?;
                let prefix = arg_string(args, 1, "set-group-name")?;
                let label = arg_string(args, 2, "set-group-name")?;
                s.lock().pending_group_names.push((map, prefix, label));
                Ok(Value::Void)
            },
        );

        // --- File I/O ---

        // (read-file PATH)
        vm.register_fn(
            "read-file",
            "Read a file (capped at 1MB)",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "read-file")?;
                match std::fs::read_to_string(&path) {
                    Ok(content) if content.len() <= 1_048_576 => Ok(Value::string(content)),
                    Ok(_) => Ok(Value::string("ERROR: file exceeds 1MB limit")),
                    Err(e) => Ok(Value::string(format!("ERROR: {}", e))),
                }
            },
        );

        // (file-exists? PATH)
        vm.register_fn(
            "file-exists?",
            "Check if a file exists",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "file-exists?")?;
                Ok(Value::Bool(std::path::Path::new(&path).exists()))
            },
        );

        // (list-directory PATH)
        vm.register_fn(
            "list-directory",
            "List directory entries",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "list-directory")?;
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let items: Vec<Value> = entries
                            .flatten()
                            .map(|e| {
                                let name = e.file_name().to_string_lossy().into_owned();
                                let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                                Value::list(vec![Value::string(name), Value::Bool(is_dir)])
                            })
                            .collect();
                        Ok(Value::list(items))
                    }
                    Err(_) => Ok(Value::Null),
                }
            },
        );

        // --- Package infrastructure ---

        // (provide-feature FEATURE)
        let s = shared.clone();
        vm.register_fn(
            "provide-feature",
            "Mark feature as loaded",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let feature = arg_string(args, 0, "provide-feature")?;
                s.lock().loaded_features.insert(feature);
                Ok(Value::Void)
            },
        );

        // (featurep FEATURE)
        let s = shared.clone();
        vm.register_fn(
            "featurep",
            "Check if feature is loaded",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let feature = arg_string(args, 0, "featurep")?;
                Ok(Value::Bool(s.lock().loaded_features.contains(&feature)))
            },
        );

        // (require-feature FEATURE)
        let s = shared.clone();
        vm.register_fn(
            "require-feature",
            "Request loading a feature",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let feature = arg_string(args, 0, "require-feature")?;
                let mut state = s.lock();
                if !state.loaded_features.contains(&feature) {
                    state.pending_requires.push(feature);
                }
                Ok(Value::Void)
            },
        );

        // (load-path)
        let s = shared.clone();
        vm.register_fn(
            "load-path",
            "Return current load-path",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let state = s.lock();
                let items: Vec<Value> = state
                    .load_path
                    .iter()
                    .map(|p| Value::string(p.to_string_lossy().into_owned()))
                    .collect();
                Ok(Value::list(items))
            },
        );

        // (add-to-load-path! DIR)
        let s = shared.clone();
        vm.register_fn(
            "add-to-load-path!",
            "Prepend directory to load-path",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let dir = arg_string(args, 0, "add-to-load-path!")?;
                s.lock().load_path.insert(0, PathBuf::from(dir));
                Ok(Value::Void)
            },
        );

        // (autoload COMMAND-NAME FEATURE DOC)
        let s = shared.clone();
        vm.register_fn(
            "autoload",
            "Register a command backed by autoload",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let cmd_name = arg_string(args, 0, "autoload")?;
                let feature = arg_string(args, 1, "autoload")?;
                let doc = arg_string(args, 2, "autoload")?;
                s.lock().pending_autoloads.push((cmd_name, feature, doc));
                Ok(Value::Void)
            },
        );

        // --- Module system functions ---

        // (when-flag MODULE-NAME FLAG-NAME THUNK)
        vm.eval(
            r#"
(define (when-flag module-name flag-name thunk)
  (thunk))
"#,
        )
        .ok();

        // (define-option! NAME KIND DEFAULT DOC)
        let s = shared.clone();
        vm.register_fn(
            "define-option!",
            "Register a runtime option",
            Arity::Fixed(4),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "define-option!")?;
                let kind = arg_string(args, 1, "define-option!")?;
                let default = arg_string(args, 2, "define-option!")?;
                let doc = arg_string(args, 3, "define-option!")?;
                s.lock()
                    .pending_dynamic_options
                    .push((name, kind, default, doc));
                Ok(Value::Void)
            },
        );

        // (module-loaded? NAME)
        let s = shared.clone();
        vm.register_fn(
            "module-loaded?",
            "Check if a module is active",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "module-loaded?")?;
                Ok(Value::Bool(s.lock().active_modules.contains_key(&name)))
            },
        );

        // (module-version NAME)
        let s = shared.clone();
        vm.register_fn(
            "module-version",
            "Get version of active module",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "module-version")?;
                match s.lock().active_modules.get(&name) {
                    Some(v) => Ok(Value::string(v.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // (module-list)
        let s = shared.clone();
        vm.register_fn(
            "module-list",
            "List all active module names",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let state = s.lock();
                Ok(Value::list(
                    state
                        .active_modules
                        .keys()
                        .map(|k| Value::string(k.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );

        // (register-module! NAME VERSION)
        let s = shared.clone();
        vm.register_fn(
            "register-module!",
            "Register a loaded module",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "register-module!")?;
                let version = arg_string(args, 1, "register-module!")?;
                s.lock().active_modules.insert(name, version);
                Ok(Value::Void)
            },
        );

        // (when-module NAME THUNK) — Scheme-level wrapper
        vm.eval(
            r#"
(define (when-module name thunk)
  (when (module-loaded? name)
    (thunk)))
"#,
        )
        .ok();

        // (module-flags NAME)
        vm.register_fn(
            "module-flags",
            "Get enabled flags for a module",
            Arity::Fixed(1),
            move |_args: &[Value]| Ok(Value::Null),
        );

        // --- Declarative package management (mae!, package!) ---

        // (mae-declare-module! NAME FLAGS)
        let s = shared.clone();
        vm.register_fn(
            "mae-declare-module!",
            "Declare a module with flags",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "mae-declare-module!")?;
                let flags = if args.len() > 1 {
                    list_to_strings(&args[1])
                } else {
                    vec![]
                };
                s.lock().declared_modules.insert(name, flags);
                Ok(Value::Void)
            },
        );

        // (mae-declared-modules)
        let s = shared.clone();
        vm.register_fn(
            "mae-declared-modules",
            "List declared module names",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let state = s.lock();
                Ok(Value::list(
                    state
                        .declared_modules
                        .keys()
                        .map(|k| Value::string(k.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );

        // (mae-declare-package! NAME SOURCE PIN DISABLE)
        let s = shared.clone();
        vm.register_fn(
            "mae-declare-package!",
            "Declare a third-party package",
            Arity::Fixed(4),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "mae-declare-package!")?;
                let source = arg_string(args, 1, "mae-declare-package!")?;
                let pin = arg_string(args, 2, "mae-declare-package!")?;
                let disable = arg_bool(args, 3, "mae-declare-package!")?;
                s.lock().declared_packages.push(DeclaredPackage {
                    name,
                    source: if source.is_empty() {
                        None
                    } else {
                        Some(source)
                    },
                    pin: if pin.is_empty() { None } else { Some(pin) },
                    disable,
                });
                Ok(Value::Void)
            },
        );

        // Define mae! and package! Scheme-level wrappers
        vm.eval(
            r#"
;; Pre-define category labels
(define :editor ":editor")
(define :ui ":ui")
(define :lang ":lang")
(define :tools ":tools")
(define :completion ":completion")
(define :emacs ":emacs")
(define :term ":term")
(define :os ":os")
(define :app ":app")
(define :config ":config")
(define :input ":input")

(define (mae! . args)
  (for-each
    (lambda (item)
      (cond
        ((and (string? item)
              (> (string-length item) 0)
              (equal? (substring item 0 1) ":"))
         #f)
        ((list? item)
         (mae-declare-module! (car item) (cdr item)))
        ((string? item)
         (mae-declare-module! item '()))
        ((symbol? item)
         (mae-declare-module! (symbol->string item) '()))
        (else #f)))
    args))

(define :source ":source")
(define :pin ":pin")
(define :disable ":disable")

(define (package! name . kwargs)
  (define (kwarg-ref key default)
    (let loop ((rest kwargs))
      (cond
        ((null? rest) default)
        ((and (>= (length rest) 2)
              (equal? (car rest) key))
         (cadr rest))
        (else (loop (cdr rest))))))
  (mae-declare-package! name
                        (kwarg-ref ":source" "")
                        (kwarg-ref ":pin" "")
                        (if (kwarg-ref ":disable" #f) #t #f)))
"#,
        )
        .ok();

        // (undefine-command! NAME)
        let s = shared.clone();
        vm.register_fn(
            "undefine-command!",
            "Remove a command",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "undefine-command!")?;
                s.lock().pending_command_unregisters.push(name);
                Ok(Value::Void)
            },
        );

        // (undefine-option! NAME)
        let s = shared.clone();
        vm.register_fn(
            "undefine-option!",
            "Remove an option",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "undefine-option!")?;
                s.lock().pending_option_unregisters.push(name);
                Ok(Value::Void)
            },
        );

        // (unload-feature NAME)
        let s = shared.clone();
        vm.register_fn(
            "unload-feature",
            "Remove from loaded_features",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "unload-feature")?;
                let removed = s.lock().loaded_features.remove(&name);
                Ok(Value::Bool(removed))
            },
        );

        // (define-kb-node! ID TITLE BODY)
        let s = shared.clone();
        vm.register_fn(
            "define-kb-node!",
            "Register a KB node from Scheme",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "define-kb-node!")?;
                let title = arg_string(args, 1, "define-kb-node!")?;
                let body = arg_string(args, 2, "define-kb-node!")?;
                s.lock().pending_kb_nodes.push((id, title, body));
                Ok(Value::Void)
            },
        );

        // (kb-agenda FILTER [ARGS]) — dispatch graph agenda query
        let s = shared.clone();
        vm.register_fn(
            "kb-agenda",
            "Query KB graph: (kb-agenda \"orphan\"), (kb-agenda \"todo\" \"TODO\")",
            Arity::Variadic(1),
            move |args: &[Value]| {
                let filter = arg_string(args, 0, "kb-agenda")?;
                let extra = args.get(1).map(|v| format!("{}", v)).unwrap_or_default();
                let cmd = if extra.is_empty() {
                    format!("kb-agenda {}", filter)
                } else {
                    format!("kb-agenda {} {}", filter, extra)
                };
                s.lock().pending_ex_commands.push(cmd);
                Ok(Value::Void)
            },
        );

        // (kb-history NODE-ID) — show version history
        let s = shared.clone();
        vm.register_fn(
            "kb-history",
            "Show version history for a KB node",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-history")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-history {}", id));
                Ok(Value::Void)
            },
        );

        // (kb-restore NODE-ID VERSION) — restore a node to a previous version
        let s = shared.clone();
        vm.register_fn(
            "kb-restore",
            "Restore a KB node to a previous version",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-restore")?;
                let version = match &args[1] {
                    Value::Int(n) => *n,
                    other => return Err(LispError::type_error("integer", format!("{}", other))),
                };
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-restore {} {}", id, version));
                Ok(Value::Void)
            },
        );

        // (kb-raw-query DATALOG) — execute raw CozoDB Datalog query
        let s = shared.clone();
        vm.register_fn(
            "kb-raw-query",
            "Execute raw CozoDB Datalog query against the KB",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let query = arg_string(args, 0, "kb-raw-query")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-raw-query {}", query));
                Ok(Value::Void)
            },
        );

        // --- Typed link functions ---

        // (kb-add-link! SOURCE-ID TARGET-ID REL-TYPE)
        let s = shared.clone();
        vm.register_fn(
            "kb-add-link!",
            "Add a typed link between KB nodes",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let src = arg_string(args, 0, "kb-add-link!")?;
                let dst = arg_string(args, 1, "kb-add-link!")?;
                let rel = arg_string(args, 2, "kb-add-link!")?;
                s.lock().pending_kb_links.push((src, dst, rel));
                Ok(Value::Void)
            },
        );

        // --- KB collaboration lifecycle (first-class, route through CollabIntent) ---

        // (kb-share [KB-NAME]) — share a KB (default = primary).
        let s = shared.clone();
        vm.register_fn(
            "kb-share",
            "Share a knowledge base for collaborative editing (default: primary KB)",
            Arity::Variadic(0),
            move |args: &[Value]| {
                let kb_name = if args.is_empty() {
                    mae_core::KB_DEFAULT_NAME.to_string()
                } else {
                    arg_string(args, 0, "kb-share")?
                };
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::Share { kb_name });
                Ok(Value::Void)
            },
        );

        // (kb-share-p2p [KB-ID]) — mint a shareable P2P join ticket ("magnet
        // link") and RETURN it (mae://join/…). Unlike (kb-share) this is a
        // synchronous daemon control-socket call, so it returns the ticket string
        // directly. Same single backend as the kb-share-p2p command + kb_share_p2p
        // MCP tool (ADR-025 §"Driving surfaces").
        let s = shared.clone();
        vm.register_fn(
            "kb-share-p2p",
            "Mint a P2P join ticket (magnet link) for a KB and return the mae://join/… string (default: primary KB).",
            Arity::Variadic(0),
            move |args: &[Value]| {
                let kb_id = if args.is_empty() {
                    mae_core::KB_DEFAULT_NAME.to_string()
                } else {
                    arg_string(args, 0, "kb-share-p2p")?
                };
                // Clone the Arc out before the blocking call so the SharedState
                // lock is not held across daemon I/O.
                let control = s.lock().daemon_control.clone();
                match control {
                    Some(c) => c
                        .mint_p2p_ticket(&kb_id)
                        .map(Value::string)
                        .map_err(|e| LispError::user(e, vec![])),
                    None => Err(LispError::user(
                        "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                            .to_string(),
                        vec![],
                    )),
                }
            },
        );

        // (kb-join-ticket TICKET) — queue a P2P join from a "magnet link" and
        // RETURN the daemon's confirmation. Synchronous daemon control-socket call;
        // the background dialer then connects + pulls the KB (after the owner
        // approves). Same single backend as the kb-join-p2p command + kb_join_p2p
        // MCP tool + `mae kb-join` CLI (ADR-025 §"Driving surfaces").
        let s = shared.clone();
        vm.register_fn(
            "kb-join-ticket",
            "Queue a P2P join from a mae://join/… ticket; the dialer pulls the KB after the owner approves.",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let ticket = arg_string(args, 0, "kb-join-ticket")?;
                let control = s.lock().daemon_control.clone();
                match control {
                    Some(c) => c
                        .join_p2p_ticket(&ticket)
                        .map(Value::string)
                        .map_err(|e| LispError::user(e, vec![])),
                    None => Err(LispError::user(
                        "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                            .to_string(),
                        vec![],
                    )),
                }
            },
        );

        // (kb-join KB-ID) — join a shared KB.
        let s = shared.clone();
        vm.register_fn(
            "kb-join",
            "Join a shared knowledge base from the daemon",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-join")?;
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::Join { kb_id });
                Ok(Value::Void)
            },
        );

        // (kb-leave KB-ID) — leave a shared KB (local copy preserved).
        let s = shared.clone();
        vm.register_fn(
            "kb-leave",
            "Leave a shared knowledge base (local copy preserved)",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-leave")?;
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::Leave { kb_id });
                Ok(Value::Void)
            },
        );

        // (kb-add-member KB-ID FINGERPRINT [ROLE]) — owner-only.
        let s = shared.clone();
        vm.register_fn(
            "kb-add-member",
            "Add a peer to a shared KB by fingerprint with a role (default editor; owner-only)",
            Arity::Variadic(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-add-member")?;
                let member = arg_string(args, 1, "kb-add-member")?;
                let role = if args.len() > 2 {
                    arg_string(args, 2, "kb-add-member")?
                } else {
                    "editor".to_string()
                };
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::AddMember {
                        kb_id,
                        member,
                        role,
                    });
                Ok(Value::Void)
            },
        );

        // (kb-remove-member KB-ID FINGERPRINT) — owner-only.
        let s = shared.clone();
        vm.register_fn(
            "kb-remove-member",
            "Remove a peer from a shared KB by fingerprint (owner-only)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-remove-member")?;
                let member = arg_string(args, 1, "kb-remove-member")?;
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::RemoveMember { kb_id, member });
                Ok(Value::Void)
            },
        );

        // (kb-approve KB-ID FINGERPRINT [ROLE]) — approve a pending join (owner-only).
        let s = shared.clone();
        vm.register_fn(
            "kb-approve",
            "Approve a pending join request by fingerprint at a role (default editor; owner-only)",
            Arity::Variadic(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-approve")?;
                let principal = arg_string(args, 1, "kb-approve")?;
                let role = if args.len() > 2 {
                    arg_string(args, 2, "kb-approve")?
                } else {
                    "editor".to_string()
                };
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::Approve {
                        kb_id,
                        principal,
                        role,
                    });
                Ok(Value::Void)
            },
        );

        // (kb-set-policy KB-ID POLICY) — restrictive|invite|permissive (owner-only).
        let s = shared.clone();
        vm.register_fn(
            "kb-set-policy",
            "Set a shared KB's join policy: restrictive | invite | permissive (owner-only)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-set-policy")?;
                let policy = arg_string(args, 1, "kb-set-policy")?;
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::SetPolicy { kb_id, policy });
                Ok(Value::Void)
            },
        );

        // (kb-block-member KB-ID FINGERPRINT) — add a principal to this daemon's LOCAL
        // self-protection blocklist (ADR-039 A2, #162). Local-only, never propagated;
        // not owner-gated (you may block even the owner).
        let s = shared.clone();
        vm.register_fn(
            "kb-block-member",
            "Locally block a principal on a KB by fingerprint (self-protection deny-list; local-only, not propagated)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-block-member")?;
                let member = arg_string(args, 1, "kb-block-member")?;
                s.lock().pending_kb_collab_actions.push(
                    mae_core::KbCollabAction::SetBlock {
                        kb_id,
                        member,
                        blocked: true,
                    },
                );
                Ok(Value::Void)
            },
        );

        // (kb-unblock-member KB-ID FINGERPRINT) — remove a principal from the LOCAL
        // self-protection blocklist (ADR-039 A2, #162).
        let s = shared.clone();
        vm.register_fn(
            "kb-unblock-member",
            "Locally unblock a principal on a KB by fingerprint (removes the local self-protection block)",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-unblock-member")?;
                let member = arg_string(args, 1, "kb-unblock-member")?;
                s.lock().pending_kb_collab_actions.push(
                    mae_core::KbCollabAction::SetBlock {
                        kb_id,
                        member,
                        blocked: false,
                    },
                );
                Ok(Value::Void)
            },
        );

        // (kb-set-encryption KB-ID MODE) — enable E2E content encryption (owner-only,
        // one-way: MODE = "e2e"). ADR-037/039.
        let s = shared.clone();
        vm.register_fn(
            "kb-set-encryption",
            "Enable E2E content encryption on an owned KB (owner-only, one-way): MODE = \"e2e\". \
Protects node CONTENT from non-members/relay; does NOT provide forward secrecy, hide metadata \
(who/when/which-node/size), or retroactively protect already-shared plaintext — enable before \
sharing. Lost identity key = permanent loss. See :help concept:kb-encryption.",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-set-encryption")?;
                let mode = arg_string(args, 1, "kb-set-encryption")?;
                s.lock()
                    .pending_kb_collab_actions
                    .push(mae_core::KbCollabAction::SetEncryption { kb_id, mode });
                Ok(Value::Void)
            },
        );

        // (kb-join-p2p TICKET) — parity alias for (kb-join-ticket): the P2P
        // join command surface is `kb-join-p2p`, so the same name resolves in
        // Scheme (principle #3 — one action, same name on every surface). Same
        // single backend (daemon control-socket `join_p2p_ticket`).
        let s = shared.clone();
        vm.register_fn(
            "kb-join-p2p",
            "Queue a P2P join from a mae://join/… ticket (alias of kb-join-ticket).",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let ticket = arg_string(args, 0, "kb-join-p2p")?;
                let control = s.lock().daemon_control.clone();
                match control {
                    Some(c) => c
                        .join_p2p_ticket(&ticket)
                        .map(Value::string)
                        .map_err(|e| LispError::user(e, vec![])),
                    None => Err(LispError::user(
                        "not connected to a daemon — start one and enable P2P with \
                         `mae setup-collab --p2p`"
                            .to_string(),
                        vec![],
                    )),
                }
            },
        );

        // --- Collab/identity ACTION primitives (first-class parity, #3) ---
        // These editor actions have a command + an MCP tool; give them a named
        // Scheme prim too (instead of forcing generic (run-command …)) so they
        // are discoverable via :help scheme:* and self-documenting. Each just
        // routes the confirmed command name through the same dispatch the
        // command surface uses (no-arg → pending_commands; arg-taking →
        // pending_ex_commands / the ex parser). The action runs on the next
        // editor-loop drain.
        macro_rules! register_collab_command_prim {
            ($name:literal, $doc:literal) => {{
                let s = shared.clone();
                vm.register_fn($name, $doc, Arity::Fixed(0), move |_args: &[Value]| {
                    s.lock().pending_commands.push($name.to_string());
                    Ok(Value::Void)
                });
            }};
        }
        register_collab_command_prim!(
            "collab-rotate-identity",
            "Rotate this peer's collab identity key across every KB it owns/belongs to (ADR-040). \
Authorize the new key on the daemon out-of-band, then reconnect."
        );
        register_collab_command_prim!(
            "collab-register-recovery-key",
            "Register an offline recovery key across your KBs (ADR-040 §Recovery-key). Back up the \
saved recovery key OFFLINE — it can later authorize a rebind if the primary is lost."
        );
        register_collab_command_prim!(
            "collab-disconnect",
            "Disconnect from the collaboration daemon."
        );
        register_collab_command_prim!(
            "collab-doctor",
            "Run collaboration connectivity diagnostics and report the results."
        );
        register_collab_command_prim!(
            "collab-list",
            "List shared documents advertised by the connected daemon."
        );
        register_collab_command_prim!(
            "collab-discover",
            "Discover MAE collaboration peers on the local network via mDNS."
        );
        register_collab_command_prim!(
            "collab-share",
            "Share the active buffer for collaborative editing (parity with the command + collab_share MCP tool)."
        );
        register_collab_command_prim!(
            "collab-sync",
            "Force a sync of shared buffers with the daemon now."
        );
        register_collab_command_prim!(
            "kb-list-remote",
            "List shared KBs advertised by the connected daemon."
        );

        // (kb-pending KB-ID) — list pending join requests for a shared KB you own
        // (the same set surfaced in kb-sharing-status). Arg-taking → ex parser.
        let s = shared.clone();
        vm.register_fn(
            "kb-pending",
            "List pending join requests for a shared KB by id (owner-only view).",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-pending")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-pending {kb_id}"));
                Ok(Value::Void)
            },
        );

        // (kb-set-ai-residency KB-ID-OR-PRIMARY POLICY) — open | local_models_only
        // (ADR-048). NOT a collab/daemon action despite living alongside the KB-sharing
        // prims above — a plain, freely-toggleable local registry field (one local user's
        // own KB, not a multi-peer trust problem), so it routes through the ex parser
        // (`dispatch_kb` in `editor/dispatch/kb.rs`) like `kb-pending` above, not through
        // `pending_kb_collab_actions`/`CollabIntent`.
        let s = shared.clone();
        vm.register_fn(
            "kb-set-ai-residency",
            "Set a KB's AI-residency policy: open | local_models_only (ADR-048). Use \"primary\" for the primary/local KB.",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let kb_id = arg_string(args, 0, "kb-set-ai-residency")?;
                let policy = arg_string(args, 1, "kb-set-ai-residency")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-set-ai-residency {kb_id} {policy}"));
                Ok(Value::Void)
            },
        );

        // (kb-set-role NODE-ID ROLE) — source | atom | molecule | hub, the molecular-note
        // classification (Source→Atom→Molecule→Hub). Also NOT a collab/daemon action —
        // routes through the ex parser like kb-set-ai-residency above.
        let s = shared.clone();
        vm.register_fn(
            "kb-set-role",
            "Set a KB node's molecular-note role: source | atom | molecule | hub.",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-set-role")?;
                let role = arg_string(args, 1, "kb-set-role")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-set-role {id} {role}"));
                Ok(Value::Void)
            },
        );

        // (collab-connect [ADDR]) — connect to a daemon; ADDR optional (defaults
        // to the configured server). Arg-taking → route through the ex parser.
        let s = shared.clone();
        vm.register_fn(
            "collab-connect",
            "Connect to a collaboration daemon. Optional ADDR (host:port) overrides the configured server.",
            Arity::Variadic(0),
            move |args: &[Value]| {
                let cmd = if args.is_empty() {
                    "collab-connect".to_string()
                } else {
                    format!("collab-connect {}", arg_string(args, 0, "collab-connect")?)
                };
                s.lock().pending_ex_commands.push(cmd);
                Ok(Value::Void)
            },
        );

        // (collab-recover-identity RECOVERY-KEY-PATH OLD-FINGERPRINT) — recover a
        // lost identity via a pre-registered offline recovery key (ADR-040
        // §Recovery-key). Arg-taking → ex parser (parsed in editor/command.rs).
        // Closes the G5 parity gap (had an MCP tool + command but no Scheme peer).
        let s = shared.clone();
        vm.register_fn(
            "collab-recover-identity",
            "Recover a lost identity via an offline recovery key: RECOVERY-KEY-PATH (dir holding the \
restored recovery id_ed25519) + OLD-FINGERPRINT (the lost key's SHA256:…). Authors a recovery-signed \
rebind so a fresh primary inherits the lost key's seats (ADR-040 §Recovery-key).",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "collab-recover-identity")?;
                let old_fp = arg_string(args, 1, "collab-recover-identity")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("collab-recover-identity {path} {old_fp}"));
                Ok(Value::Void)
            },
        );

        // (kb-remove-link! SOURCE-ID TARGET-ID)
        let s = shared.clone();
        vm.register_fn(
            "kb-remove-link!",
            "Remove a link between KB nodes",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let src = arg_string(args, 0, "kb-remove-link!")?;
                let dst = arg_string(args, 1, "kb-remove-link!")?;
                s.lock().pending_kb_link_removals.push((src, dst));
                Ok(Value::Void)
            },
        );

        // --- Meta-node functions ---

        // (kb-add-meta-member! META-ID MEMBER-ID ROLE)
        let s = shared.clone();
        vm.register_fn(
            "kb-add-meta-member!",
            "Add a member to a meta-node",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let meta = arg_string(args, 0, "kb-add-meta-member!")?;
                let member = arg_string(args, 1, "kb-add-meta-member!")?;
                let role = arg_string(args, 2, "kb-add-meta-member!")?;
                s.lock().pending_kb_meta_adds.push((meta, member, role));
                Ok(Value::Void)
            },
        );

        // (kb-remove-meta-member! META-ID MEMBER-ID)
        let s = shared.clone();
        vm.register_fn(
            "kb-remove-meta-member!",
            "Remove a member from a meta-node",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let meta = arg_string(args, 0, "kb-remove-meta-member!")?;
                let member = arg_string(args, 1, "kb-remove-meta-member!")?;
                s.lock().pending_kb_meta_removes.push((meta, member));
                Ok(Value::Void)
            },
        );

        // (kb-compose-meta META-ID) — recompose meta body from members
        let s = shared.clone();
        vm.register_fn(
            "kb-compose-meta",
            "Recompose a meta-node body from its members",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-compose-meta")?;
                s.lock()
                    .pending_ex_commands
                    .push(format!("kb-compose-meta {}", id));
                Ok(Value::Void)
            },
        );

        // --- Relationship type management ---

        // (kb-add-rel-type! NAME LABEL DESCRIPTION INVERSE DIRECTED)
        let s = shared.clone();
        vm.register_fn(
            "kb-add-rel-type!",
            "Add a custom relationship type to the KB",
            Arity::Fixed(5),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "kb-add-rel-type!")?;
                let label = arg_string(args, 1, "kb-add-rel-type!")?;
                let desc = arg_string(args, 2, "kb-add-rel-type!")?;
                let inverse = arg_string(args, 3, "kb-add-rel-type!")?;
                let directed = match &args[4] {
                    Value::Bool(b) => *b,
                    _ => true,
                };
                s.lock().pending_ex_commands.push(format!(
                    "kb-add-rel-type {} {} {} {} {}",
                    name, label, desc, inverse, directed
                ));
                Ok(Value::Void)
            },
        );

        // --- Read-only KB query functions ---

        // (kb-links-from ID) → list of (target rel-type display)
        let s = shared.clone();
        vm.register_fn(
            "kb-links-from",
            "Return outgoing typed links from a KB node",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-links-from")?;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.links_from(&id) {
                        Ok(links) => Ok(Value::list(
                            links
                                .into_iter()
                                .map(|l| {
                                    Value::list(vec![
                                        Value::string(l.dst),
                                        Value::string(l.rel_type),
                                        Value::string(l.display.unwrap_or_default()),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )),
                        Err(e) => Err(LispError::internal(format!("kb-links-from: {}", e))),
                    }
                } else {
                    Ok(Value::list(vec![]))
                }
            },
        );

        // (kb-links-to ID) → list of (source rel-type display)
        let s = shared.clone();
        vm.register_fn(
            "kb-links-to",
            "Return incoming typed links to a KB node",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-links-to")?;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.links_to(&id) {
                        Ok(links) => Ok(Value::list(
                            links
                                .into_iter()
                                .map(|l| {
                                    Value::list(vec![
                                        Value::string(l.src),
                                        Value::string(l.rel_type),
                                        Value::string(l.display.unwrap_or_default()),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )),
                        Err(e) => Err(LispError::internal(format!("kb-links-to: {}", e))),
                    }
                } else {
                    Ok(Value::list(vec![]))
                }
            },
        );

        // (kb-links-typed ID REL-TYPE) → list of (target display)
        let s = shared.clone();
        vm.register_fn(
            "kb-links-typed",
            "Return links of a specific relationship type from a node",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-links-typed")?;
                let rel_type = arg_string(args, 1, "kb-links-typed")?;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.links_typed(&id, &rel_type) {
                        Ok(links) => Ok(Value::list(
                            links
                                .into_iter()
                                .map(|l| {
                                    Value::list(vec![
                                        Value::string(l.dst),
                                        Value::string(l.display.unwrap_or_default()),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )),
                        Err(e) => Err(LispError::internal(format!("kb-links-typed: {}", e))),
                    }
                } else {
                    Ok(Value::list(vec![]))
                }
            },
        );

        // (kb-meta-members ID) → list of (member-id role order)
        let s = shared.clone();
        vm.register_fn(
            "kb-meta-members",
            "Return members of a meta-node",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-meta-members")?;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.meta_members(&id) {
                        Ok(members) => Ok(Value::list(
                            members
                                .into_iter()
                                .map(|m| {
                                    Value::list(vec![
                                        Value::string(m.member_id),
                                        Value::string(m.role),
                                        Value::Int(m.position as i64),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )),
                        Err(e) => Err(LispError::internal(format!("kb-meta-members: {}", e))),
                    }
                } else {
                    Ok(Value::list(vec![]))
                }
            },
        );

        // (kb-rel-types) → list of type names
        let s = shared.clone();
        vm.register_fn(
            "kb-rel-types",
            "Return all known relationship type names",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.known_rel_types() {
                        Ok(types) => Ok(Value::list(
                            types.into_iter().map(Value::string).collect::<Vec<_>>(),
                        )),
                        Err(e) => Err(LispError::internal(format!("kb-rel-types: {}", e))),
                    }
                } else {
                    Ok(Value::list(vec![]))
                }
            },
        );

        // (kb-get-block ID INDEX) → block text or #f
        let s = shared.clone();
        vm.register_fn(
            "kb-get-block",
            "Get a specific block from a KB node by index",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-get-block")?;
                let index = arg_int(args, 1, "kb-get-block")? as usize;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.get_block(&id, index) {
                        Ok(Some(block)) => Ok(Value::string(block.content)),
                        Ok(None) => Ok(Value::Bool(false)),
                        Err(_) => Ok(Value::Bool(false)),
                    }
                } else {
                    Ok(Value::Bool(false))
                }
            },
        );

        // (kb-block-count ID) → number of blocks
        let s = shared.clone();
        vm.register_fn(
            "kb-block-count",
            "Return the number of blocks in a KB node",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "kb-block-count")?;
                let state = s.lock();
                if let Some(ref store) = state.kb_store {
                    match store.get_blocks(&id) {
                        Ok(blocks) => Ok(Value::Int(blocks.len() as i64)),
                        Err(_) => Ok(Value::Int(0)),
                    }
                } else {
                    Ok(Value::Int(0))
                }
            },
        );

        // (deprecate-function! OLD-NAME NEW-NAME SINCE-VERSION)
        let s = shared.clone();
        vm.register_fn(
            "deprecate-function!",
            "Register a deprecation warning",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let old_name = arg_string(args, 0, "deprecate-function!")?;
                let new_name = arg_string(args, 1, "deprecate-function!")?;
                let since = arg_string(args, 2, "deprecate-function!")?;
                s.lock()
                    .deprecated_functions
                    .insert(old_name, (new_name, since));
                Ok(Value::Void)
            },
        );

        // (register-ai-tool! NAME DESCRIPTION HANDLER-FN PERMISSION)
        let s = shared.clone();
        vm.register_fn(
            "register-ai-tool!",
            "Register an AI tool from Scheme",
            Arity::Fixed(4),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "register-ai-tool!")?;
                let desc = arg_string(args, 1, "register-ai-tool!")?;
                let handler = arg_string(args, 2, "register-ai-tool!")?;
                let perm = arg_string(args, 3, "register-ai-tool!")?;
                let mut st = s.lock();
                let params = st.pending_ai_tool_params.remove(&name).unwrap_or_default();
                let required = st
                    .pending_ai_tool_required
                    .remove(&name)
                    .unwrap_or_default();
                st.pending_ai_tools.push(mae_core::SchemeToolDef {
                    name,
                    description: desc,
                    params,
                    required,
                    handler_fn: handler,
                    permission: perm,
                });
                Ok(Value::Void)
            },
        );

        // (ai-tool-param! TOOL-NAME PARAM-NAME PARAM-TYPE DESCRIPTION)
        let s = shared.clone();
        vm.register_fn(
            "ai-tool-param!",
            "Add a parameter to an AI tool",
            Arity::Fixed(4),
            move |args: &[Value]| {
                let tool = arg_string(args, 0, "ai-tool-param!")?;
                let pname = arg_string(args, 1, "ai-tool-param!")?;
                let ptype = arg_string(args, 2, "ai-tool-param!")?;
                let pdesc = arg_string(args, 3, "ai-tool-param!")?;
                s.lock()
                    .pending_ai_tool_params
                    .entry(tool)
                    .or_default()
                    .push((pname, ptype, pdesc));
                Ok(Value::Void)
            },
        );

        // (ai-tool-require! TOOL-NAME PARAM-NAME)
        let s = shared.clone();
        vm.register_fn(
            "ai-tool-require!",
            "Mark an AI tool parameter as required",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let tool = arg_string(args, 0, "ai-tool-require!")?;
                let pname = arg_string(args, 1, "ai-tool-require!")?;
                s.lock()
                    .pending_ai_tool_required
                    .entry(tool)
                    .or_default()
                    .push(pname);
                Ok(Value::Void)
            },
        );

        // (register-splash-art! NAME ART-STRING)
        let s = shared.clone();
        vm.register_fn(
            "register-splash-art!",
            "Register custom splash art",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "register-splash-art!")?;
                let art = arg_string(args, 1, "register-splash-art!")?;
                s.lock().pending_splash_arts.push((name, art, None));
                Ok(Value::Void)
            },
        );

        // (register-splash-art-image! NAME IMAGE-PATH)
        let s = shared.clone();
        vm.register_fn(
            "register-splash-art-image!",
            "Register splash art image",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "register-splash-art-image!")?;
                let path = arg_string(args, 1, "register-splash-art-image!")?;
                let mut st = s.lock();
                let resolved = {
                    let p = PathBuf::from(&path);
                    if p.is_relative() {
                        if let Some(ref dir) = st.current_module_dir {
                            dir.join(&p)
                        } else {
                            p
                        }
                    } else {
                        p
                    }
                };
                st.pending_splash_arts
                    .push((name, String::new(), Some(resolved)));
                Ok(Value::Void)
            },
        );

        // --- String utilities ---

        vm.register_fn(
            "string-split",
            "Split a string by separator",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-split")?;
                let sep = arg_string(args, 1, "string-split")?;
                Ok(Value::list(
                    s.split(&sep).map(Value::string).collect::<Vec<_>>(),
                ))
            },
        );

        vm.register_fn(
            "string-join",
            "Join a list of strings with separator",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let lst = list_to_strings(&args[0]);
                let sep = arg_string(args, 1, "string-join")?;
                Ok(Value::string(lst.join(&sep)))
            },
        );

        vm.register_fn(
            "string-trim",
            "Trim whitespace from string",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-trim")?;
                Ok(Value::string(s.trim()))
            },
        );

        vm.register_fn(
            "string-contains?",
            "Check if string contains substring",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-contains?")?;
                let sub = arg_string(args, 1, "string-contains?")?;
                Ok(Value::Bool(s.contains(&sub)))
            },
        );

        vm.register_fn(
            "string-replace",
            "Replace occurrences in string",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-replace")?;
                let from = arg_string(args, 1, "string-replace")?;
                let to = arg_string(args, 2, "string-replace")?;
                Ok(Value::string(s.replace(&from, &to)))
            },
        );

        vm.register_fn(
            "string-upcase",
            "Convert to uppercase",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-upcase")?;
                Ok(Value::string(s.to_uppercase()))
            },
        );

        vm.register_fn(
            "string-downcase",
            "Convert to lowercase",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let s = arg_string(args, 0, "string-downcase")?;
                Ok(Value::string(s.to_lowercase()))
            },
        );

        // --- Process execution ---

        vm.register_fn(
            "shell-command",
            "Execute a shell command",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let cmd = arg_string(args, 0, "shell-command")?;
                use std::process::Command;
                match Command::new("sh").arg("-c").arg(&cmd).output() {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        if stdout.len() > 1_048_576 {
                            Ok(Value::string(&stdout[..1_048_576]))
                        } else {
                            Ok(Value::string(stdout.into_owned()))
                        }
                    }
                    Err(e) => Ok(Value::string(format!("ERROR: {}", e))),
                }
            },
        );

        // --- Buffer creation/kill ---

        let s = shared.clone();
        vm.register_fn(
            "create-buffer",
            "Create a new buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "create-buffer")?;
                s.lock().pending_create_buffer = Some(name);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "kill-buffer-by-name",
            "Kill a buffer by name",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "kill-buffer-by-name")?;
                s.lock().pending_kill_buffer = Some(name);
                Ok(Value::Void)
            },
        );

        // --- Advice system ---

        let s = shared.clone();
        vm.register_fn(
            "advice-add!",
            "Add advice to a command",
            Arity::Fixed(3),
            move |args: &[Value]| {
                let command = arg_string(args, 0, "advice-add!")?;
                let kind = arg_string(args, 1, "advice-add!")?;
                let fn_name = arg_string(args, 2, "advice-add!")?;
                s.lock().pending_advice_adds.push((command, kind, fn_name));
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "advice-remove!",
            "Remove advice from a command",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let command = arg_string(args, 0, "advice-remove!")?;
                let fn_name = arg_string(args, 1, "advice-remove!")?;
                s.lock().pending_advice_removes.push((command, fn_name));
                Ok(Value::Void)
            },
        );

        // (check-deprecated NAME)
        let s = shared.clone();
        vm.register_fn(
            "check-deprecated",
            "Check if function is deprecated",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "check-deprecated")?;
                let mut state = s.lock();
                if let Some((new_name, since)) = state.deprecated_functions.get(&name).cloned() {
                    if state.deprecated_warned.insert(name.clone()) {
                        warn!(
                            "'{}' is deprecated since v{}, use '{}' instead",
                            name, since, new_name
                        );
                        state.pending_messages.push(format!(
                            "Warning: '{}' is deprecated since v{}, use '{}' instead",
                            name, since, new_name
                        ));
                    }
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(false))
                }
            },
        );

        // --- Test framework primitives ---

        let s = shared.clone();
        vm.register_fn(
            "exit",
            "Request process exit",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let code = arg_int(args, 0, "exit")?;
                s.lock().pending_exit_code = Some(code as i32);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "write-file",
            "Write a string to disk",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let path = arg_string(args, 0, "write-file")?;
                let content = arg_string(args, 1, "write-file")?;
                s.lock().pending_write_files.push((path, content));
                Ok(Value::Void)
            },
        );

        // (goto-char OFFSET)
        let s = shared.clone();
        vm.register_fn(
            "goto-char",
            "Move cursor to character offset",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let offset = arg_int(args, 0, "goto-char")?;
                s.lock().pending_cursor = Some((usize::MAX, offset.max(0) as usize));
                Ok(Value::Void)
            },
        );

        // --- Test introspection via SharedState ---

        let s = shared.clone();
        vm.register_fn(
            "current-mode",
            "Read the current mode",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(s.lock().current_mode.clone())),
        );

        // --- E2E key-injection (test harness) ---
        let s = shared.clone();
        vm.register_fn(
            "feed-keys",
            "E2E test: feed a raw key sequence (e.g. \"C-; b s\") through the real key handler",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let keys = arg_string(args, 0, "feed-keys")?;
                s.lock().pending_feed_keys.push(keys);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "which-key-open?",
            "E2E test: is the transient leader keypad / which-key popup active?",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().leader_active)),
        );

        let s = shared.clone();
        vm.register_fn(
            "which-key-entry-count",
            "E2E test: number of which-key entries for the current keymap",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().which_key_count as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-buffer-string",
            "Read active buffer text",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(s.lock().current_buffer_text.clone())),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-buffer-text",
            "Read named buffer text",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "test-buffer-text")?;
                let state = s.lock();
                match state
                    .all_buffer_texts
                    .iter()
                    .find(|(n, _)| n == &name || n.ends_with(&name))
                {
                    Some((_, t)) => Ok(Value::string(t.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "messages-buffer-text",
            "Read *messages* buffer content",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let state = s.lock();
                Ok(Value::string(
                    state
                        .all_buffer_texts
                        .iter()
                        .find(|(n, _)| n == "*messages*")
                        .map(|(_, t)| t.clone())
                        .unwrap_or_default(),
                ))
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-sync-enabled?",
            "Whether sync is enabled",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().sync_enabled)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-pending-updates",
            "Number of pending sync updates",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().pending_update_count as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-sync-content",
            "Sync doc content or #f",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().sync_content {
                Some(c) => Ok(Value::string(c.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-encode-state",
            "Encoded sync state or #f",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().encoded_state {
                Some(s) => Ok(Value::string(s.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-get-buffer-by-name",
            "Lookup buffer index by name",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "test-get-buffer-by-name")?;
                let state = s.lock();
                match state.buffer_names.iter().find(|(_, n)| n == &name) {
                    Some((i, _)) => Ok(Value::Int(*i as i64)),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-get-option",
            "Read option value from SharedState",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "test-get-option")?;
                let state = s.lock();
                match state.option_values.iter().find(|(n, _)| n == &name) {
                    Some((_, v)) => Ok(Value::string(v.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-region-active?",
            "Whether visual selection is active",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().region_active)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-region-start",
            "Start offset of visual selection",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().region_start as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-region-end",
            "End offset of visual selection",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().region_end as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-search-forward",
            "Search for pattern in active buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let pattern = arg_string(args, 0, "test-search-forward")?;
                let state = s.lock();
                match state.current_buffer_text.find(&pattern) {
                    Some(byte_offset) => {
                        let char_offset = state.current_buffer_text[..byte_offset].chars().count();
                        Ok(Value::Int(char_offset as i64))
                    }
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "test-cursor-row",
            "Cursor row (0-indexed)",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().cursor_row as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-cursor-col",
            "Cursor column (0-indexed)",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().cursor_col as i64)),
        );

        let s = shared.clone();
        vm.register_fn(
            "test-status-message",
            "Last status bar message",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(s.lock().last_status_message.clone())),
        );

        // --- CRDT/sync test primitives ---

        let s = shared.clone();
        vm.register_fn(
            "buffer-enable-sync",
            "Enable sync on active buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let client_id = arg_int(args, 0, "buffer-enable-sync")?;
                s.lock().pending_enable_sync = Some(client_id.max(1) as u64);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-disable-sync",
            "Disable sync on active buffer",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_disable_sync = true;
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-apply-update",
            "Apply encoded sync update",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let buf_name = arg_string(args, 0, "buffer-apply-update")?;
                let update_b64 = arg_string(args, 1, "buffer-apply-update")?;
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&update_b64) {
                    Ok(bytes) => {
                        s.lock().pending_sync_applies.push((buf_name, bytes));
                        Ok(Value::Bool(true))
                    }
                    Err(e) => Ok(Value::string(format!("base64 decode error: {}", e))),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-load-sync-state",
            "Load full state into active buffer",
            Arity::Fixed(2),
            move |args: &[Value]| {
                let state_b64 = arg_string(args, 0, "buffer-load-sync-state")?;
                let client_id = arg_int(args, 1, "buffer-load-sync-state")?;
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&state_b64) {
                    Ok(bytes) => {
                        s.lock().pending_load_sync_state = Some((bytes, client_id.max(1) as u64));
                        Ok(Value::Bool(true))
                    }
                    Err(e) => Ok(Value::string(format!("base64 decode error: {}", e))),
                }
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-encode-state-vector",
            "Request encoding state vector",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                s.lock().pending_encode_state_vector = true;
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-get-state-vector",
            "Retrieve encoded state vector",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().encoded_state_vector {
                Some(sv) => Ok(Value::string(sv.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-compute-diff",
            "Compute diff from remote state vector",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let sv_b64 = arg_string(args, 0, "buffer-compute-diff")?;
                s.lock().pending_compute_diff = Some(sv_b64);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-get-diff",
            "Retrieve computed diff",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().computed_diff {
                Some(d) => Ok(Value::string(d.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-reconcile-to",
            "Reconcile sync doc to target text",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let text = arg_string(args, 0, "buffer-reconcile-to")?;
                s.lock().pending_reconcile_to = Some(text);
                Ok(Value::Void)
            },
        );

        let s = shared.clone();
        vm.register_fn(
            "buffer-get-reconcile-result",
            "Retrieve reconcile result",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().reconcile_result {
                Some(r) => Ok(Value::string(r.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        // Register default values for state-injected variables
        vm.define_global("*buffer-name*", Value::string("scratch"));
        vm.define_global("*buffer-modified?*", Value::Bool(false));
        vm.define_global("*buffer-line-count*", Value::Int(0));
        vm.define_global("*buffer-char-count*", Value::Int(0));
        vm.define_global("*cursor-row*", Value::Int(1));
        vm.define_global("*cursor-col*", Value::Int(1));
        vm.define_global("*mode*", Value::string("normal"));
        vm.define_global("*shell-buffers*", Value::Null);

        // Build default load-path
        let default_load_path: Vec<PathBuf> = if let Ok(home) = std::env::var("HOME") {
            vec![
                PathBuf::from(&home)
                    .join(".config")
                    .join("mae")
                    .join("packages"),
                PathBuf::from(&home)
                    .join(".config")
                    .join("mae")
                    .join("lisp"),
            ]
        } else {
            vec![]
        };

        // Seed SharedState load_path
        {
            let mut state = shared.lock();
            state.load_path = default_load_path.clone();
        }

        Ok(SchemeRuntime {
            vm,
            shared,
            error_history: Vec::new(),
            error_seq: 0,
            max_errors: 50,
            load_path: default_load_path,
            loaded_features: HashSet::new(),
        })
    }

    /// Return declared modules from `(mae! ...)` — name → enabled flags.
    /// Empty if no `mae!` block was evaluated.
    pub fn declared_modules(&self) -> HashMap<String, Vec<String>> {
        self.shared.lock().declared_modules.clone()
    }

    /// Return declared packages from `(package! ...)`.
    pub fn declared_packages(&self) -> Vec<DeclaredPackage> {
        self.shared.lock().declared_packages.clone()
    }

    /// Set the current module directory for relative path resolution.
    /// Called by the module loader before evaluating each module's autoloads.
    pub fn set_module_dir(&mut self, dir: Option<&Path>) {
        self.shared.lock().current_module_dir = dir.map(|d| d.to_path_buf());
    }

    /// Drain pending KB nodes registered via `(define-kb-node! ...)`.
    pub fn drain_kb_nodes(&mut self) -> Vec<(String, String, String)> {
        let mut state = self.shared.lock();
        std::mem::take(&mut state.pending_kb_nodes)
    }

    /// Drain pending typed link additions: (source, target, rel_type).
    pub fn drain_kb_links(&mut self) -> Vec<(String, String, String)> {
        let mut state = self.shared.lock();
        std::mem::take(&mut state.pending_kb_links)
    }

    /// Drain pending link removals: (source, target).
    pub fn drain_kb_link_removals(&mut self) -> Vec<(String, String)> {
        let mut state = self.shared.lock();
        std::mem::take(&mut state.pending_kb_link_removals)
    }

    /// Drain pending meta-member additions: (meta_id, member_id, role).
    pub fn drain_kb_meta_adds(&mut self) -> Vec<(String, String, String)> {
        let mut state = self.shared.lock();
        std::mem::take(&mut state.pending_kb_meta_adds)
    }

    /// Drain pending meta-member removals: (meta_id, member_id).
    pub fn drain_kb_meta_removes(&mut self) -> Vec<(String, String)> {
        let mut state = self.shared.lock();
        std::mem::take(&mut state.pending_kb_meta_removes)
    }

    // --- Test framework accessors ---

    /// Take the pending exit code set by `(exit CODE)`, if any.
    pub fn take_exit_code(&mut self) -> Option<i32> {
        self.shared.lock().pending_exit_code.take()
    }

    /// Update the current mode string in SharedState (for test runner).
    pub fn set_current_mode(&self, mode: &str) {
        self.shared.lock().current_mode = mode.to_string();
    }

    /// Update the active buffer text in SharedState (for test runner).
    pub fn set_current_buffer_text(&self, text: &str) {
        self.shared.lock().current_buffer_text = text.to_string();
    }

    /// Update all buffer texts in SharedState (for test runner).
    pub fn set_all_buffer_texts(&self, texts: Vec<(String, String)>) {
        self.shared.lock().all_buffer_texts = texts;
    }

    /// Update sync state in SharedState (for test runner).
    pub fn set_sync_state(
        &self,
        enabled: bool,
        pending_count: usize,
        content: Option<String>,
        encoded: Option<String>,
    ) {
        let mut state = self.shared.lock();
        state.sync_enabled = enabled;
        state.pending_update_count = pending_count;
        state.sync_content = content;
        state.encoded_state = encoded;
    }

    /// Update buffer names in SharedState for `(get-buffer-by-name)` across tests.
    pub fn set_buffer_names(&self, names: Vec<(usize, String)>) {
        self.shared.lock().buffer_names = names;
    }

    /// Update option values in SharedState for test runner.
    pub fn set_option_values(&self, values: Vec<(String, String)>) {
        self.shared.lock().option_values = values;
    }

    /// Update region (visual selection) state in SharedState for test runner.
    pub fn set_region_state(&self, active: bool, start: usize, end: usize) {
        let mut state = self.shared.lock();
        state.region_active = active;
        state.region_start = start;
        state.region_end = end;
    }

    /// Update cursor position in SharedState (called by test runner).
    pub fn set_cursor_position(&self, row: usize, col: usize) {
        let mut state = self.shared.lock();
        state.cursor_row = row;
        state.cursor_col = col;
    }

    /// Update last status message in SharedState (called by test runner).
    pub fn set_last_status_message(&self, msg: &str) {
        self.shared.lock().last_status_message = msg.to_string();
    }

    /// Drain pending file writes from `(write-file PATH CONTENT)`.
    pub fn drain_write_files(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.shared.lock().pending_write_files)
    }

    /// Always accumulate pending sync updates from the active buffer into
    /// SharedState. Called before `drain_and_broadcast` so Scheme tests can
    /// retrieve updates via `(buffer-drain-updates)` without a two-step flag
    /// dance. Clones (not drains) so `drain_and_broadcast` still forwards them.
    pub fn capture_pending_sync_updates(&mut self, editor: &mae_core::Editor) {
        let mut state = self.shared.lock();
        let idx = editor.active_buffer_idx();
        for u in &editor.buffers[idx].pending_sync_updates {
            use base64::Engine as _;
            state
                .accumulated_sync_updates
                .push(base64::engine::general_purpose::STANDARD.encode(u));
        }
    }

    /// Update cached GC stats in SharedState for Scheme-callable `(gc-stats)`.
    fn sync_gc_stats(&self) {
        // parking_lot::Mutex::lock() returns the guard directly (no poisoning).
        let mut st = self.shared.lock();
        st.gc_stats_snapshot = self.vm.gc_stats.clone();
    }

    /// Evaluate a Scheme expression and return the result as a string.
    /// Errors are recorded in the error history for debugger introspection.
    pub fn eval(&mut self, code: &str) -> Result<String, SchemeError> {
        debug!(code_len = code.len(), "scheme eval");
        let result = self.vm.eval(code).map_err(|e| {
            let err = SchemeError::from(e);
            error!(error = %err.message, code_preview = &code[..code.len().min(100)], "scheme eval error");
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
        self.sync_gc_stats();
        Ok(value_to_display(&result))
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
        let file = path.to_string_lossy();
        self.vm.eval_with_file(&content, &file).map_err(|e| {
            let err = SchemeError::from(e);
            error!(path = %path.display(), error = %err.message, "scheme file evaluation failed");
            err
        })?;
        Ok(())
    }

    /// Evaluate Scheme source already in memory, using `virtual_name` as the
    /// file label for error messages / stack frames (e.g.
    /// `"embedded:keymap-doom/autoloads.scm"`). This is the in-memory twin of
    /// [`load_file`](Self::load_file) — used to load modules embedded in the
    /// binary without touching the filesystem.
    pub fn load_source(&mut self, content: &str, virtual_name: &str) -> Result<(), SchemeError> {
        info!(file = %virtual_name, "loading scheme source");
        self.vm.eval_with_file(content, virtual_name).map_err(|e| {
            let err = SchemeError::from(e);
            error!(file = %virtual_name, error = %err.message, "scheme source evaluation failed");
            err
        })?;
        Ok(())
    }

    /// Evaluate Scheme code, returning yield requests to the caller
    /// instead of blocking. The caller handles the yield (sleep, wait-for-file)
    /// and calls `resume_yield(value)` to continue.
    ///
    /// Returns `Ok(SchemeEvalResult::Done(display_string))` when evaluation completes,
    /// or `Ok(SchemeEvalResult::Yield(request))` when the VM wants to suspend.
    pub fn eval_yielding(&mut self, code: &str) -> Result<SchemeEvalResult, SchemeError> {
        debug!(code_len = code.len(), "scheme eval_yielding");
        use crate::vm::EvalResult;
        match self.vm.eval_yielding(code) {
            Ok(EvalResult::Done(v)) => Ok(SchemeEvalResult::Done(value_to_display(&v))),
            Ok(EvalResult::Yield(req)) => {
                debug!(request = ?req, "scheme eval yielded");
                Ok(SchemeEvalResult::Yield(req))
            }
            Err(e) => {
                let err = SchemeError::from(e);
                error!(error = %err.message, "scheme eval_yielding error");
                self.record_error(code, &err);
                Err(err)
            }
        }
    }

    /// Resume execution after handling a yield request.
    /// `resume_value` is the result pushed onto the stack (typically `#t`).
    pub fn resume_yield(&mut self, resume_value: Value) -> Result<SchemeEvalResult, SchemeError> {
        debug!("scheme resume_yield");
        use crate::vm::EvalResult;
        match self.vm.resume(resume_value) {
            Ok(EvalResult::Done(v)) => Ok(SchemeEvalResult::Done(value_to_display(&v))),
            Ok(EvalResult::Yield(req)) => {
                debug!(request = ?req, "scheme resume yielded again");
                Ok(SchemeEvalResult::Yield(req))
            }
            Err(e) => {
                let err = SchemeError::from(e);
                error!(error = %err.message, "scheme resume error");
                Err(err)
            }
        }
    }

    // --- Introspection API (Phase 13h) ---

    /// Describe a function by name. Returns formatted documentation.
    pub fn describe_function(&self, name: &str) -> Option<String> {
        crate::introspect::describe_function(&self.vm, name)
            .map(|d| crate::introspect::format_doc(&d))
    }

    /// Search for functions matching a pattern.
    pub fn apropos(&self, pattern: &str) -> Vec<crate::introspect::FunctionDoc> {
        crate::introspect::apropos(&self.vm, pattern)
    }

    /// Get the full function registry.
    pub fn function_registry(&self) -> Vec<crate::introspect::FunctionDoc> {
        crate::introspect::function_registry(&self.vm)
    }

    /// Get current GC statistics.
    pub fn gc_stats(&self) -> crate::vm::GcStats {
        self.vm.gc_stats.clone()
    }

    /// Update the Editor's cached scheme stats for MCP introspection.
    pub fn update_editor_scheme_stats(&self, editor: &mut mae_core::Editor) {
        let stats = &self.vm.gc_stats;
        editor.scheme_stats.eval_count = stats.eval_count;
        editor.scheme_stats.collections_count = stats.collections_count;
        editor.scheme_stats.globals_count = stats.globals_count;
        editor.scheme_stats.stack_hwm = stats.stack_hwm;
        editor.scheme_stats.function_count = crate::introspect::function_registry(&self.vm).len();
        editor.scheme_stats.error_count = self.error_history.len();
    }

    /// Generate KB node data for all registered functions.
    /// Returns (id, title, body, tags) tuples for insertion into KB.
    pub fn kb_function_nodes(&self) -> Vec<(String, String, String, Vec<String>)> {
        let mut nodes = Vec::new();
        for doc in crate::introspect::function_registry(&self.vm) {
            let id = format!("scheme:{}", doc.name);
            let title = format!("Scheme: {}", doc.name);
            let kind_str = doc.kind.to_string();
            let arity_str = doc.arity.to_string();

            let mut body = format!("## Signature\n```scheme\n({}", doc.name);
            match &doc.arity {
                crate::lisp_error::Arity::Fixed(n) => {
                    for i in 0..*n {
                        body.push_str(&format!(" arg{}", i + 1));
                    }
                }
                crate::lisp_error::Arity::Variadic(n) => {
                    for i in 0..*n {
                        body.push_str(&format!(" arg{}", i + 1));
                    }
                    body.push_str(" . rest");
                }
                crate::lisp_error::Arity::Multi(ns) => {
                    body.push_str(&format!(
                        " <{}>",
                        ns.iter()
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                }
            }
            body.push_str(")\n```\n\n");

            if !doc.doc.is_empty() {
                body.push_str(&format!("{}\n\n", doc.doc));
            }

            body.push_str(&format!(
                "**Kind:** {}\n**Arity:** {}\n\n",
                kind_str, arity_str
            ));

            if let Some(ref file) = doc.source_file {
                if let Some(line) = doc.source_line {
                    body.push_str(&format!("**Source:** {}:{}\n\n", file, line));
                }
            }

            body.push_str("See also: [[concept:scheme-api]], [[index]]");

            let tags = vec![
                "scheme".to_string(),
                "api".to_string(),
                kind_str.to_string(),
            ];
            nodes.push((id, title, body, tags));
        }
        nodes
    }

    /// Record an error in the error history.
    fn record_error(&mut self, code: &str, err: &SchemeError) {
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
    }

    /// Inject read-only buffer information as Scheme globals.
    /// Call this before eval to give Scheme access to current editor state.
    pub fn inject_editor_state(&mut self, editor: &Editor) {
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

        // *buffer-list*
        let buf_info: Vec<Value> = editor
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                Value::list(vec![
                    Value::Int(i as i64),
                    Value::string(b.name.clone()),
                    Value::string(format!("{:?}", b.kind)),
                    Value::Bool(b.modified),
                ])
            })
            .collect();
        self.vm
            .define_global("*buffer-list*", Value::list(buf_info));

        // (get-buffer-by-name NAME) — reads from SharedState for always-fresh data
        let s = self.shared.clone();
        self.vm.register_fn(
            "get-buffer-by-name",
            "Get buffer index by name",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "get-buffer-by-name")?;
                let state = s.lock();
                match state.buffer_names.iter().find(|(_, n)| n == &name) {
                    Some((i, _)) => Ok(Value::Int(*i as i64)),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // *window-count*
        self.vm.define_global(
            "*window-count*",
            Value::Int(editor.window_mgr.window_count() as i64),
        );

        // *window-list*
        let win_info: Vec<Value> = editor
            .window_mgr
            .iter_windows()
            .map(|w| {
                Value::list(vec![
                    Value::Int(w.id as i64),
                    Value::Int(w.buffer_idx as i64),
                    Value::Int(w.cursor_row as i64),
                    Value::Int(w.cursor_col as i64),
                ])
            })
            .collect();
        self.vm
            .define_global("*window-list*", Value::list(win_info));

        // *option-list*
        let opt_info: Vec<Value> = editor
            .option_registry
            .list()
            .iter()
            .map(|o| {
                Value::list(vec![
                    Value::string(o.name.as_ref()),
                    Value::string(format!("{}", o.kind)),
                    Value::string(o.default_value.as_ref()),
                    Value::string(o.doc.as_ref()),
                ])
            })
            .collect();
        self.vm
            .define_global("*option-list*", Value::list(opt_info));

        // Populate SharedState option_values
        {
            let values: Vec<(String, String)> = editor
                .option_registry
                .list()
                .iter()
                .filter_map(|o| {
                    editor
                        .get_option(&o.name)
                        .map(|(v, _)| (o.name.to_string(), v))
                })
                .collect();
            self.shared.lock().option_values = values;
        }

        // (get-option NAME)
        let s = self.shared.clone();
        self.vm.register_fn(
            "get-option",
            "Get current option value",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "get-option")?;
                let state = s.lock();
                match state.option_values.iter().find(|(n, _)| n == &name) {
                    Some((_, v)) => Ok(Value::string(v.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // *command-list*
        let cmd_info: Vec<Value> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| {
                Value::list(vec![
                    Value::string(c.name.clone()),
                    Value::string(c.doc.clone()),
                    Value::string(format!("{:?}", c.source)),
                ])
            })
            .collect();
        self.vm
            .define_global("*command-list*", Value::list(cmd_info));

        // (command-exists? NAME)
        let cmd_names: Vec<String> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        self.vm.register_fn(
            "command-exists?",
            "Check if command exists",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "command-exists?")?;
                Ok(Value::Bool(cmd_names.iter().any(|n| n == &name)))
            },
        );

        // *keymap-list*
        let keymap_names: Vec<Value> = editor
            .keymaps
            .keys()
            .map(|k| Value::string(k.clone()))
            .collect();
        self.vm
            .define_global("*keymap-list*", Value::list(keymap_names));

        // (keymap-bindings MAP-NAME)
        let keymaps_snapshot: std::collections::HashMap<String, Vec<(String, String)>> = editor
            .keymaps
            .iter()
            .map(|(name, km)| {
                let bindings: Vec<(String, String)> = km
                    .bindings()
                    .map(|(seq, cmd)| (mae_core::keymap::serialize_macro(seq), cmd.clone()))
                    .collect();
                (name.clone(), bindings)
            })
            .collect();
        self.vm.register_fn(
            "keymap-bindings",
            "List bindings for a keymap",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "keymap-bindings")?;
                Ok(keymaps_snapshot
                    .get(&name)
                    .map(|bindings| {
                        Value::list(
                            bindings
                                .iter()
                                .map(|(k, c)| {
                                    Value::list(vec![
                                        Value::string(k.clone()),
                                        Value::string(c.clone()),
                                    ])
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .unwrap_or(Value::Null))
            },
        );

        // (buffer-string) — reads from SharedState for always-fresh data
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-string",
            "Full text of active buffer",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(s.lock().current_buffer_text.clone())),
        );

        // (buffer-text NAME)
        {
            let all_buf_texts: Vec<(String, String)> = editor
                .buffers
                .iter()
                .map(|b| (b.name.clone(), b.text()))
                .collect();
            self.shared.lock().all_buffer_texts = all_buf_texts;
        }
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-text",
            "Full text of named buffer",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let name = arg_string(args, 0, "buffer-text")?;
                let state = s.lock();
                match state
                    .all_buffer_texts
                    .iter()
                    .find(|(n, _)| n == &name || n.ends_with(&name))
                {
                    Some((_, t)) => Ok(Value::string(t.clone())),
                    None => Ok(Value::Bool(false)),
                }
            },
        );

        // (collab-status)
        let collab_status_str = editor.collab.status.as_str().to_string();
        let collab_server_addr = editor.collab.server_address.clone();
        let collab_synced_docs = editor.collab.synced_docs;
        self.vm.register_fn(
            "collab-status",
            "Current collaboration state",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(vec![
                    Value::list(vec![
                        Value::string("status"),
                        Value::string(collab_status_str.clone()),
                    ]),
                    Value::list(vec![
                        Value::string("server"),
                        Value::string(collab_server_addr.clone()),
                    ]),
                    Value::list(vec![
                        Value::string("synced-docs"),
                        Value::Int(collab_synced_docs as i64),
                    ]),
                    Value::list(vec![Value::string("peer-count"), Value::Int(0)]),
                ]))
            },
        );

        // (kb-sharing-status) — JSON snapshot of this peer's KB-sharing state
        // (shared KBs with members + roles, policy, pending requests, my role +
        // epoch, sync status). The SAME snapshot the `*KB Sharing*` buffer and
        // the `kb_sharing_status` MCP tool expose (CLAUDE.md #3 the AI is a peer,
        // #8 one builder). Re-captured each sync so it stays fresh. Returns a JSON
        // string (parse it scheme-side); `{}` if serialization fails.
        let kb_sharing_json = editor.kb_sharing_snapshot_json();
        self.vm.register_fn(
            "kb-sharing-status",
            "JSON snapshot of this peer's KB-sharing state (members, roles, policy, pending, my role/epoch).",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(kb_sharing_json.clone())),
        );

        // --- Daemon capability parity (ADR-035) ---
        // The human (commands/buffers), the AI peer (MCP `daemon_status`), and
        // Scheme all read the SAME capability model: is a daemon present, and is a
        // given daemon-dependent feature available right now (with the why + fix).
        // Re-captured each sync so it stays fresh, like kb-sharing-status above.

        // (daemon-available?) — is a daemon present (control or read layer)?
        let daemon_present = editor.daemon_available();
        self.vm.register_fn(
            "daemon-available?",
            "Whether a daemon is present (control or read layer) right now.",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(daemon_present)),
        );

        // (daemon-status) — JSON: daemon state + per-feature availability.
        let daemon_status_json = editor.daemon_status_json();
        self.vm.register_fn(
            "daemon-status",
            "JSON snapshot of daemon state + per-feature availability (ADR-035 capability model).",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::string(daemon_status_json.clone())),
        );

        // (feature-available? FEATURE-ID) — JSON availability for one feature
        // (e.g. "p2p-sharing", "continuous-sync", "kb-hosting"). Per-id JSON is
        // precomputed at registration; the closure looks the id up.
        let feature_json: std::collections::HashMap<String, String> =
            mae_core::editor::DaemonFeature::ALL
                .iter()
                .map(|f| (f.id().to_string(), editor.feature_availability_json(f.id())))
                .collect();
        let feature_ids: Vec<String> = mae_core::editor::DaemonFeature::ALL
            .iter()
            .map(|f| f.id().to_string())
            .collect();
        self.vm.register_fn(
            "feature-available?",
            "JSON availability of a daemon-dependent feature by id (ADR-035 capability model).",
            Arity::Fixed(1),
            move |args: &[Value]| {
                let id = arg_string(args, 0, "feature-available?")?;
                let norm = id.trim().to_ascii_lowercase().replace('_', "-");
                let json = feature_json.get(&norm).cloned().unwrap_or_else(|| {
                    let known = feature_ids
                        .iter()
                        .map(|i| format!("\"{i}\""))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("{{\"error\":\"unknown feature '{id}'\",\"known\":[{known}]}}")
                });
                Ok(Value::string(json))
            },
        );

        // (collab-synced-buffers)
        let synced_names: Vec<String> = editor.collab.synced_buffers.iter().cloned().collect();
        self.vm.register_fn(
            "collab-synced-buffers",
            "List synced buffer names",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(
                    synced_names
                        .iter()
                        .map(|n| Value::string(n.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );

        // (collab-confirmed-shares) — doc IDs confirmed by the server.
        // Unlike collab-synced-buffers which is optimistically updated on intent
        // drain, this only contains doc IDs after BufferShared/BufferJoined events.
        let confirmed: Vec<String> = editor.collab.confirmed_shares.iter().cloned().collect();
        self.vm.register_fn(
            "collab-confirmed-shares",
            "List doc IDs confirmed by the server",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                Ok(Value::list(
                    confirmed
                        .iter()
                        .map(|n| Value::string(n.clone()))
                        .collect::<Vec<_>>(),
                ))
            },
        );

        // --- Sync/CRDT state --- reads from SharedState for always-fresh data

        let sync_enabled = buf.sync_doc.is_some();
        self.vm
            .define_global("*buffer-sync-enabled?*", Value::Bool(sync_enabled));
        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-sync-enabled?",
            "Whether sync is enabled",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(s.lock().sync_enabled)),
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-pending-updates",
            "Number of pending sync updates",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Int(s.lock().pending_update_count as i64)),
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-sync-content",
            "Sync doc content",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().sync_content {
                Some(c) => Ok(Value::string(c.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-drain-updates",
            "Take accumulated sync updates",
            Arity::Fixed(0),
            move |_args: &[Value]| {
                let mut state = s.lock();
                let updates = std::mem::take(&mut state.accumulated_sync_updates);
                Ok(Value::list(
                    updates.into_iter().map(Value::string).collect::<Vec<_>>(),
                ))
            },
        );

        let s = self.shared.clone();
        self.vm.register_fn(
            "buffer-encode-state",
            "Full yrs document state as base64",
            Arity::Fixed(0),
            move |_args: &[Value]| match &s.lock().encoded_state {
                Some(st) => Ok(Value::string(st.clone())),
                None => Ok(Value::Bool(false)),
            },
        );

        let has_undo = buf.has_undo();
        self.vm.register_fn(
            "undo-available?",
            "Whether undo stack is non-empty",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(has_undo)),
        );

        let has_redo = buf.has_redo();
        self.vm.register_fn(
            "redo-available?",
            "Whether redo stack is non-empty",
            Arity::Fixed(0),
            move |_args: &[Value]| Ok(Value::Bool(has_redo)),
        );

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
    pub fn apply_to_editor(&mut self, editor: &mut Editor) {
        let mut state = self.shared.lock();

        // Create new keymaps (must come before bindings so define-key can target them)
        for (name, parent) in state.keymap_defs.drain(..) {
            if !editor.keymaps.contains_key(&name) {
                debug!(keymap = %name, parent = %parent, "creating scheme keymap");
                editor
                    .keymaps
                    .insert(name.clone(), mae_core::Keymap::with_parent(&name, &parent));
            }
        }

        // Apply context routing (buffer kind / language -> context keymap).
        for (sel_type, sel_value, keymap) in state.context_bindings.drain(..) {
            if let Err(e) = editor
                .keymap_registry
                .apply_binding(&sel_type, &sel_value, &keymap)
            {
                warn!(
                    selector_type = %sel_type,
                    selector_value = %sel_value,
                    keymap = %keymap,
                    "ignoring bind-context-keymap: {e}"
                );
            }
        }

        // Apply keymap bindings
        let binding_count = state.keymap_bindings.len();
        for (map_name, key_str, cmd_name) in state.keymap_bindings.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&key_str);
                if seq.is_empty() {
                    warn!(keymap = %map_name, key = %key_str, command = %cmd_name,
                          "scheme keybinding produced empty key sequence, skipping");
                } else {
                    let prev = keymap.bind(seq, &cmd_name);
                    if let Some(ref prev_cmd) = prev {
                        if prev_cmd != &cmd_name {
                            warn!(keymap = %map_name, key = %key_str, command = %cmd_name,
                                   previous = %prev_cmd, "keybinding conflict: overwriting");
                            editor.message_log.push(
                                mae_core::MessageLevel::Warn,
                                "keybinding",
                                format!(
                                    "Key conflict in '{}': {} was '{}', now '{}' (module load order)",
                                    map_name, key_str, prev_cmd, cmd_name
                                ),
                            );
                        }
                    } else {
                        debug!(keymap = %map_name, key = %key_str, command = %cmd_name,
                               "applying scheme keybinding");
                    }
                }
            } else {
                warn!(keymap = %map_name, key = %key_str, command = %cmd_name, "scheme keybinding targets unknown keymap");
            }
        }

        // Register Scheme-defined commands
        let cmd_count = state.command_defs.len();
        for (name, doc, scheme_fn) in state.command_defs.drain(..) {
            debug!(command = %name, scheme_fn = %scheme_fn, "registering scheme command");
            let overwrote = editor.commands.register_scheme(&name, &doc, &scheme_fn);
            if overwrote {
                editor.message_log.push(
                    mae_core::MessageLevel::Warn,
                    "command",
                    format!(
                        "Module overrides builtin command '{}' with Scheme function '{}'",
                        name, scheme_fn
                    ),
                );
            }
        }

        // Register autoload commands
        for (cmd_name, feature, doc) in state.pending_autoloads.drain(..) {
            debug!(command = %cmd_name, feature = %feature, "registering autoload command");
            editor.commands.register_autoload(&cmd_name, &doc, &feature);
        }

        // Register dynamic options from (define-option!)
        for (name, kind_str, default, doc) in state.pending_dynamic_options.drain(..) {
            let kind = match kind_str.as_str() {
                "bool" | "boolean" => mae_core::options::OptionKind::Bool,
                "int" | "integer" => mae_core::options::OptionKind::Int,
                "string" => mae_core::options::OptionKind::String,
                other => {
                    warn!(name = %name, kind = %other, "define-option! unknown kind, defaulting to string");
                    mae_core::options::OptionKind::String
                }
            };
            editor
                .option_registry
                .register_dynamic(name.clone(), vec![], doc, kind, default, None);
            debug!(option = %name, "registered dynamic option from module");
        }

        // Unregister commands (for module unload)
        for name in state.pending_command_unregisters.drain(..) {
            if editor.commands.unregister(&name) {
                debug!(command = %name, "unregistered command");
            }
        }

        // Unregister options (for module unload)
        for name in state.pending_option_unregisters.drain(..) {
            if editor.option_registry.unregister(&name) {
                debug!(option = %name, "unregistered option");
            }
        }

        // Apply hook registrations
        for (hook, fn_name) in state.pending_hook_adds.drain(..) {
            editor.hooks.add(&hook, &fn_name);
            debug!(hook = %hook, fn_name = %fn_name, "hook registered");
        }
        for (hook, fn_name) in state.pending_hook_removes.drain(..) {
            if editor.hooks.remove(&hook, &fn_name) {
                debug!(hook = %hook, fn_name = %fn_name, "hook removed");
            }
        }

        // Apply display-rule overrides from (set-display-rule!)
        for (kind_str, action_str) in state.pending_display_rules.drain(..) {
            use mae_core::display_policy::{parse_action, parse_buffer_kind};
            match (parse_buffer_kind(&kind_str), parse_action(&action_str)) {
                (Some(kind), Some(action)) => {
                    editor.display_policy.set_override(kind, action);
                    debug!(kind = %kind_str, action = %action_str, "display rule override applied");
                }
                _ => {
                    warn!(kind = %kind_str, action = %action_str, "invalid set-display-rule! args");
                    editor.set_status(format!(
                        "Invalid display rule: kind='{}', action='{}'",
                        kind_str, action_str
                    ));
                }
            }
        }

        // Apply replaceable-kind changes from (set-buffer-kind-replaceable!)
        for (kind_str, enable) in state.pending_replaceable_kinds.drain(..) {
            use mae_core::display_policy::parse_buffer_kind;
            match parse_buffer_kind(&kind_str) {
                Some(kind) => {
                    if enable {
                        if !editor.replaceable_kinds.contains(&kind) {
                            editor.replaceable_kinds.push(kind);
                        }
                    } else {
                        editor.replaceable_kinds.retain(|k| *k != kind);
                    }
                    debug!(kind = %kind_str, enable = %enable, "replaceable kind updated");
                }
                None => {
                    warn!(kind = %kind_str, "invalid set-buffer-kind-replaceable! arg");
                    editor.set_status(format!("Unknown buffer kind: '{}'", kind_str));
                }
            }
        }

        // Apply KB nodes registered from Scheme via (define-kb-node! ID TITLE BODY)
        for (id, title, body) in state.pending_kb_nodes.drain(..) {
            let node = mae_core::KbNode::new(id.clone(), title, mae_core::KbNodeKind::Note, body)
                .with_tags(["scheme"]);
            editor.kb.primary.insert(node);
            debug!(id = %id, "kb node registered from scheme");
        }

        // Apply KB collaboration lifecycle actions from `(kb-share)` etc. — lowered
        // to the SAME CollabIntent the commands + MCP tools use.
        for action in state.pending_kb_collab_actions.drain(..) {
            editor.queue_kb_collab_action(action);
        }

        // Apply typed link additions from (kb-add-link! SRC DST REL_TYPE)
        if let Some(ref store) = editor.kb.store {
            for (src, dst, rel_type) in state.pending_kb_links.drain(..) {
                if let Err(e) = store.add_typed_link(&src, &dst, &rel_type, 1.0) {
                    warn!(src = %src, dst = %dst, rel = %rel_type, "kb-add-link! error: {}", e);
                } else {
                    debug!(src = %src, dst = %dst, rel = %rel_type, "typed link added from scheme");
                }
            }
            for (src, dst) in state.pending_kb_link_removals.drain(..) {
                if let Err(e) = store.remove_link(&src, &dst) {
                    warn!(src = %src, dst = %dst, "kb-remove-link! error: {}", e);
                } else {
                    debug!(src = %src, dst = %dst, "link removed from scheme");
                }
            }
            for (meta_id, member_id, role) in state.pending_kb_meta_adds.drain(..) {
                if let Err(e) = store.add_meta_member(&meta_id, &member_id, 0, &role) {
                    warn!(meta = %meta_id, member = %member_id, "kb-add-meta-member! error: {}", e);
                } else {
                    debug!(meta = %meta_id, member = %member_id, role = %role, "meta member added from scheme");
                }
            }
            for (meta_id, member_id) in state.pending_kb_meta_removes.drain(..) {
                if let Err(e) = store.remove_meta_member(&meta_id, &member_id) {
                    warn!(meta = %meta_id, member = %member_id, "kb-remove-meta-member! error: {}", e);
                } else {
                    debug!(meta = %meta_id, member = %member_id, "meta member removed from scheme");
                }
            }
        } else {
            // No store — just drain to avoid accumulating
            state.pending_kb_links.clear();
            state.pending_kb_link_removals.clear();
            state.pending_kb_meta_adds.clear();
            state.pending_kb_meta_removes.clear();
        }

        // Apply editor options via the OptionRegistry (single source of truth)
        for (key, value) in state.pending_options.drain(..) {
            match editor.set_option(&key, &value) {
                Ok(_) => {}
                Err(e) => {
                    warn!(key = key.as_str(), "set-option! error: {}", e);
                    editor.set_status(e);
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
            editor.fire_hook("after-insert");
        }

        // (cursor-goto ROW COL) or (goto-char OFFSET)
        if let Some((row, col)) = state.pending_cursor.take() {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            if row == usize::MAX {
                // goto-char mode: col holds the char offset
                let offset = col.min(editor.buffers[idx].rope().len_chars());
                let rope = editor.buffers[idx].rope();
                let new_row = rope.char_to_line(offset);
                let line_start = rope.line_to_char(new_row);
                win.cursor_row = new_row;
                win.cursor_col = offset.saturating_sub(line_start);
            } else {
                win.cursor_row = row;
                win.cursor_col = col;
            }
            win.clamp_cursor(&editor.buffers[idx]);
        }

        // (open-file PATH)
        if let Some(path) = state.pending_open_file.take() {
            editor.open_file(&path);
        }

        // --- Round 2: buffer editing primitives ---

        // (buffer-delete-range START END)
        if let Some((start, end)) = state.pending_delete_range.take() {
            let idx = editor.active_buffer_idx();
            let len = editor.buffers[idx].rope().len_chars();
            let start = start.min(len);
            let end = end.min(len);
            if start < end {
                editor.buffers[idx].delete_range(start, end);
                editor.fire_hook("after-delete");
            }
        }

        // (buffer-replace-range START END TEXT)
        if let Some((start, end, text)) = state.pending_replace_range.take() {
            let idx = editor.active_buffer_idx();
            let len = editor.buffers[idx].rope().len_chars();
            let start = start.min(len);
            let end = end.min(len);
            if start <= end {
                if start < end {
                    editor.buffers[idx].delete_range(start, end);
                }
                editor.buffers[idx].insert_text_at(start, &text);
            }
        }

        // (create-buffer NAME)
        if let Some(name) = state.pending_create_buffer.take() {
            let mut buf = mae_core::Buffer::new();
            buf.name = name;
            editor.buffers.push(buf);
            let new_idx = editor.buffers.len() - 1;
            editor.display_buffer(new_idx);
        }

        // (kill-buffer-by-name NAME)
        if let Some(name) = state.pending_kill_buffer.take() {
            if let Some(idx) = editor.buffers.iter().position(|b| b.name == name) {
                if editor.buffers.len() > 1 {
                    editor.buffers.remove(idx);
                    editor.notify_buffer_removed(idx);
                    for w in editor.window_mgr.iter_windows_mut() {
                        if w.buffer_idx == idx {
                            w.buffer_idx = 0;
                        } else if w.buffer_idx > idx {
                            w.buffer_idx -= 1;
                        }
                    }
                }
            }
        }

        // Apply advice registrations
        for (command, kind_str, fn_name) in state.pending_advice_adds.drain(..) {
            let kind = match kind_str.as_str() {
                ":before" | "before" => mae_core::hooks::AdviceKind::Before,
                ":after" | "after" => mae_core::hooks::AdviceKind::After,
                other => {
                    warn!(kind = %other, "advice-add! unknown kind, defaulting to :before");
                    mae_core::hooks::AdviceKind::Before
                }
            };
            editor.hooks.add_advice(&command, kind, &fn_name);
            debug!(command = %command, kind = %kind_str, fn_name = %fn_name, "advice registered");
        }

        // Apply advice removals
        for (command, fn_name) in state.pending_advice_removes.drain(..) {
            editor.hooks.remove_advice(&command, &fn_name);
            debug!(command = %command, fn_name = %fn_name, "advice removed");
        }

        // (buffer-undo)
        if state.pending_undo {
            state.pending_undo = false;
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].undo(win);
        }

        // (buffer-redo)
        if state.pending_redo {
            state.pending_redo = false;
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].redo(win);
        }

        // (buffer-undo-boundary)
        if state.pending_undo_boundary {
            state.pending_undo_boundary = false;
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].sync_undo_boundary();
        }

        // --- CRDT/sync operations ---

        // (buffer-enable-sync CLIENT-ID)
        if let Some(client_id) = state.pending_enable_sync.take() {
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].enable_sync(client_id);
            debug!(client_id = client_id, "sync enabled on active buffer");
        }

        // (buffer-disable-sync)
        if state.pending_disable_sync {
            state.pending_disable_sync = false;
            let idx = editor.active_buffer_idx();
            editor.buffers[idx].disable_sync();
            debug!("sync disabled on active buffer");
        }

        // (buffer-load-sync-state STATE-BYTES CLIENT-ID)
        if let Some((state_bytes, client_id)) = state.pending_load_sync_state.take() {
            let idx = editor.active_buffer_idx();
            match editor.buffers[idx].load_sync_state(&state_bytes, client_id) {
                Ok(()) => debug!(client_id = client_id, "sync state loaded on active buffer"),
                Err(e) => warn!(error = %e, "failed to load sync state"),
            }
        }

        // (buffer-drain-updates) — now handled by capture_pending_sync_updates(),
        // which must run before drain_and_broadcast in the test runner.

        // (buffer-apply-update BUFFER-NAME UPDATE-BYTES)
        let sync_applies: Vec<(String, Vec<u8>)> = std::mem::take(&mut state.pending_sync_applies);
        for (buf_name, update_bytes) in sync_applies {
            if let Some(idx) = editor.buffers.iter().position(|b| b.name == buf_name) {
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => debug!(buffer = %buf_name, "sync update applied"),
                    Err(e) => warn!(buffer = %buf_name, error = %e, "failed to apply sync update"),
                }
            } else {
                warn!(buffer = %buf_name, "buffer not found for sync update");
            }
        }

        // (buffer-encode-state-vector) — encode active buffer's state vector.
        if state.pending_encode_state_vector {
            state.pending_encode_state_vector = false;
            let idx = editor.active_buffer_idx();
            if let Some(ref sync) = editor.buffers[idx].sync_doc {
                use base64::Engine as _;
                let sv = sync.state_vector();
                state.encoded_state_vector =
                    Some(base64::engine::general_purpose::STANDARD.encode(&sv));
            } else {
                state.encoded_state_vector = None;
            }
        }

        // (buffer-compute-diff SV-BASE64) — compute diff from remote state vector.
        if let Some(sv_b64) = state.pending_compute_diff.take() {
            use base64::Engine as _;
            use mae_sync::yrs::updates::decoder::Decode;
            use mae_sync::yrs::{ReadTxn, Transact};
            let idx = editor.active_buffer_idx();
            if let Some(ref sync) = editor.buffers[idx].sync_doc {
                match base64::engine::general_purpose::STANDARD.decode(&sv_b64) {
                    Ok(sv_bytes) => {
                        let txn = sync.doc().transact();
                        match mae_sync::yrs::StateVector::decode_v1(&sv_bytes) {
                            Ok(sv) => {
                                let diff = txn.encode_state_as_update_v1(&sv);
                                state.computed_diff =
                                    Some(base64::engine::general_purpose::STANDARD.encode(&diff));
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to decode state vector");
                                state.computed_diff = None;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to base64-decode state vector");
                        state.computed_diff = None;
                    }
                }
            } else {
                state.computed_diff = None;
            }
        }

        // (buffer-reconcile-to TEXT) — reconcile sync doc to target text.
        if let Some(target) = state.pending_reconcile_to.take() {
            use base64::Engine as _;
            let idx = editor.active_buffer_idx();
            let has_sync = editor.buffers[idx].sync_doc.is_some();
            if has_sync {
                let update = editor.buffers[idx]
                    .sync_doc
                    .as_mut()
                    .unwrap()
                    .reconcile_to(&target);
                if update.is_empty() {
                    state.reconcile_result = Some(String::new());
                } else {
                    state.reconcile_result =
                        Some(base64::engine::general_purpose::STANDARD.encode(&update));
                }
                // Rebuild the buffer rope from the sync doc.
                editor.buffers[idx].rebuild_rope_from_sync();
            } else {
                state.reconcile_result = None;
            }
        }

        // (switch-to-buffer IDX)
        if let Some(idx) = state.pending_switch_buffer.take() {
            if idx < editor.buffers.len() {
                let prev = editor.active_buffer_idx();
                editor.vi.alternate_buffer_idx = Some(prev);
                editor.display_buffer(idx);
            }
        }

        // (undefine-key! MAP KEY)
        for (map_name, key_str) in state.pending_key_removals.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&key_str);
                if !seq.is_empty() {
                    keymap.unbind(&seq);
                }
            }
        }

        // (set-group-name MAP PREFIX LABEL)
        // @ai-caution: [scheme-api] set-group-name must drain in apply_to_editor alongside keymap_bindings.
        for (map_name, prefix_str, label) in state.pending_group_names.drain(..) {
            if let Some(keymap) = editor.keymaps.get_mut(&map_name) {
                let seq = parse_key_seq_spaced(&prefix_str);
                if !seq.is_empty() {
                    keymap.set_group_name(seq, &label);
                    debug!(keymap = %map_name, prefix = %prefix_str, label = %label,
                           "applying scheme group name");
                }
            }
        }

        // (run-command NAME) — dispatch each queued command.
        // We drain them outside the lock since dispatch_builtin
        // may re-enter shared state.
        let commands: Vec<String> = std::mem::take(&mut state.pending_commands);

        // (execute-ex CMD) — dispatch through ex-command parser (supports args).
        let ex_commands: Vec<String> = std::mem::take(&mut state.pending_ex_commands);

        // (message TEXT) — append to message log
        for msg in state.pending_messages.drain(..) {
            info!("[scheme] {}", msg);
        }

        // (shell-send-input BUF-IDX TEXT) — queue shell terminal input.
        for (buf_idx, text) in state.pending_shell_inputs.drain(..) {
            editor.shell.inputs.push((buf_idx, text));
        }

        // Recent files and projects
        for path in state.pending_recent_files.drain(..) {
            editor.recent_files.push(std::path::PathBuf::from(path));
        }
        for path in state.pending_recent_projects.drain(..) {
            editor.recent_projects.push(std::path::PathBuf::from(path));
        }

        // Agenda file management
        for path in state.pending_agenda_adds.drain(..) {
            editor.agenda_add_path(&path);
        }
        for path in state.pending_agenda_removes.drain(..) {
            editor.agenda_remove_path(&path);
        }
        if state.pending_agenda_list {
            state.pending_agenda_list = false;
            editor.agenda_list_paths();
        }

        // Visual buffer operations
        let visual_ops = std::mem::take(&mut state.pending_visual_ops);
        if !visual_ops.is_empty() {
            let buf_idx = editor.active_buffer_idx();
            if editor.buffers[buf_idx].kind == mae_core::BufferKind::Visual {
                if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
                    for op in visual_ops {
                        match op {
                            VisualOp::AddRect {
                                x,
                                y,
                                w,
                                h,
                                fill,
                                stroke,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Rect {
                                    x,
                                    y,
                                    w,
                                    h,
                                    fill,
                                    stroke,
                                });
                            }
                            VisualOp::AddLine {
                                x1,
                                y1,
                                x2,
                                y2,
                                color,
                                thickness,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Line {
                                    x1,
                                    y1,
                                    x2,
                                    y2,
                                    color,
                                    thickness,
                                });
                            }
                            VisualOp::AddCircle {
                                cx,
                                cy,
                                r,
                                fill,
                                stroke,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Circle {
                                    cx,
                                    cy,
                                    r,
                                    fill,
                                    stroke,
                                });
                            }
                            VisualOp::AddText {
                                x,
                                y,
                                text,
                                font_size,
                                color,
                            } => {
                                vb.add(mae_core::visual_buffer::VisualElement::Text {
                                    x,
                                    y,
                                    text,
                                    font_size,
                                    color,
                                });
                            }
                            VisualOp::Clear => vb.clear(),
                        }
                    }
                }
            }
        }

        // Buffer-local options: (set-local-option! KEY VALUE)
        for (key, value) in state.pending_local_options.drain(..) {
            match editor.set_local_option(&key, &value) {
                Ok(_) => {}
                Err(e) => {
                    warn!(key = key.as_str(), "set-local-option! error: {}", e);
                    editor.set_status(e);
                }
            }
        }

        // Scheme-registered AI tools
        let mut ai_tools: Vec<mae_core::SchemeToolDef> =
            std::mem::take(&mut state.pending_ai_tools);
        for tool in &mut ai_tools {
            // Merge any late-registered params (ai-tool-param! called after register-ai-tool!)
            if let Some(extra) = state.pending_ai_tool_params.remove(&tool.name) {
                tool.params.extend(extra);
            }
            if let Some(extra) = state.pending_ai_tool_required.remove(&tool.name) {
                tool.required.extend(extra);
            }
        }
        for tool in ai_tools {
            debug!(name = %tool.name, handler = %tool.handler_fn, "registering Scheme AI tool");
            // Upsert: replace if already registered by name
            if let Some(existing) = editor
                .ai
                .scheme_tools
                .iter_mut()
                .find(|t| t.name == tool.name)
            {
                *existing = tool;
            } else {
                editor.ai.scheme_tools.push(tool);
            }
        }

        // Custom splash arts
        for (name, art, image_path) in state.pending_splash_arts.drain(..) {
            use mae_core::render_common::splash::CustomSplashArt;
            let entry = CustomSplashArt {
                name: name.clone(),
                art,
                accent_lines: Vec::new(),
                image_path,
            };
            // Upsert by name
            if let Some(existing) = editor
                .custom_splash_arts
                .iter_mut()
                .find(|a| a.name == name)
            {
                *existing = entry;
            } else {
                editor.custom_splash_arts.push(entry);
            }
        }

        // Drop the lock before dispatching commands (which may call
        // back into Scheme via user-defined commands).
        drop(state);

        for name in commands {
            editor.dispatch_builtin(&name);
        }

        for cmd in ex_commands {
            editor.execute_command(&cmd);
        }

        if binding_count > 0 || cmd_count > 0 {
            info!(
                keybindings = binding_count,
                commands = cmd_count,
                "scheme config applied to editor"
            );
        }

        // Note: We do NOT call inject_editor_state here — the caller
        // is responsible for calling it before eval if needed.

        // Update cached scheme stats for MCP introspection
        self.update_editor_scheme_stats(editor);
    }

    /// Call a named Scheme function (for executing Scheme-backed commands).
    pub fn call_function(&mut self, name: &str) -> Result<String, SchemeError> {
        let code = format!("({})", name);
        self.eval(&code)
    }

    /// Load a feature by name: search `load_path` for `{name}.scm`, eval it,
    /// and verify `(provide ...)` was called during loading.
    pub fn require_feature(&mut self, name: &str) -> Result<(), String> {
        // Already loaded?
        if self.loaded_features.contains(name) {
            return Ok(());
        }

        // Also check SharedState (provide may have been called during a previous eval).
        {
            let state = self.shared.lock();
            if state.loaded_features.contains(name) {
                // Sync to our own set.
                drop(state);
                self.loaded_features.insert(name.to_string());
                return Ok(());
            }
        }

        let filename = format!("{}.scm", name);
        let mut found_path = None;

        for dir in &self.load_path {
            let candidate = dir.join(&filename);
            if candidate.is_file() {
                found_path = Some(candidate);
                break;
            }
        }

        let path = found_path.ok_or_else(|| {
            format!(
                "Feature '{}' not found in load-path: {:?}",
                name,
                self.load_path
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
            )
        })?;

        info!(feature = name, path = %path.display(), "requiring feature");

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        self.vm
            .eval(&content)
            .map_err(|e| format!("Error loading feature '{}': {}", name, e))?;

        // Check if provide was called during loading.
        {
            let state = self.shared.lock();
            if !state.loaded_features.contains(name) {
                return Err(format!(
                    "Feature '{}' was loaded but did not call (provide-feature \"{}\")",
                    name, name
                ));
            }
        }

        self.loaded_features.insert(name.to_string());
        info!(feature = name, "feature loaded successfully");
        Ok(())
    }

    /// Drain any `(require ...)` calls that were queued during the last eval
    /// and resolve them. Must be called after `apply_to_editor` when the
    /// engine is available.
    pub fn process_requires(&mut self) -> Vec<String> {
        let requires: Vec<String> = {
            let mut state = self.shared.lock();
            std::mem::take(&mut state.pending_requires)
        };

        // Sync load_path from SharedState (add-to-load-path! may have modified it).
        {
            let state = self.shared.lock();
            self.load_path = state.load_path.clone();
        }

        let mut errors = Vec::new();
        for feature in &requires {
            if let Err(e) = self.require_feature(feature) {
                error!(feature = feature.as_str(), error = %e, "require failed");
                errors.push(e);
            }
        }
        errors
    }

    /// Drain key sequences queued by `(feed-keys ...)`. The test runner owns the
    /// real `handle_key` pipeline, so it pulls these and dispatches each parsed
    /// key against the live editor + loaded keymaps (true E2E key injection).
    pub fn take_pending_feed_keys(&mut self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().pending_feed_keys)
    }

    // --- Debugger introspection methods ---

    /// List all Scheme-defined commands accumulated via `(define-command ...)`.
    /// Returns (name, doc, scheme_fn_name) triples.
    pub fn list_user_commands(&self) -> Vec<(String, String, String)> {
        self.shared.lock().command_defs.clone()
    }

    /// List all keybindings accumulated via `(define-key ...)`.
    /// Returns (keymap_name, key_string, command_name) triples.
    pub fn list_keybindings(&self) -> Vec<(String, String, String)> {
        self.shared.lock().keymap_bindings.clone()
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
        self.vm.define_global(name, Value::string(value));
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

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::{parse_key_seq, CommandSource, Editor};

    fn new_runtime() -> SchemeRuntime {
        SchemeRuntime::new().unwrap()
    }

    #[test]
    fn new_runtime_creates_successfully() {
        let rt = SchemeRuntime::new();
        assert!(rt.is_ok());
    }

    #[test]
    fn load_source_evaluates_in_memory_content() {
        // load_source is how embedded modules are loaded (no filesystem).
        let mut rt = new_runtime();
        rt.load_source(
            "(define embedded-test-var 42)",
            "embedded:test/autoloads.scm",
        )
        .expect("valid in-memory source should evaluate");
        let out = rt.eval("embedded-test-var").unwrap();
        assert!(
            out.contains("42"),
            "define from load_source should take effect: {out}"
        );
        // Malformed source surfaces an error rather than silently succeeding.
        assert!(
            rt.load_source("(((", "embedded:test/bad.scm").is_err(),
            "malformed in-memory source should error"
        );
    }

    #[test]
    fn eval_arithmetic() {
        let mut rt = new_runtime();
        let result = rt.eval("(+ 1 2 3)").unwrap();
        assert_eq!(result, "6");
    }

    #[test]
    fn eval_string_ops() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(string-append "hello" " " "world")"#).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn eval_boolean() {
        let mut rt = new_runtime();
        assert_eq!(rt.eval("(= 1 1)").unwrap(), "#t");
        assert_eq!(rt.eval("(= 1 2)").unwrap(), "#f");
    }

    #[test]
    fn define_key_from_scheme() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        let keymap = editor.keymaps.get("normal").unwrap();
        let seq = parse_key_seq("Q");
        assert_eq!(keymap.lookup(&seq), mae_core::LookupResult::Exact("quit"));
    }

    #[test]
    fn define_command_from_scheme() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-status "Hello from Scheme!")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.status_msg, "Hello from Scheme!");
    }

    #[test]
    fn inject_and_read_editor_state() {
        let mut rt = new_runtime();
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
        std::fs::write(&path, r#"(define-key "normal" "Q" "my-custom-save")"#).unwrap();

        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.load_file(&path).unwrap();
        rt.apply_to_editor(&mut editor);

        let keymap = editor.keymaps.get("normal").unwrap();
        assert_eq!(
            keymap.lookup(&parse_key_seq("Q")),
            mae_core::LookupResult::Exact("my-custom-save")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn define_key_spaced_sequence_from_scheme() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let result = rt.eval("(undefined-function)");
        assert!(result.is_err());
    }

    #[test]
    fn eval_error_recorded_in_history() {
        let mut rt = new_runtime();
        let _ = rt.eval("(undefined-function)");
        let errors = rt.last_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].expression.contains("undefined-function"));
        assert!(!errors[0].error_message.is_empty());
        assert_eq!(errors[0].seq, 1);
    }

    #[test]
    fn set_theme_from_scheme() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-theme "gruvbox-dark")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.theme.name, "gruvbox-dark");
    }

    #[test]
    fn list_user_commands_after_define() {
        let mut rt = new_runtime();
        rt.eval(r#"(define-command "greet" "Say hello" "greet-fn")"#)
            .unwrap();
        let cmds = rt.list_user_commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].0, "greet");
    }

    #[test]
    fn list_keybindings_after_define() {
        let mut rt = new_runtime();
        rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
        let bindings = rt.list_keybindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0], ("normal".into(), "Q".into(), "quit".into()));
    }

    #[test]
    fn define_keymap_creates_with_parent() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(define-keymap "python" "normal")"#).unwrap();
        rt.eval(r#"(define-key "python" "C-c" "run-python-buffer")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        let km = editor.keymaps.get("python").unwrap();
        assert_eq!(km.parent.as_deref(), Some("normal"));
        let seq = parse_key_seq("C-c");
        assert_eq!(
            km.lookup(&seq),
            mae_core::LookupResult::Exact("run-python-buffer")
        );
    }

    #[test]
    fn eval_for_debug_works() {
        let mut rt = new_runtime();
        let result = rt.eval_for_debug("(+ 10 20)").unwrap();
        assert_eq!(result, "30");
    }

    // --- New API surface tests ---

    #[test]
    fn buffer_text_global_available() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        assert_eq!(rt.eval("*mode*").unwrap(), "normal");
    }

    #[test]
    fn kb_lifecycle_primitives_queue_collab_intents() {
        // P2: first-class `(kb-…)` primitives route through the SAME CollabIntent
        // the commands + MCP tools use (no execute-ex strings).
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval("(kb-add-member \"team\" \"SHA256:bob\" \"viewer\")")
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(mae_core::CollabIntent::KbAddMember { kb_id, member, role })
                if kb_id == "team" && member == "SHA256:bob" && role == "viewer"
        ));

        editor.collab.pending_intent = None;
        rt.eval("(kb-set-policy \"team\" \"permissive\")").unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(mae_core::CollabIntent::KbSetPolicy { kb_id, policy })
                if kb_id == "team" && policy == "permissive"
        ));

        editor.collab.pending_intent = None;
        rt.eval("(kb-leave \"team\")").unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(matches!(
            &editor.collab.pending_intent,
            Some(mae_core::CollabIntent::LeaveKb { kb_id }) if kb_id == "team"
        ));
    }

    #[test]
    fn kb_set_ai_residency_primitive_mutates_registry_via_ex_command() {
        // ADR-048: unlike the KB-sharing primitives above, `(kb-set-ai-residency ...)` is
        // NOT a collab/daemon action — it queues a plain ex-command string
        // (`pending_ex_commands`) that `apply_to_editor` runs through the same
        // `execute_command` → `dispatch_kb` path as typing `:kb-set-ai-residency ...`
        // would, synchronously mutating the local KB registry — no `CollabIntent` involved.
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval("(kb-set-ai-residency \"primary\" \"local_models_only\")")
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(
            editor.kb.registry.primary_ai_residency,
            mae_kb::federation::AiResidency::LocalModelsOnly
        );
        assert!(
            editor.collab.pending_intent.is_none(),
            "kb-set-ai-residency must not go through the collab-intent path"
        );

        rt.eval("(kb-set-ai-residency \"primary\" \"open\")")
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(
            editor.kb.registry.primary_ai_residency,
            mae_kb::federation::AiResidency::Open
        );
    }

    #[test]
    fn kb_set_role_primitive_stamps_property_via_ex_command() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor
            .kb_create_node(
                "note:role-scheme-test",
                "Test",
                "body",
                mae_kb::NodeKind::Note,
            )
            .unwrap();

        rt.eval("(kb-set-role \"note:role-scheme-test\" \"hub\")")
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(
            editor
                .kb
                .primary
                .get("note:role-scheme-test")
                .unwrap()
                .properties
                .get("role"),
            Some(&"hub".to_string())
        );
        assert!(
            editor.collab.pending_intent.is_none(),
            "kb-set-role must not go through the collab-intent path"
        );
    }

    #[test]
    fn kb_sharing_status_primitive_returns_snapshot_json() {
        // P0: users can script KB-sharing introspection — `(kb-sharing-status)`
        // returns the same JSON snapshot the buffer + MCP tool expose.
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.collab.local_fingerprint = "mefp".to_string();
        let coll = mae_sync::kb::KbCollectionDoc::new_owned("Team", "mefp", "me");
        editor
            .collab
            .kb_collection_state
            .insert("team".to_string(), coll.encode_state());
        rt.inject_editor_state(&editor);
        let json = rt.eval("(kb-sharing-status)").unwrap();
        // The Scheme string is the JSON snapshot; it names our KB + owner role.
        assert!(json.contains("\"team\""), "snapshot names the KB: {json}");
        assert!(
            json.contains("\"owner\""),
            "snapshot shows the owner role: {json}"
        );
    }

    #[test]
    fn daemon_capability_primitives_expose_the_model() {
        // ADR-035 parity: Scheme sees the same capability model as AI + commands.
        let mut rt = new_runtime();
        let editor = Editor::new(); // no daemon wired → floor only
        rt.inject_editor_state(&editor);

        // (daemon-available?) → #f with no daemon.
        assert_eq!(rt.eval("(daemon-available?)").unwrap(), "#f");

        // (daemon-status) → JSON naming the mode + features.
        let status = rt.eval("(daemon-status)").unwrap();
        assert!(status.contains("\"mode\""), "status has mode: {status}");
        assert!(
            status.contains("p2p-sharing"),
            "status enumerates features: {status}"
        );

        // (feature-available? "p2p-sharing") → unavailable + a fix, with no daemon.
        let p2p = rt.eval("(feature-available? \"p2p-sharing\")").unwrap();
        assert!(p2p.contains("\"requirement\":\"requires\""), "p2p: {p2p}");
        assert!(
            p2p.contains("\"available\":false"),
            "p2p unavailable w/o daemon: {p2p}"
        );
        assert!(p2p.contains("\"fix\""), "p2p carries a fix: {p2p}");

        // local-kb is the floor — always available.
        let local = rt.eval("(feature-available? \"local-kb\")").unwrap();
        assert!(
            local.contains("\"available\":true"),
            "local-kb always available: {local}"
        );

        // Unknown id → an error object naming known ids.
        let bogus = rt.eval("(feature-available? \"nope\")").unwrap();
        assert!(bogus.contains("\"error\""), "unknown id errors: {bogus}");
    }

    #[test]
    fn buffer_line_function_works() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        rt.eval(r#"(buffer-insert "hello")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "hello");
    }

    #[test]
    fn cursor_goto_from_scheme() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        // search-forward-start switches to Search mode.
        rt.eval(r#"(run-command "search-forward-start")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.mode, mae_core::Mode::Search);
    }

    #[test]
    fn eval_for_repl_formats_output() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        let output = rt.eval_for_repl("(+ 1 2)", &mut editor);
        assert!(output.contains("> (+ 1 2)"));
        assert!(output.contains("; => 3"));
    }

    #[test]
    fn eval_for_repl_formats_error() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        let output = rt.eval_for_repl("(undefined-fn)", &mut editor);
        assert!(output.contains("> (undefined-fn)"));
        assert!(output.contains("; error:"));
    }

    #[test]
    fn multiple_define_keys_in_sequence() {
        let mut rt = new_runtime();
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
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(add-hook! "before-save" "my-save-fn")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.hooks.get("before-save"), &["my-save-fn"]);
    }

    #[test]
    fn remove_hook_from_scheme() {
        let mut rt = new_runtime();
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
    fn add_hook_any_name_succeeds() {
        // Hook namespace is open — modules can define custom hooks.
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(add-hook! "custom-module-hook" "fn")"#).unwrap();
        rt.apply_to_editor(&mut editor);

        assert_eq!(editor.hooks.get("custom-module-hook"), &["fn"]);
    }

    // --- set-option! tests ---

    #[test]
    fn set_option_line_numbers() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        assert!(editor.show_line_numbers); // default true

        rt.eval(r#"(set-option! "line-numbers" "false")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(!editor.show_line_numbers);
    }

    #[test]
    fn set_option_word_wrap() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        assert!(!editor.word_wrap); // default false

        rt.eval(r#"(set-option! "word-wrap" "true")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.word_wrap);
    }

    #[test]
    fn set_option_relative_line_numbers() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "relative-line-numbers" "on")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.relative_line_numbers);
    }

    #[test]
    fn set_option_theme() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "theme" "gruvbox-dark")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.theme.name, "gruvbox-dark");
    }

    #[test]
    fn set_option_show_break() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "show-break" ">> ")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.show_break, ">> ");
    }

    #[test]
    fn set_option_unknown_warns() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(set-option! "nonexistent" "value")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.status_msg.contains("Unknown option"));
    }

    // --- Shell state tests ---

    #[test]
    fn test_shell_cwd_returns_cached_value() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor
            .shell
            .viewport_cwds
            .insert(1, "/home/user".to_string());
        rt.inject_editor_state(&editor);
        let result = rt.eval("(shell-cwd 1)").unwrap();
        assert_eq!(result, "/home/user");
    }

    #[test]
    fn test_shell_read_output_returns_viewport() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor
            .shell
            .viewports
            .insert(2, vec!["$ ls".to_string(), "file.txt".to_string()]);
        rt.inject_editor_state(&editor);
        let result = rt.eval("(shell-read-output 2 10)").unwrap();
        assert!(result.contains("$ ls"));
        assert!(result.contains("file.txt"));
    }

    #[test]
    fn test_shell_list_with_buffers() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor
            .buffers
            .push(mae_core::Buffer::new_shell("*terminal*"));
        rt.inject_editor_state(&editor);
        let result = rt.eval("*shell-buffers*").unwrap();
        // Should contain the index of the shell buffer (1).
        assert!(result.contains("1"));
    }

    #[test]
    fn test_recent_files_and_projects() {
        let mut editor = Editor::new();
        let mut runtime = new_runtime();

        // Initially empty
        assert_eq!(editor.recent_files.len(), 0);
        assert_eq!(editor.recent_projects.len(), 0);

        // Evaluate scheme calls (use non-temp paths since temp dirs are rejected)
        runtime
            .eval("(recent-files-add! \"/home/testuser/test.txt\")")
            .unwrap();
        runtime
            .eval("(recent-projects-add! \"/home/testuser/project\")")
            .unwrap();

        // Apply to editor
        runtime.apply_to_editor(&mut editor);

        // Verify editor state updated
        assert_eq!(editor.recent_files.len(), 1);
        assert_eq!(
            editor.recent_files.list()[0],
            std::path::PathBuf::from("/home/testuser/test.txt")
        );
        assert_eq!(editor.recent_projects.len(), 1);
        assert_eq!(
            editor.recent_projects.list()[0],
            std::path::PathBuf::from("/home/testuser/project")
        );
    }

    // --- Round 2: buffer editing API tests ---

    #[test]
    fn buffer_text_range_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "Hello, World!");
        rt.inject_editor_state(&editor);
        let result = rt.eval("(buffer-text-range 0 5)").unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn buffer_text_range_out_of_bounds() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "Hi");
        rt.inject_editor_state(&editor);
        let result = rt.eval("(buffer-text-range 0 100)").unwrap();
        assert_eq!(result, "Hi");
    }

    #[test]
    fn buffer_delete_range_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "Hello, World!");
        rt.eval("(buffer-delete-range 5 13)").unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "Hello");
    }

    #[test]
    fn buffer_replace_range_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "Hello, World!");
        rt.eval(r#"(buffer-replace-range 7 12 "Scheme")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "Hello, Scheme!");
    }

    #[test]
    fn buffer_undo_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'A');
        }
        assert_eq!(editor.buffers[0].text(), "A");
        rt.eval("(buffer-undo)").unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "");
    }

    #[test]
    fn buffer_redo_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'X');
        }
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].undo(win);
        }
        assert_eq!(editor.buffers[0].text(), "");
        rt.eval("(buffer-redo)").unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers[0].text(), "X");
    }

    // --- Round 2: buffer list API tests ---

    #[test]
    fn buffer_list_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("(length *buffer-list*)").unwrap();
        assert!(result.parse::<i32>().unwrap() >= 1);
    }

    #[test]
    fn get_buffer_by_name_found() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(get-buffer-by-name "[scratch]")"#).unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn get_buffer_by_name_not_found() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(get-buffer-by-name "nonexistent")"#).unwrap();
        assert_eq!(result, "#f");
    }

    #[test]
    fn switch_to_buffer_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        // Add a second buffer manually
        editor.buffers.push(mae_core::Buffer::new());
        editor.buffers[1].name = "second".to_string();
        // Switch to it via Scheme, then back to 0
        editor.window_mgr.focused_window_mut().buffer_idx = 1;
        assert_eq!(editor.active_buffer_idx(), 1);
        rt.eval("(switch-to-buffer 0)").unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.active_buffer_idx(), 0);
    }

    // --- Round 2: window API tests ---

    #[test]
    fn window_count_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("*window-count*").unwrap();
        assert_eq!(result, "1");
    }

    #[test]
    fn window_list_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("(length *window-list*)").unwrap();
        assert_eq!(result, "1");
    }

    // --- Round 2: option + command introspection tests ---

    #[test]
    fn command_exists_true() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(command-exists? "save")"#).unwrap();
        assert_eq!(result, "#t");
    }

    #[test]
    fn command_exists_false() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(command-exists? "nonexistent-cmd")"#).unwrap();
        assert_eq!(result, "#f");
    }

    #[test]
    fn command_list_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("(length *command-list*)").unwrap();
        let count: i32 = result.parse().unwrap();
        assert!(
            count > 10,
            "should have many builtin commands, got {}",
            count
        );
    }

    #[test]
    fn option_list_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("(length *option-list*)").unwrap();
        let count: i32 = result.parse().unwrap();
        assert!(count >= 10, "should have many options, got {}", count);
    }

    #[test]
    fn get_option_returns_value() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(get-option "scroll_speed")"#).unwrap();
        assert_eq!(result, "3");
    }

    #[test]
    fn get_option_unknown_returns_false() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(get-option "nonexistent_option")"#).unwrap();
        assert_eq!(result, "#f");
    }

    // --- Round 2: keymap introspection tests ---

    #[test]
    fn keymap_list_available() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval("(length *keymap-list*)").unwrap();
        let count: i32 = result.parse().unwrap();
        assert!(
            count >= 2,
            "should have normal + insert keymaps, got {}",
            count
        );
    }

    #[test]
    fn keymap_bindings_returns_list() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt.eval(r#"(length (keymap-bindings "normal"))"#).unwrap();
        let count: i32 = result.parse().unwrap();
        assert!(count > 0, "normal keymap should have bindings");
    }

    #[test]
    fn keymap_bindings_unknown_returns_empty() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        let result = rt
            .eval(r#"(length (keymap-bindings "nonexistent"))"#)
            .unwrap();
        assert_eq!(result, "0");
    }

    #[test]
    fn undefine_key_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(
            editor
                .keymaps
                .get("normal")
                .unwrap()
                .lookup(&parse_key_seq("Q")),
            mae_core::LookupResult::Exact("quit")
        );
        rt.eval(r#"(undefine-key! "normal" "Q")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(
            editor
                .keymaps
                .get("normal")
                .unwrap()
                .lookup(&parse_key_seq("Q")),
            mae_core::LookupResult::None
        );
    }

    #[test]
    fn set_group_name_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        // Add some bindings under SPC z prefix
        rt.eval(r#"(define-key "normal" "SPC z a" "quit")"#)
            .unwrap();
        rt.eval(r#"(define-key "normal" "SPC z b" "save")"#)
            .unwrap();
        rt.eval(r#"(set-group-name "normal" "SPC z" "+test-group")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        let normal = editor.keymaps.get("normal").unwrap();
        let spc = mae_core::parse_key_seq_spaced("SPC");
        let entries = normal.which_key_entries(&spc, &editor.commands);
        let z_entry = entries
            .iter()
            .find(|e| matches!(e.key.key, mae_core::Key::Char('z')));
        assert!(z_entry.is_some(), "SPC should have a 'z' group");
        assert_eq!(z_entry.unwrap().label, "+test-group");
    }

    #[test]
    fn runtime_define_key_updates_keymap() {
        let mut rt = new_runtime();
        let mut ed = Editor::new();
        rt.eval(r#"(define-key "normal" "SPC z z" "quit")"#)
            .unwrap();
        rt.apply_to_editor(&mut ed);
        let normal = ed.keymaps.get("normal").unwrap();
        assert_eq!(
            normal.lookup(&mae_core::parse_key_seq_spaced("SPC z z")),
            mae_core::LookupResult::Exact("quit")
        );
    }

    // --- Round 2: file I/O tests ---

    #[test]
    fn file_exists_check() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(file-exists? "/tmp")"#).unwrap();
        assert_eq!(result, "#t");
    }

    #[test]
    fn file_exists_false() {
        let mut rt = new_runtime();
        let result = rt
            .eval(r#"(file-exists? "/tmp/nonexistent_file_12345")"#)
            .unwrap();
        assert_eq!(result, "#f");
    }

    #[test]
    fn read_file_works() {
        let mut rt = new_runtime();
        let test_path = "/tmp/mae_test_read_file.txt";
        std::fs::write(test_path, "test content").unwrap();
        let result = rt.eval(&format!(r#"(read-file "{}")"#, test_path)).unwrap();
        assert_eq!(result, "test content");
        let _ = std::fs::remove_file(test_path);
    }

    #[test]
    fn read_file_missing_returns_error() {
        let mut rt = new_runtime();
        let result = rt
            .eval(r#"(read-file "/tmp/nonexistent_file_99999")"#)
            .unwrap();
        assert!(result.starts_with("ERROR:"));
    }

    #[test]
    fn list_directory_works() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(length (list-directory "/tmp"))"#).unwrap();
        let count: i32 = result.parse().unwrap();
        assert!(count >= 0);
    }

    // --- Round 2: hook tests ---

    #[test]
    fn new_hooks_valid() {
        use mae_core::hooks::HookRegistry;
        assert!(HookRegistry::is_valid("option-change"));
        assert!(HookRegistry::is_valid("before-revert"));
        assert!(HookRegistry::is_valid("after-revert"));
        assert!(HookRegistry::is_valid("window-split"));
        assert!(HookRegistry::is_valid("window-close"));
        assert!(HookRegistry::is_valid("option-change:scroll_speed"));
    }

    #[test]
    fn buffer_char_count_injected() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "ABCDE");
        rt.inject_editor_state(&editor);
        let result = rt.eval("*buffer-char-count*").unwrap();
        assert_eq!(result, "5");
    }

    // --- Package infrastructure tests ---

    #[test]
    fn require_feature_not_found() {
        let mut rt = new_runtime();
        let result = rt.require_feature("nonexistent_feature_xyz");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in load-path"));
    }

    #[test]
    fn provide_marks_feature() {
        let mut rt = new_runtime();
        // provide-feature is the Rust-registered canonical name.
        rt.eval(r#"(provide-feature "my-feature")"#).unwrap();
        {
            let state = rt.shared.lock();
            assert!(
                state.loaded_features.contains("my-feature"),
                "SharedState should contain 'my-feature', got: {:?}",
                state.loaded_features
            );
        }
        let result = rt.eval(r#"(featurep "my-feature")"#).unwrap();
        assert_eq!(result, "#t");
    }

    #[test]
    fn load_path_default() {
        let rt = new_runtime();
        assert_eq!(rt.load_path.len(), 2);
        let paths: Vec<String> = rt
            .load_path
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert!(
            paths[0].ends_with("mae/packages"),
            "first entry should be packages dir: {}",
            paths[0]
        );
        assert!(
            paths[1].ends_with("mae/lisp"),
            "second entry should be lisp dir: {}",
            paths[1]
        );
    }

    #[test]
    fn add_to_load_path() {
        let mut rt = new_runtime();
        rt.eval(r#"(add-to-load-path! "/tmp/mae-test-packages")"#)
            .unwrap();
        // Sync from SharedState.
        rt.process_requires();
        assert_eq!(rt.load_path.len(), 3);
        assert_eq!(
            rt.load_path[0].display().to_string(),
            "/tmp/mae-test-packages"
        );
    }

    #[test]
    fn featurep_false_initially() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(featurep "unknown-feature")"#).unwrap();
        assert_eq!(result, "#f");
    }

    #[test]
    fn require_already_loaded_is_noop() {
        let mut rt = new_runtime();
        // Manually mark as loaded.
        rt.loaded_features.insert("already-loaded".to_string());
        let result = rt.require_feature("already-loaded");
        assert!(result.is_ok());
    }

    #[test]
    fn require_feature_loads_and_provides() {
        let dir = std::env::temp_dir().join("mae_test_require");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("test-pkg.scm"), r#"(provide-feature "test-pkg")"#).unwrap();

        let mut rt = new_runtime();
        rt.load_path.insert(0, dir.clone());
        let result = rt.require_feature("test-pkg");
        assert!(result.is_ok(), "require_feature failed: {:?}", result);
        assert!(rt.loaded_features.contains("test-pkg"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autoload_registers_command() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        rt.eval(r#"(autoload "my-cmd" "my-pkg" "My autoloaded command")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        let cmd = editor.commands.get("my-cmd").unwrap();
        assert_eq!(cmd.doc, "My autoloaded command");
        assert_eq!(
            cmd.source,
            CommandSource::Autoload {
                feature: "my-pkg".into()
            }
        );
    }

    #[test]
    fn module_loaded_query() {
        let mut rt = SchemeRuntime::new().unwrap();
        // No modules registered → module-loaded? returns false
        let result = rt.eval(r#"(module-loaded? "dashboard")"#).unwrap();
        assert!(result.contains("f"), "expected false, got: {}", result);

        // Register a module → returns true
        rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
            .unwrap();
        let result = rt.eval(r#"(module-loaded? "dashboard")"#).unwrap();
        assert!(result.contains("t"), "expected true, got: {}", result);
    }

    #[test]
    fn module_version_query() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval(r#"(module-version "dashboard")"#).unwrap();
        assert!(result.contains("f"), "expected false, got: {}", result);

        rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
            .unwrap();
        let result = rt.eval(r#"(module-version "dashboard")"#).unwrap();
        assert!(
            result.contains("0.1.0"),
            "expected version, got: {}",
            result
        );
    }

    #[test]
    fn module_list_query() {
        let mut rt = SchemeRuntime::new().unwrap();
        let result = rt.eval("(module-list)").unwrap();
        // Empty list
        assert!(
            result.contains("()"),
            "expected empty list, got: {}",
            result
        );

        rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
            .unwrap();
        let result = rt.eval("(module-list)").unwrap();
        assert!(
            result.contains("dashboard"),
            "expected dashboard, got: {}",
            result
        );
    }

    #[test]
    fn define_option_applies() {
        let mut rt = SchemeRuntime::new().unwrap();
        rt.eval(r#"(define-option! "my_option" "string" "hello" "A test option")"#)
            .unwrap();
        let mut editor = Editor::new();
        rt.apply_to_editor(&mut editor);
        let def = editor.option_registry.find("my_option");
        assert!(def.is_some(), "dynamic option should be registered");
        assert_eq!(def.unwrap().default_value.as_ref(), "hello");
    }

    #[test]
    fn undefine_command_applies() {
        let mut rt = SchemeRuntime::new().unwrap();
        let mut editor = Editor::new();
        // Editor starts with built-in commands
        assert!(editor.commands.get("move-left").is_some());
        rt.eval(r#"(undefine-command! "move-left")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert!(editor.commands.get("move-left").is_none());
    }

    #[test]
    fn unload_feature_removes() {
        let mut rt = SchemeRuntime::new().unwrap();
        rt.eval(r#"(provide-feature "test-mod")"#).unwrap();
        // Check via unload return value — true means it was present
        let result = rt.eval(r#"(unload-feature "test-mod")"#).unwrap();
        assert!(
            result.contains("t"),
            "expected true (was loaded), got: {}",
            result
        );
        // Second unload should return false
        let result = rt.eval(r#"(unload-feature "test-mod")"#).unwrap();
        assert!(
            result.contains("f"),
            "expected false (already removed), got: {}",
            result
        );
    }

    #[test]
    fn deprecation_warns_once() {
        let mut rt = SchemeRuntime::new().unwrap();
        rt.eval(r#"(deprecate-function! "old-fn" "new-fn" "0.9.0")"#)
            .unwrap();

        // First check-deprecated returns true
        let result = rt.eval(r#"(check-deprecated "old-fn")"#).unwrap();
        assert!(result.contains("t"), "expected true, got: {}", result);

        // Non-deprecated returns false
        let result = rt.eval(r#"(check-deprecated "new-fn")"#).unwrap();
        assert!(result.contains("f"), "expected false, got: {}", result);

        // Verify a warning message was queued
        let state = rt.shared.lock();
        assert!(
            state
                .pending_messages
                .iter()
                .any(|m| m.contains("deprecated")),
            "expected deprecation warning in messages"
        );
    }

    // ── mae! / package! declarative config tests ────────────────

    #[test]
    fn mae_bang_parses_modules() {
        let mut rt = new_runtime();
        rt.eval(r#"(mae! :editor "surround" "search")"#).unwrap();
        let decl = rt.declared_modules();
        assert!(decl.contains_key("surround"), "expected surround");
        assert!(decl.contains_key("search"), "expected search");
        assert_eq!(decl.len(), 2);
    }

    #[test]
    fn mae_bang_parses_flags() {
        let mut rt = new_runtime();
        rt.eval(r#"(mae! :editor (list "multicursor" "+align" "+fancy"))"#)
            .unwrap();
        let decl = rt.declared_modules();
        let flags = decl.get("multicursor").unwrap();
        assert!(flags.contains(&"+align".to_string()));
        assert!(flags.contains(&"+fancy".to_string()));
    }

    #[test]
    fn mae_bang_categories_are_labels() {
        let mut rt = new_runtime();
        rt.eval(r#"(mae! :editor "surround" :ui "dashboard" :lang "tables")"#)
            .unwrap();
        let decl = rt.declared_modules();
        assert_eq!(decl.len(), 3);
        assert!(decl.contains_key("surround"));
        assert!(decl.contains_key("dashboard"));
        assert!(decl.contains_key("tables"));
    }

    #[test]
    fn package_bang_basic() {
        let mut rt = new_runtime();
        rt.eval(r#"(package! "org-roam" :source "github:user/mae-org-roam")"#)
            .unwrap();
        let pkgs = rt.declared_packages();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "org-roam");
        assert_eq!(pkgs[0].source.as_deref(), Some("github:user/mae-org-roam"));
        assert!(!pkgs[0].disable);
    }

    #[test]
    fn package_bang_pin() {
        let mut rt = new_runtime();
        rt.eval(r#"(package! "my-theme" :source "github:u/r" :pin "abc123")"#)
            .unwrap();
        let pkgs = rt.declared_packages();
        assert_eq!(pkgs[0].pin.as_deref(), Some("abc123"));
    }

    #[test]
    fn package_bang_disable() {
        let mut rt = new_runtime();
        rt.eval(r#"(package! "dashboard" :disable #t)"#).unwrap();
        let pkgs = rt.declared_packages();
        assert!(pkgs[0].disable);
    }

    #[test]
    fn define_kb_node_from_scheme() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();

        rt.eval(r#"(define-kb-node! "module:test:guide" "Test Guide" "Some body text")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);

        let node = editor.kb.primary.get("module:test:guide");
        assert!(node.is_some(), "expected kb node to be registered");
        assert_eq!(node.unwrap().title, "Test Guide");
    }

    #[test]
    fn undeclared_modules_not_in_declared() {
        let mut rt = new_runtime();
        rt.eval(r#"(mae! :editor "surround")"#).unwrap();
        let decl = rt.declared_modules();
        assert!(!decl.contains_key("dashboard"), "dashboard not declared");
        assert!(decl.contains_key("surround"), "surround declared");
    }

    // --- Phase A: New Scheme API tests ---

    #[test]
    fn string_split_works() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(string-split "a,b,c" ",")"#).unwrap();
        assert!(result.contains("a"));
        assert!(result.contains("b"));
        assert!(result.contains("c"));
    }

    #[test]
    fn string_join_works() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(string-join '("a" "b" "c") ",")"#).unwrap();
        assert_eq!(result, "a,b,c");
    }

    #[test]
    fn string_trim_works() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(string-trim "  hello  ")"#).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn string_contains_works() {
        let mut rt = new_runtime();
        assert_eq!(
            rt.eval(r#"(string-contains? "hello world" "world")"#)
                .unwrap(),
            "#t"
        );
        assert_eq!(
            rt.eval(r#"(string-contains? "hello" "xyz")"#).unwrap(),
            "#f"
        );
    }

    #[test]
    fn string_replace_works() {
        let mut rt = new_runtime();
        let result = rt
            .eval(r#"(string-replace "hello world" "world" "rust")"#)
            .unwrap();
        assert_eq!(result, "hello rust");
    }

    #[test]
    fn string_upcase_downcase_works() {
        let mut rt = new_runtime();
        assert_eq!(rt.eval(r#"(string-upcase "hello")"#).unwrap(), "HELLO");
        assert_eq!(rt.eval(r#"(string-downcase "HELLO")"#).unwrap(), "hello");
    }

    #[test]
    fn shell_command_works() {
        let mut rt = new_runtime();
        let result = rt.eval(r#"(shell-command "echo hello")"#).unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[test]
    fn create_buffer_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        let initial_count = editor.buffers.len();
        rt.eval(r#"(create-buffer "test-buf")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers.len(), initial_count + 1);
        assert_eq!(editor.buffers.last().unwrap().name, "test-buf");
    }

    #[test]
    fn kill_buffer_by_name_works() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        // Create a buffer first
        let mut buf = mae_core::Buffer::new();
        buf.name = "kill-me".to_string();
        editor.buffers.push(buf);
        assert_eq!(editor.buffers.len(), 2);
        rt.eval(r#"(kill-buffer-by-name "kill-me")"#).unwrap();
        rt.apply_to_editor(&mut editor);
        assert_eq!(editor.buffers.len(), 1);
    }

    #[test]
    fn buffer_introspection_functions() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            for ch in "hello\nworld".chars() {
                editor.buffers[0].insert_char(win, ch);
            }
        }
        rt.inject_editor_state(&editor);
        assert_eq!(rt.eval("(current-line-number)").unwrap(), "2");
        // point-min is always 0
        assert_eq!(rt.eval("(point-min)").unwrap(), "0");
        // point-max = total chars
        let pmax = rt.eval("(point-max)").unwrap();
        assert!(pmax.parse::<i64>().unwrap() > 0);
        // current-buffer-name
        let name = rt.eval("(current-buffer-name)").unwrap();
        assert!(!name.is_empty());
    }

    #[test]
    fn region_inactive_in_normal_mode() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        assert_eq!(rt.eval("(region-active?)").unwrap(), "#f");
    }

    #[test]
    fn advice_add_and_remove() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        rt.eval(r#"(advice-add! "save" ":before" "my-before-save")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        let before = editor
            .hooks
            .get_advice("save", mae_core::hooks::AdviceKind::Before);
        assert_eq!(before, vec!["my-before-save"]);

        rt.eval(r#"(advice-remove! "save" "my-before-save")"#)
            .unwrap();
        rt.apply_to_editor(&mut editor);
        let before = editor
            .hooks
            .get_advice("save", mae_core::hooks::AdviceKind::Before);
        assert!(before.is_empty());
    }

    #[test]
    fn current_command_variable_exists() {
        let mut rt = new_runtime();
        let editor = Editor::new();
        rt.inject_editor_state(&editor);
        // Should not error — variable exists
        let result = rt.eval("*current-command*").unwrap();
        assert!(result.is_empty());
    }
}
