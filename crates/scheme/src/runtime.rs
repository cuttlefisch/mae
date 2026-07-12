// @ai-caution: [architecture-debt] Scheme VM runtime. `SchemeRuntime::new()`'s
// `register_fn` calls and the `inject_editor_state`/`apply_to_editor` state-sync
// pair have been split into `crates/scheme/src/runtime/*.rs` submodules by
// category — this residual file holds `SharedState` + `SchemeRuntime`'s core
// methods (eval, load_file, accessors) and is still over the 800-line ceiling.
// Tracked in .claude/commands/mae-audit.md's "Known exceptions" and
// ROADMAP.md's "Architecture Debt" section; prefer extending an existing
// `runtime/*.rs` submodule (or adding a new one) over growing this file further.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use tracing::{debug, error, info};

use mae_core::Editor;

use crate::ffi::value_to_display;
use crate::lisp_error::LispError;
use crate::value::Value;
use crate::vm::Vm;

mod editor_ops;
mod io_packages;
mod kb_graph_view;
mod kb_primitives;
mod kb_queries;
mod keybindings;
mod misc_primitives;
mod shell_agenda;
mod state_sync_apply;
mod state_sync_apply2;
mod state_sync_inject;
mod state_sync_inject_kb;
mod test_primitives;

use editor_ops::register_editor_ops_fns;
use io_packages::register_io_package_fns;
use kb_graph_view::register_kb_graph_view_fns;
use kb_primitives::register_kb_primitive_fns;
use kb_queries::register_kb_query_fns;
use keybindings::register_keybinding_fns;
use misc_primitives::register_misc_primitive_fns;
use shell_agenda::register_shell_agenda_fns;
use test_primitives::register_test_primitive_fns;

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
    /// Pending native KB graph-view intents from `(kb-graph-view-open)` etc.
    /// (Part C Phase 1) — drained in order into the matching
    /// `Editor::kb_graph_view_*` method, mirroring
    /// `pending_kb_collab_actions` above.
    pending_graph_view_intents: Vec<mae_core::GraphViewIntent>,
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

        register_keybinding_fns(&mut vm, &shared);
        register_editor_ops_fns(&mut vm, &shared);
        register_shell_agenda_fns(&mut vm, &shared);
        register_io_package_fns(&mut vm, &shared);
        register_kb_primitive_fns(&mut vm, &shared);
        register_kb_query_fns(&mut vm, &shared);
        register_kb_graph_view_fns(&mut vm, &shared);
        register_misc_primitive_fns(&mut vm, &shared);
        register_test_primitive_fns(&mut vm, &shared);

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
#[path = "runtime_tests.rs"]
mod tests;
