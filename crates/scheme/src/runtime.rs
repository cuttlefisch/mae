use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// (keymap_name, parent_name) — new keymaps to create
    keymap_defs: Vec<(String, String)>,
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
    /// Pending sleep from `(sleep-ms N)`.
    pending_sleep_ms: Option<u64>,
    /// Ex-commands to dispatch via `(execute-ex CMD-STRING)`.
    /// Routes through `execute_command()` which handles argument parsing.
    pending_ex_commands: Vec<String>,

    // --- CRDT/sync test primitives ---
    /// Pending enable-sync: client_id for active buffer.
    pending_enable_sync: Option<u64>,
    /// Pending disable-sync on active buffer.
    pending_disable_sync: bool,
    /// Pending sync updates to apply: (buffer_name, base64-encoded update).
    pending_sync_applies: Vec<(String, Vec<u8>)>,
    /// Pending load-sync-state: (base64-decoded state bytes, client_id).
    pending_load_sync_state: Option<(Vec<u8>, u64)>,
    /// Flag: drain pending_sync_updates on active buffer after next apply.
    pending_drain_sync_updates: bool,
    /// Drained sync updates (stored here so Scheme can retrieve them).
    drained_sync_updates: Vec<String>,
    /// Current mode string for test inspection (updated by test runner).
    current_mode: String,
    /// Active buffer text for test inspection (updated by test runner).
    current_buffer_text: String,
    /// All buffer texts for (buffer-text NAME) (updated by test runner).
    all_buffer_texts: Vec<(String, String)>,
    /// Whether sync is enabled on active buffer (updated by test runner).
    sync_enabled: bool,
    /// Number of pending sync updates (updated by test runner).
    pending_update_count: usize,
    /// Sync doc content (None if sync not enabled) (updated by test runner).
    sync_content: Option<String>,
    /// Encoded sync state (None if sync not enabled) (updated by test runner).
    encoded_state: Option<String>,
    /// Buffer name→index mapping (updated by test runner for cross-test visibility).
    buffer_names: Vec<(usize, String)>,

    // --- Option state (updated by test runner) ---
    /// Snapshot of option values: (name, value_string).
    option_values: Vec<(String, String)>,

    // --- Visual/region state (updated by test runner) ---
    /// Whether a visual selection is active.
    region_active: bool,
    /// Start offset of the visual selection.
    region_start: usize,
    /// End offset of the visual selection.
    region_end: usize,

    // --- Cursor state (updated by test runner) ---
    /// Cursor row (0-indexed), updated by sync_scheme_state.
    cursor_row: usize,
    /// Cursor column (0-indexed), updated by sync_scheme_state.
    cursor_col: usize,
    /// Last status message set by the editor (for test inspection).
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
    /// Directories to search for `(require FEATURE)`.
    pub load_path: Vec<PathBuf>,
    /// Features that have been successfully loaded via `require`/`provide`.
    pub loaded_features: HashSet<String>,
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

        // Register define-keymap: (define-keymap NAME PARENT)
        let s = shared.clone();
        engine.register_fn("define-keymap", move |name: String, parent: String| {
            s.lock().unwrap().keymap_defs.push((name, parent));
            SteelVal::Void
        });

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

        // (execute-ex CMD-STRING) — route through ex-command parser.
        // Handles argument splitting: (execute-ex "collab-join test.txt"),
        // (execute-ex "saveas /path/to/file"), (execute-ex "w /path"), etc.
        let s = shared.clone();
        engine.register_fn("execute-ex", move |cmd: String| {
            s.lock().unwrap().pending_ex_commands.push(cmd);
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

        // (set-local-option! KEY VALUE) — set a buffer-local option on the active buffer.
        let s = shared.clone();
        engine.register_fn("set-local-option!", move |key: String, value: String| {
            s.lock().unwrap().pending_local_options.push((key, value));
            SteelVal::Void
        });

        // (display-buffer-policy KIND) — query active display rule for a BufferKind
        {
            // This is read-only from Scheme — just needs editor access at apply time.
            // We return a static value by having the engine store nothing; the real
            // query happens in apply_to_editor. For now, expose a simple version
            // that doesn't need editor state.
            engine.register_fn("display-buffer-policy", move |kind: String| -> SteelVal {
                use mae_core::display_policy::{
                    action_to_string, parse_buffer_kind, DisplayPolicy,
                };
                match parse_buffer_kind(&kind) {
                    Some(bk) => {
                        let policy = DisplayPolicy::default();
                        SteelVal::StringV(action_to_string(&policy.action_for(bk)).into())
                    }
                    None => SteelVal::StringV(format!("unknown kind: {}", kind).into()),
                }
            });
        }

        // (set-display-rule! KIND ACTION) — override display policy from init.scm
        let s = shared.clone();
        engine.register_fn("set-display-rule!", move |kind: String, action: String| {
            s.lock().unwrap().pending_display_rules.push((kind, action));
            SteelVal::Void
        });

        // (set-buffer-kind-replaceable! KIND ENABLE) — mark a buffer kind as replaceable
        let s = shared.clone();
        engine.register_fn(
            "set-buffer-kind-replaceable!",
            move |kind: String, enable: bool| {
                s.lock()
                    .unwrap()
                    .pending_replaceable_kinds
                    .push((kind, enable));
                SteelVal::Void
            },
        );

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

        let s = shared.clone();
        engine.register_fn("recent-files-add!", move |path: String| {
            s.lock().unwrap().pending_recent_files.push(path);
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn("recent-projects-add!", move |path: String| {
            s.lock().unwrap().pending_recent_projects.push(path);
            SteelVal::Void
        });

        // --- Agenda file management ---

        let s = shared.clone();
        engine.register_fn("agenda-add!", move |path: String| {
            s.lock().unwrap().pending_agenda_adds.push(path);
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn("agenda-remove!", move |path: String| {
            s.lock().unwrap().pending_agenda_removes.push(path);
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn("agenda-list", move || {
            s.lock().unwrap().pending_agenda_list = true;
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn(
            "visual-buffer-add-rect!",
            move |x: f64, y: f64, w: f64, h: f64, fill: Option<String>, stroke: Option<String>| {
                let mut state = s.lock().unwrap();
                state.pending_visual_ops.push(VisualOp::AddRect {
                    x: x as f32,
                    y: y as f32,
                    w: w as f32,
                    h: h as f32,
                    fill,
                    stroke,
                });
                SteelVal::Void
            },
        );

        let s = shared.clone();
        engine.register_fn("visual-buffer-clear!", move || {
            s.lock().unwrap().pending_visual_ops.push(VisualOp::Clear);
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn(
            "visual-buffer-add-line!",
            move |x1: f64, y1: f64, x2: f64, y2: f64, color: String, thickness: f64| {
                let mut state = s.lock().unwrap();
                state.pending_visual_ops.push(VisualOp::AddLine {
                    x1: x1 as f32,
                    y1: y1 as f32,
                    x2: x2 as f32,
                    y2: y2 as f32,
                    color,
                    thickness: thickness as f32,
                });
                SteelVal::Void
            },
        );

        let s = shared.clone();
        engine.register_fn(
            "visual-buffer-add-circle!",
            move |cx: f64, cy: f64, r: f64, fill: Option<String>, stroke: Option<String>| {
                let mut state = s.lock().unwrap();
                state.pending_visual_ops.push(VisualOp::AddCircle {
                    cx: cx as f32,
                    cy: cy as f32,
                    r: r as f32,
                    fill,
                    stroke,
                });
                SteelVal::Void
            },
        );

        let s = shared.clone();
        engine.register_fn(
            "visual-buffer-add-text!",
            move |x: f64, y: f64, text: String, font_size: f64, color: String| {
                let mut state = s.lock().unwrap();
                state.pending_visual_ops.push(VisualOp::AddText {
                    x: x as f32,
                    y: y as f32,
                    text,
                    font_size: font_size as f32,
                    color,
                });
                SteelVal::Void
            },
        );

        // --- Round 2: buffer editing, buffer list, keymap introspection ---

        // (buffer-delete-range START END)
        let s = shared.clone();
        engine.register_fn("buffer-delete-range", move |start: isize, end: isize| {
            s.lock().unwrap().pending_delete_range =
                Some((start.max(0) as usize, end.max(0) as usize));
            SteelVal::Void
        });

        // (buffer-replace-range START END TEXT)
        let s = shared.clone();
        engine.register_fn(
            "buffer-replace-range",
            move |start: isize, end: isize, text: String| {
                s.lock().unwrap().pending_replace_range =
                    Some((start.max(0) as usize, end.max(0) as usize, text));
                SteelVal::Void
            },
        );

        // (buffer-undo)
        let s = shared.clone();
        engine.register_fn("buffer-undo", move || {
            s.lock().unwrap().pending_undo = true;
            SteelVal::Void
        });

        // (buffer-redo)
        let s = shared.clone();
        engine.register_fn("buffer-redo", move || {
            s.lock().unwrap().pending_redo = true;
            SteelVal::Void
        });

        // (switch-to-buffer IDX)
        let s = shared.clone();
        engine.register_fn("switch-to-buffer", move |idx: isize| {
            s.lock().unwrap().pending_switch_buffer = Some(idx.max(0) as usize);
            SteelVal::Void
        });

        // (undefine-key! MAP KEY)
        let s = shared.clone();
        engine.register_fn("undefine-key!", move |map: String, key: String| {
            s.lock().unwrap().pending_key_removals.push((map, key));
            SteelVal::Void
        });

        // (set-group-name MAP PREFIX LABEL) — set which-key group label
        let s = shared.clone();
        engine.register_fn(
            "set-group-name",
            move |map: String, prefix: String, label: String| {
                s.lock()
                    .unwrap()
                    .pending_group_names
                    .push((map, prefix, label));
                SteelVal::Void
            },
        );

        // --- File I/O (no editor state needed) ---

        // (read-file PATH) — reads a file, capped at 1MB
        engine.register_fn("read-file", |path: String| -> SteelVal {
            match std::fs::read_to_string(&path) {
                Ok(content) if content.len() <= 1_048_576 => SteelVal::StringV(content.into()),
                Ok(_) => SteelVal::StringV("ERROR: file exceeds 1MB limit".into()),
                Err(e) => SteelVal::StringV(format!("ERROR: {}", e).into()),
            }
        });

        // (file-exists? PATH)
        engine.register_fn("file-exists?", |path: String| -> bool {
            std::path::Path::new(&path).exists()
        });

        // (list-directory PATH) — returns list of (name is-dir?)
        engine.register_fn("list-directory", |path: String| -> SteelVal {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let items: Vec<SteelVal> = entries
                        .flatten()
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                            SteelVal::ListV(
                                vec![SteelVal::StringV(name.into()), SteelVal::BoolV(is_dir)]
                                    .into(),
                            )
                        })
                        .collect();
                    SteelVal::ListV(items.into())
                }
                Err(_) => SteelVal::ListV(vec![].into()),
            }
        });

        // --- Package infrastructure ---

        // (provide FEATURE) — mark feature as loaded.
        // Steel has a built-in `provide` (module system) that shadows `register_fn`,
        // so we register as `provide-feature` and also define a Scheme alias.
        // Package files should use `(provide-feature "name")` for reliability.
        let s = shared.clone();
        engine.register_fn("provide-feature", move |feature: String| {
            s.lock().unwrap().loaded_features.insert(feature);
            SteelVal::Void
        });

        // (featurep FEATURE) — check if feature is loaded.
        let s = shared.clone();
        engine.register_fn("featurep", move |feature: String| {
            let loaded = s.lock().unwrap().loaded_features.contains(&feature);
            SteelVal::BoolV(loaded)
        });

        // (require-feature FEATURE) — request loading; resolved in process_requires().
        // Named `require-feature` to avoid collision with Steel's built-in `require`.
        let s = shared.clone();
        engine.register_fn("require-feature", move |feature: String| {
            let mut state = s.lock().unwrap();
            if !state.loaded_features.contains(&feature) {
                state.pending_requires.push(feature);
            }
            SteelVal::Void
        });

        // (load-path) — return current load-path as list of strings.
        let s = shared.clone();
        engine.register_fn("load-path", move || {
            let state = s.lock().unwrap();
            let items: Vec<SteelVal> = state
                .load_path
                .iter()
                .map(|p| SteelVal::StringV(p.to_string_lossy().into_owned().into()))
                .collect();
            SteelVal::ListV(items.into())
        });

        // (add-to-load-path! DIR) — prepend directory to load-path.
        let s = shared.clone();
        engine.register_fn("add-to-load-path!", move |dir: String| {
            let mut state = s.lock().unwrap();
            state.load_path.insert(0, PathBuf::from(dir));
            SteelVal::Void
        });

        // (autoload COMMAND-NAME FEATURE DOC) — register a command backed by autoload.
        let s = shared.clone();
        engine.register_fn(
            "autoload",
            move |cmd_name: String, feature: String, doc: String| {
                s.lock()
                    .unwrap()
                    .pending_autoloads
                    .push((cmd_name, feature, doc));
                SteelVal::Void
            },
        );

        // --- Module system functions ---

        // (when-flag FLAG-NAME THUNK) — evaluate thunk if flag is set.
        // Flags are set as __mae-flag-MODULE-FLAG variables by the loader.
        // This is a convenience wrapper that modules use in autoloads.scm.
        engine
            .run(
                r#"
(define (when-flag module-name flag-name thunk)
  ;; Flag variables are set as __mae-flag-MODULE-FLAG = #t by the loader.
  ;; We can't easily check from Scheme since we don't know the module name here,
  ;; so for now just evaluate the thunk. The loader only sets flags that are enabled.
  (thunk))
"#,
            )
            .ok();

        // (define-option! NAME KIND DEFAULT DOC) — register a runtime option.
        // Queued and applied in apply_to_editor().
        let s = shared.clone();
        engine.register_fn(
            "define-option!",
            move |name: String, kind: String, default: String, doc: String| {
                s.lock()
                    .unwrap()
                    .pending_dynamic_options
                    .push((name, kind, default, doc));
                SteelVal::Void
            },
        );

        // (module-loaded? NAME) — check if a module is active
        let s = shared.clone();
        engine.register_fn("module-loaded?", move |name: String| {
            SteelVal::BoolV(s.lock().unwrap().active_modules.contains_key(&name))
        });

        // (module-version NAME) — get version of active module, or #f
        let s = shared.clone();
        engine.register_fn("module-version", move |name: String| {
            match s.lock().unwrap().active_modules.get(&name) {
                Some(v) => SteelVal::StringV(v.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // (module-list) — list all active module names
        let s = shared.clone();
        engine.register_fn("module-list", move || {
            let state = s.lock().unwrap();
            SteelVal::ListV(
                state
                    .active_modules
                    .keys()
                    .map(|k| SteelVal::StringV(k.clone().into()))
                    .collect::<Vec<_>>()
                    .into(),
            )
        });

        // (register-module! NAME VERSION) — called by loader after loading a module
        let s = shared.clone();
        engine.register_fn("register-module!", move |name: String, version: String| {
            s.lock().unwrap().active_modules.insert(name, version);
            SteelVal::Void
        });

        // (when-module NAME THUNK) — evaluate thunk only if module is active.
        // Defined in Scheme for ergonomics (thunk is a lambda).
        engine
            .run(
                r#"
(define (when-module name thunk)
  (when (module-loaded? name)
    (thunk)))
"#,
            )
            .ok();

        // (module-flags NAME) — get enabled flags for a module.
        // Returns the flags stored by the loader via flag variables.
        // For now returns an empty list — flags are injected as individual
        // Scheme variables (__mae-flag-<module>-<flag>), not collected.
        // TODO: populate from loader when mae! parsing is implemented.
        engine.register_fn("module-flags", move |_name: String| -> SteelVal {
            SteelVal::ListV(vec![].into())
        });

        // --- Declarative package management (mae!, package!) ---

        // (mae-declare-module! NAME . FLAGS) — declare a module with optional flags.
        // Called by the Scheme-level mae! helper for each module entry.
        let s = shared.clone();
        engine.register_fn(
            "mae-declare-module!",
            move |name: String, flags: Vec<String>| {
                s.lock().unwrap().declared_modules.insert(name, flags);
                SteelVal::Void
            },
        );

        // (mae-declared-modules) — return list of declared module names (for introspection).
        let s = shared.clone();
        engine.register_fn("mae-declared-modules", move || {
            let state = s.lock().unwrap();
            SteelVal::ListV(
                state
                    .declared_modules
                    .keys()
                    .map(|k| SteelVal::StringV(k.clone().into()))
                    .collect::<Vec<_>>()
                    .into(),
            )
        });

        // (package! NAME . KWARGS) — declare a third-party package.
        // Keyword args: :source STRING, :pin STRING, :disable BOOL
        // Implemented as a multi-arity function; kwargs parsed by Scheme wrapper.
        let s = shared.clone();
        engine.register_fn(
            "mae-declare-package!",
            move |name: String, source: String, pin: String, disable: bool| {
                s.lock().unwrap().declared_packages.push(DeclaredPackage {
                    name,
                    source: if source.is_empty() {
                        None
                    } else {
                        Some(source)
                    },
                    pin: if pin.is_empty() { None } else { Some(pin) },
                    disable,
                });
                SteelVal::Void
            },
        );

        // Define mae! and package! Scheme-level wrappers.
        // mae! accepts category labels (:editor, :ui, :lang) and module entries.
        // Categories are informational only — they don't affect behavior.
        // Module entries can be bare names or (name +flag1 +flag2).
        //
        // Steel doesn't have Clojure-style keywords. We pre-define category
        // symbols as strings so they can be used unquoted in mae! blocks.
        engine
            .run(
                r#"
;; Pre-define category labels so they're valid identifiers.
;; Their values are strings starting with ":" — mae! skips them.
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

;; (mae! :category1 "mod1" ("mod2" "+flag") :category2 "mod3" ...)
;; Category labels (strings starting with ":") are ignored.
;; String entries declare a module with no flags.
;; List entries declare a module (first string) with flags (remaining strings).
(define (mae! . args)
  (for-each
    (lambda (item)
      (cond
        ;; Skip category strings (starting with ":")
        ((and (string? item)
              (> (string-length item) 0)
              (equal? (substring item 0 1) ":"))
         #f)
        ;; List entry: ("module-name" "+flag1" "+flag2" ...)
        ((list? item)
         (mae-declare-module! (car item) (cdr item)))
        ;; String entry: module with no flags
        ((string? item)
         (mae-declare-module! item '()))
        ;; Symbol entry: convert to string
        ((symbol? item)
         (mae-declare-module! (symbol->string item) '()))
        (else #f)))
    args))

;; Keyword symbols for package! kwargs.
(define :source ":source")
(define :pin ":pin")
(define :disable ":disable")

;; (package! NAME :source SRC :pin SHA :disable BOOL)
;; All keyword args are optional.
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

        // (undefine-command! NAME) — remove a command (for module unload)
        let s = shared.clone();
        engine.register_fn("undefine-command!", move |name: String| {
            s.lock().unwrap().pending_command_unregisters.push(name);
            SteelVal::Void
        });

        // (undefine-option! NAME) — remove an option (for module unload)
        let s = shared.clone();
        engine.register_fn("undefine-option!", move |name: String| {
            s.lock().unwrap().pending_option_unregisters.push(name);
            SteelVal::Void
        });

        // (unload-feature NAME) — remove from loaded_features
        let s = shared.clone();
        engine.register_fn("unload-feature", move |name: String| {
            let removed = s.lock().unwrap().loaded_features.remove(&name);
            SteelVal::BoolV(removed)
        });

        // (define-kb-node! ID TITLE BODY) — register a KB node from Scheme.
        let s = shared.clone();
        engine.register_fn(
            "define-kb-node!",
            move |id: String, title: String, body: String| {
                s.lock().unwrap().pending_kb_nodes.push((id, title, body));
                SteelVal::Void
            },
        );

        // (deprecate-function! OLD-NAME NEW-NAME SINCE-VERSION)
        // Registers a deprecation warning. When OLD-NAME is called,
        // a warning is emitted once and the call is logged.
        let s = shared.clone();
        engine.register_fn(
            "deprecate-function!",
            move |old_name: String, new_name: String, since: String| {
                s.lock()
                    .unwrap()
                    .deprecated_functions
                    .insert(old_name, (new_name, since));
                SteelVal::Void
            },
        );

        // (register-ai-tool! NAME DESCRIPTION HANDLER-FN PERMISSION)
        let s = shared.clone();
        engine.register_fn(
            "register-ai-tool!",
            move |name: String, desc: String, handler: String, perm: String| {
                let mut st = s.lock().unwrap();
                // Collect any pre-registered params/required for this tool
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
                SteelVal::Void
            },
        );

        // (ai-tool-param! TOOL-NAME PARAM-NAME PARAM-TYPE DESCRIPTION)
        let s = shared.clone();
        engine.register_fn(
            "ai-tool-param!",
            move |tool: String, pname: String, ptype: String, pdesc: String| {
                s.lock()
                    .unwrap()
                    .pending_ai_tool_params
                    .entry(tool)
                    .or_default()
                    .push((pname, ptype, pdesc));
                SteelVal::Void
            },
        );

        // (ai-tool-require! TOOL-NAME PARAM-NAME)
        let s = shared.clone();
        engine.register_fn("ai-tool-require!", move |tool: String, pname: String| {
            s.lock()
                .unwrap()
                .pending_ai_tool_required
                .entry(tool)
                .or_default()
                .push(pname);
            SteelVal::Void
        });

        // (register-splash-art! NAME ART-STRING)
        let s = shared.clone();
        engine.register_fn("register-splash-art!", move |name: String, art: String| {
            s.lock()
                .unwrap()
                .pending_splash_arts
                .push((name, art, None));
            SteelVal::Void
        });

        // (register-splash-art-image! NAME IMAGE-PATH)
        // Resolves relative paths against current_module_dir if set.
        let s = shared.clone();
        engine.register_fn(
            "register-splash-art-image!",
            move |name: String, path: String| {
                let mut st = s.lock().unwrap();
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
                SteelVal::Void
            },
        );

        // --- A5: String utilities (no editor state needed) ---

        engine.register_fn("string-split", |s: String, sep: String| -> SteelVal {
            SteelVal::ListV(
                s.split(&sep)
                    .map(|part| SteelVal::StringV(part.into()))
                    .collect::<Vec<_>>()
                    .into(),
            )
        });

        engine.register_fn("string-join", |lst: Vec<String>, sep: String| -> String {
            lst.join(&sep)
        });

        engine.register_fn("string-trim", |s: String| -> String {
            s.trim().to_string()
        });

        engine.register_fn("string-contains?", |s: String, sub: String| -> bool {
            s.contains(&sub)
        });

        engine.register_fn(
            "string-replace",
            |s: String, from: String, to: String| -> String { s.replace(&from, &to) },
        );

        engine.register_fn("string-upcase", |s: String| -> String { s.to_uppercase() });

        engine.register_fn("string-downcase", |s: String| -> String {
            s.to_lowercase()
        });

        // --- A4: Process execution ---

        engine.register_fn("shell-command", |cmd: String| -> String {
            use std::process::Command;
            match Command::new("sh").arg("-c").arg(&cmd).output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.len() > 1_048_576 {
                        stdout[..1_048_576].to_string()
                    } else {
                        stdout.into_owned()
                    }
                }
                Err(e) => format!("ERROR: {}", e),
            }
        });

        // --- A3: Buffer creation/kill (via SharedState) ---

        let s = shared.clone();
        engine.register_fn("create-buffer", move |name: String| {
            s.lock().unwrap().pending_create_buffer = Some(name);
            SteelVal::Void
        });

        let s = shared.clone();
        engine.register_fn("kill-buffer-by-name", move |name: String| {
            s.lock().unwrap().pending_kill_buffer = Some(name);
            SteelVal::Void
        });

        // --- Phase E: Advice system ---

        // (advice-add! COMMAND KIND FN-NAME)
        // KIND is ":before" or ":after"
        let s = shared.clone();
        engine.register_fn(
            "advice-add!",
            move |command: String, kind: String, fn_name: String| {
                s.lock()
                    .unwrap()
                    .pending_advice_adds
                    .push((command, kind, fn_name));
                SteelVal::Void
            },
        );

        // (advice-remove! COMMAND FN-NAME)
        let s = shared.clone();
        engine.register_fn("advice-remove!", move |command: String, fn_name: String| {
            s.lock()
                .unwrap()
                .pending_advice_removes
                .push((command, fn_name));
            SteelVal::Void
        });

        // (check-deprecated NAME) — check if a function name is deprecated,
        // log a warning (once), return #t if deprecated, #f otherwise.
        let s = shared.clone();
        engine.register_fn("check-deprecated", move |name: String| {
            let mut state = s.lock().unwrap();
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
                SteelVal::BoolV(true)
            } else {
                SteelVal::BoolV(false)
            }
        });

        // --- Test framework primitives ---

        // (exit CODE) — request process exit with given code.
        // Accumulated in SharedState; the test runner checks after each eval.
        let s = shared.clone();
        engine.register_fn("exit", move |code: isize| {
            s.lock().unwrap().pending_exit_code = Some(code as i32);
            SteelVal::Void
        });

        // (write-file PATH CONTENT) — write a string to disk.
        // Useful for inter-container signaling in docker-based tests.
        let s = shared.clone();
        engine.register_fn("write-file", move |path: String, content: String| {
            s.lock().unwrap().pending_write_files.push((path, content));
            SteelVal::Void
        });

        // (sleep-ms N) — request a sleep of N milliseconds.
        // Accumulated in SharedState; the test runner handles the actual sleep
        // and drains collab/shell events during the wait.
        let s = shared.clone();
        engine.register_fn("sleep-ms", move |ms: isize| {
            s.lock().unwrap().pending_sleep_ms = Some(ms.max(0) as u64);
            SteelVal::Void
        });

        // (file-exists? PATH) — check if a file exists on disk.
        engine.register_fn("file-exists?", move |path: String| -> bool {
            std::path::Path::new(&path).exists()
        });

        // (current-milliseconds) — monotonic time in milliseconds.
        engine.register_fn("current-milliseconds", move || -> isize {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as isize
        });

        // (goto-char OFFSET) — move cursor to character offset (0-indexed).
        // Accumulated as a pending cursor operation.
        let s = shared.clone();
        engine.register_fn("goto-char", move |offset: isize| {
            // Store as a special sentinel: row=usize::MAX signals char-offset mode.
            // The apply_to_editor handler converts offset → (row, col).
            s.lock().unwrap().pending_cursor = Some((usize::MAX, offset.max(0) as usize));
            SteelVal::Void
        });

        // --- Test introspection via SharedState ---

        // --- Test introspection functions via SharedState ---
        // These read from SharedState (updated by test runner's sync_scheme_state),
        // so they always return the latest value regardless of Steel binding scopes.

        // (current-mode) — read the current mode.
        let s = shared.clone();
        engine.register_fn("current-mode", move || -> String {
            s.lock().unwrap().current_mode.clone()
        });

        // (test-buffer-string) — read active buffer text (test runner updates this).
        let s = shared.clone();
        engine.register_fn("test-buffer-string", move || -> String {
            s.lock().unwrap().current_buffer_text.clone()
        });

        // (test-buffer-text NAME) — read named buffer text.
        let s = shared.clone();
        engine.register_fn("test-buffer-text", move |name: String| -> SteelVal {
            let state = s.lock().unwrap();
            state
                .all_buffer_texts
                .iter()
                .find(|(n, _)| n == &name || n.ends_with(&name))
                .map(|(_, t)| SteelVal::StringV(t.clone().into()))
                .unwrap_or(SteelVal::BoolV(false))
        });

        // (test-sync-enabled?) — whether sync is enabled on active buffer.
        let s = shared.clone();
        engine.register_fn("test-sync-enabled?", move || -> bool {
            s.lock().unwrap().sync_enabled
        });

        // (test-pending-updates) — number of pending sync updates.
        let s = shared.clone();
        engine.register_fn("test-pending-updates", move || -> isize {
            s.lock().unwrap().pending_update_count as isize
        });

        // (test-sync-content) — sync doc content or #f.
        let s = shared.clone();
        engine.register_fn("test-sync-content", move || -> SteelVal {
            let state = s.lock().unwrap();
            match &state.sync_content {
                Some(c) => SteelVal::StringV(c.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // (test-encode-state) — encoded sync state or #f.
        let s = shared.clone();
        engine.register_fn("test-encode-state", move || -> SteelVal {
            let state = s.lock().unwrap();
            match &state.encoded_state {
                Some(s) => SteelVal::StringV(s.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // (test-get-buffer-by-name NAME) — lookup buffer index by name from SharedState.
        let s = shared.clone();
        engine.register_fn("test-get-buffer-by-name", move |name: String| -> SteelVal {
            let state = s.lock().unwrap();
            state
                .buffer_names
                .iter()
                .find(|(_, n)| n == &name)
                .map(|(i, _)| SteelVal::IntV(*i as isize))
                .unwrap_or(SteelVal::BoolV(false))
        });

        // (test-get-option NAME) — read option value from SharedState (fresh each step).
        let s = shared.clone();
        engine.register_fn("test-get-option", move |name: String| -> SteelVal {
            let state = s.lock().unwrap();
            state
                .option_values
                .iter()
                .find(|(n, _)| n == &name)
                .map(|(_, v)| SteelVal::StringV(v.clone().into()))
                .unwrap_or(SteelVal::BoolV(false))
        });

        // (test-region-active?) — whether a visual selection is active.
        let s = shared.clone();
        engine.register_fn("test-region-active?", move || -> bool {
            s.lock().unwrap().region_active
        });

        // (test-region-start) — start offset of the visual selection.
        let s = shared.clone();
        engine.register_fn("test-region-start", move || -> isize {
            s.lock().unwrap().region_start as isize
        });

        // (test-region-end) — end offset of the visual selection.
        let s = shared.clone();
        engine.register_fn("test-region-end", move || -> isize {
            s.lock().unwrap().region_end as isize
        });

        // (test-search-forward PATTERN) — search for PATTERN in active buffer text.
        // Returns the character offset of the first match, or #f if not found.
        let s = shared.clone();
        engine.register_fn("test-search-forward", move |pattern: String| -> SteelVal {
            let state = s.lock().unwrap();
            match state.current_buffer_text.find(&pattern) {
                Some(byte_offset) => {
                    // Convert byte offset to char offset.
                    let char_offset = state.current_buffer_text[..byte_offset].chars().count();
                    SteelVal::IntV(char_offset as isize)
                }
                None => SteelVal::BoolV(false),
            }
        });

        // (test-cursor-row) — cursor row (0-indexed) from SharedState.
        let s = shared.clone();
        engine.register_fn("test-cursor-row", move || -> isize {
            s.lock().unwrap().cursor_row as isize
        });

        // (test-cursor-col) — cursor column (0-indexed) from SharedState.
        let s = shared.clone();
        engine.register_fn("test-cursor-col", move || -> isize {
            s.lock().unwrap().cursor_col as isize
        });

        // (test-status-message) — last status bar message from SharedState.
        let s = shared.clone();
        engine.register_fn("test-status-message", move || -> String {
            s.lock().unwrap().last_status_message.clone()
        });

        // --- CRDT/sync test primitives ---

        // (buffer-enable-sync CLIENT-ID) — enable sync on active buffer.
        let s = shared.clone();
        engine.register_fn("buffer-enable-sync", move |client_id: isize| {
            s.lock().unwrap().pending_enable_sync = Some(client_id.max(1) as u64);
            SteelVal::Void
        });

        // (buffer-disable-sync) — disable sync on active buffer.
        let s = shared.clone();
        engine.register_fn("buffer-disable-sync", move || {
            s.lock().unwrap().pending_disable_sync = true;
            SteelVal::Void
        });

        // (buffer-apply-update BUFFER-NAME UPDATE-BASE64) — apply encoded sync update.
        let s = shared.clone();
        engine.register_fn(
            "buffer-apply-update",
            move |buf_name: String, update_b64: String| {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&update_b64) {
                    Ok(bytes) => {
                        s.lock()
                            .unwrap()
                            .pending_sync_applies
                            .push((buf_name, bytes));
                        SteelVal::BoolV(true)
                    }
                    Err(e) => SteelVal::StringV(format!("base64 decode error: {}", e).into()),
                }
            },
        );

        // (buffer-load-sync-state STATE-BASE64 CLIENT-ID) — load full state into active buffer.
        let s = shared.clone();
        engine.register_fn(
            "buffer-load-sync-state",
            move |state_b64: String, client_id: isize| {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&state_b64) {
                    Ok(bytes) => {
                        s.lock().unwrap().pending_load_sync_state =
                            Some((bytes, client_id.max(1) as u64));
                        SteelVal::BoolV(true)
                    }
                    Err(e) => SteelVal::StringV(format!("base64 decode error: {}", e).into()),
                }
            },
        );

        // (buffer-encode-state-vector) — request encoding of the active buffer's state vector.
        // The result is available via (buffer-get-state-vector) after the next apply cycle.
        let s = shared.clone();
        engine.register_fn("buffer-encode-state-vector", move || {
            s.lock().unwrap().pending_encode_state_vector = true;
            SteelVal::Void
        });

        // (buffer-get-state-vector) — retrieve the encoded state vector (base64) or #f.
        let s = shared.clone();
        engine.register_fn("buffer-get-state-vector", move || -> SteelVal {
            let state = s.lock().unwrap();
            match &state.encoded_state_vector {
                Some(sv) => SteelVal::StringV(sv.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // (buffer-compute-diff SV-BASE64) — compute diff from remote state vector.
        // The result is available via (buffer-get-diff) after the next apply cycle.
        let s = shared.clone();
        engine.register_fn("buffer-compute-diff", move |sv_b64: String| {
            s.lock().unwrap().pending_compute_diff = Some(sv_b64);
            SteelVal::Void
        });

        // (buffer-get-diff) — retrieve the computed diff (base64) or #f.
        let s = shared.clone();
        engine.register_fn("buffer-get-diff", move || -> SteelVal {
            let state = s.lock().unwrap();
            match &state.computed_diff {
                Some(d) => SteelVal::StringV(d.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // (buffer-reconcile-to TEXT) — reconcile sync doc to target text.
        // The result (base64 update) is available via (buffer-get-reconcile-result).
        let s = shared.clone();
        engine.register_fn("buffer-reconcile-to", move |text: String| {
            s.lock().unwrap().pending_reconcile_to = Some(text);
            SteelVal::Void
        });

        // (buffer-get-reconcile-result) — retrieve reconcile result (base64 update) or #f.
        let s = shared.clone();
        engine.register_fn("buffer-get-reconcile-result", move || -> SteelVal {
            let state = s.lock().unwrap();
            match &state.reconcile_result {
                Some(r) => SteelVal::StringV(r.clone().into()),
                None => SteelVal::BoolV(false),
            }
        });

        // Register default values for state-injected variables.
        // This prevents FreeIdentifier errors in init.scm during startup.
        engine.register_value("*buffer-name*", SteelVal::StringV("scratch".into()));
        engine.register_value("*buffer-modified?*", SteelVal::BoolV(false));
        engine.register_value("*buffer-line-count*", SteelVal::IntV(0));
        engine.register_value("*buffer-char-count*", SteelVal::IntV(0));
        engine.register_value("*cursor-row*", SteelVal::IntV(1));
        engine.register_value("*cursor-col*", SteelVal::IntV(1));
        engine.register_value("*mode*", SteelVal::StringV("normal".into()));
        engine.register_value("*shell-buffers*", SteelVal::ListV(vec![].into()));

        // Build default load-path: ~/.config/mae/packages/, ~/.config/mae/lisp/
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

        // Seed SharedState load_path so Scheme functions can read/modify it.
        {
            let mut state = shared.lock().unwrap();
            state.load_path = default_load_path.clone();
        }

        Ok(SchemeRuntime {
            engine,
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
        self.shared.lock().unwrap().declared_modules.clone()
    }

    /// Return declared packages from `(package! ...)`.
    pub fn declared_packages(&self) -> Vec<DeclaredPackage> {
        self.shared.lock().unwrap().declared_packages.clone()
    }

    /// Set the current module directory for relative path resolution.
    /// Called by the module loader before evaluating each module's autoloads.
    pub fn set_module_dir(&mut self, dir: Option<&Path>) {
        self.shared.lock().unwrap().current_module_dir = dir.map(|d| d.to_path_buf());
    }

    /// Drain pending KB nodes registered via `(define-kb-node! ...)`.
    pub fn drain_kb_nodes(&mut self) -> Vec<(String, String, String)> {
        let mut state = self.shared.lock().unwrap();
        std::mem::take(&mut state.pending_kb_nodes)
    }

    // --- Test framework accessors ---

    /// Take the pending exit code set by `(exit CODE)`, if any.
    pub fn take_exit_code(&mut self) -> Option<i32> {
        self.shared.lock().unwrap().pending_exit_code.take()
    }

    /// Update the current mode string in SharedState (for test runner).
    pub fn set_current_mode(&self, mode: &str) {
        self.shared.lock().unwrap().current_mode = mode.to_string();
    }

    /// Update the active buffer text in SharedState (for test runner).
    pub fn set_current_buffer_text(&self, text: &str) {
        self.shared.lock().unwrap().current_buffer_text = text.to_string();
    }

    /// Update all buffer texts in SharedState (for test runner).
    pub fn set_all_buffer_texts(&self, texts: Vec<(String, String)>) {
        self.shared.lock().unwrap().all_buffer_texts = texts;
    }

    /// Update sync state in SharedState (for test runner).
    pub fn set_sync_state(
        &self,
        enabled: bool,
        pending_count: usize,
        content: Option<String>,
        encoded: Option<String>,
    ) {
        let mut state = self.shared.lock().unwrap();
        state.sync_enabled = enabled;
        state.pending_update_count = pending_count;
        state.sync_content = content;
        state.encoded_state = encoded;
    }

    /// Update buffer names in SharedState for `(get-buffer-by-name)` across tests.
    pub fn set_buffer_names(&self, names: Vec<(usize, String)>) {
        self.shared.lock().unwrap().buffer_names = names;
    }

    /// Update option values in SharedState for test runner.
    pub fn set_option_values(&self, values: Vec<(String, String)>) {
        self.shared.lock().unwrap().option_values = values;
    }

    /// Update region (visual selection) state in SharedState for test runner.
    pub fn set_region_state(&self, active: bool, start: usize, end: usize) {
        let mut state = self.shared.lock().unwrap();
        state.region_active = active;
        state.region_start = start;
        state.region_end = end;
    }

    /// Update cursor position in SharedState (called by test runner).
    pub fn set_cursor_position(&self, row: usize, col: usize) {
        let mut state = self.shared.lock().unwrap();
        state.cursor_row = row;
        state.cursor_col = col;
    }

    /// Update last status message in SharedState (called by test runner).
    pub fn set_last_status_message(&self, msg: &str) {
        self.shared.lock().unwrap().last_status_message = msg.to_string();
    }

    /// Drain pending file writes from `(write-file PATH CONTENT)`.
    pub fn drain_write_files(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.shared.lock().unwrap().pending_write_files)
    }

    /// Take the pending sleep request from `(sleep-ms N)`, if any.
    pub fn take_sleep_ms(&mut self) -> Option<u64> {
        self.shared.lock().unwrap().pending_sleep_ms.take()
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

        // *buffer-language* — current buffer's detected language (or "text")
        let active_idx = editor.active_buffer_idx();
        let lang_str = editor
            .syntax
            .language_for(active_idx)
            .map(|l| l.id())
            .unwrap_or("text");
        self.engine
            .register_value("*buffer-language*", SteelVal::StringV(lang_str.into()));

        // *buffer-file-path* — current buffer's file path (empty if unsaved)
        let file_path_str = buf
            .file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.engine.register_value(
            "*buffer-file-path*",
            SteelVal::StringV(file_path_str.into()),
        );

        // (buffer-line N) — read a specific line (0-indexed). Capture
        // a snapshot of all lines so the closure is self-contained.
        let lines: Vec<String> = (0..buf.line_count())
            .map(|i| buf.line_text(i).to_string())
            .collect();
        let lines = std::sync::Arc::new(lines);
        self.engine.register_fn("buffer-line", move |n: isize| {
            lines.get(n.max(0) as usize).cloned().unwrap_or_default()
        });

        // --- Shell state ---

        // *shell-buffers* — list of buffer indices that are Shell-kind.
        let shell_indices: Vec<SteelVal> = editor
            .buffers
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == mae_core::BufferKind::Shell)
            .map(|(i, _)| SteelVal::IntV(i as isize))
            .collect();
        self.engine
            .register_value("*shell-buffers*", SteelVal::ListV(shell_indices.into()));

        // (shell-cwd BUF-IDX) — return cached CWD for a shell buffer.
        let cwds = editor.shell_cwds.clone();
        self.engine.register_fn("shell-cwd", move |idx: isize| {
            cwds.get(&(idx.max(0) as usize))
                .cloned()
                .unwrap_or_default()
        });

        // (shell-read-output BUF-IDX MAX-LINES) — read viewport snapshot.
        let viewports = editor.shell_viewports.clone();
        self.engine
            .register_fn("shell-read-output", move |idx: isize, max: isize| {
                let idx = idx.max(0) as usize;
                let max = max.max(1) as usize;
                viewports
                    .get(&idx)
                    .map(|lines| {
                        let start = lines.len().saturating_sub(max);
                        lines[start..].join("\n")
                    })
                    .unwrap_or_default()
            });

        // *current-command* — name of the command currently being dispatched
        self.engine.register_value(
            "*current-command*",
            SteelVal::StringV(editor.current_command.clone().into()),
        );

        // --- A1: Buffer introspection functions (callable forms) ---

        let buf_name = buf.name.clone();
        self.engine
            .register_fn("current-buffer-name", move || buf_name.clone());

        let file_path = buf.file_path().map(|p| p.display().to_string());
        self.engine
            .register_fn("current-buffer-file", move || -> SteelVal {
                match &file_path {
                    Some(p) => SteelVal::StringV(p.clone().into()),
                    None => SteelVal::BoolV(false),
                }
            });

        let line_num = (win.cursor_row + 1) as isize;
        self.engine
            .register_fn("current-line-number", move || line_num);

        let col = win.cursor_col as isize;
        self.engine.register_fn("current-column", move || col);

        let cursor_offset = buf.char_offset_at(win.cursor_row, win.cursor_col) as isize;
        self.engine.register_fn("point", move || cursor_offset);

        self.engine.register_fn("point-min", || 0isize);

        let max_chars = buf.rope().len_chars() as isize;
        self.engine.register_fn("point-max", move || max_chars);

        let line_begin = buf.rope().line_to_char(win.cursor_row) as isize;
        self.engine
            .register_fn("line-beginning-position", move || line_begin);

        let line_end = if win.cursor_row + 1 < buf.line_count() {
            buf.rope().line_to_char(win.cursor_row + 1) as isize - 1
        } else {
            buf.rope().len_chars() as isize
        };
        self.engine
            .register_fn("line-end-position", move || line_end);

        // --- A2: Selection / region access ---

        let is_visual = matches!(editor.mode, mae_core::Mode::Visual(_));
        self.engine.register_fn("region-active?", move || is_visual);

        // Compute region bounds (valid only in visual mode, but safe to call anytime)
        let (region_beg, region_end, selection_text) = if is_visual {
            let anchor_offset =
                buf.char_offset_at(editor.visual_anchor_row, editor.visual_anchor_col);
            let cursor_off = buf.char_offset_at(win.cursor_row, win.cursor_col);
            let beg = anchor_offset.min(cursor_off);
            let end = anchor_offset.max(cursor_off) + 1; // inclusive end
            let end = end.min(buf.rope().len_chars());
            let text: String = buf.rope().chars().skip(beg).take(end - beg).collect();
            (beg as isize, end as isize, text)
        } else {
            (0isize, 0isize, String::new())
        };
        let rb = region_beg;
        self.engine.register_fn("region-beginning", move || rb);
        let re = region_end;
        self.engine.register_fn("region-end", move || re);
        let st = selection_text;
        self.engine.register_fn("get-selection", move || st.clone());

        // --- Round 2: extended introspection ---

        // *buffer-char-count* — total chars in the active buffer
        self.engine.register_value(
            "*buffer-char-count*",
            SteelVal::IntV(buf.rope().len_chars() as isize),
        );

        // (buffer-text-range START END) — substring of buffer text
        let text_for_range = buf.text();
        self.engine
            .register_fn("buffer-text-range", move |start: isize, end: isize| {
                let s = start.max(0) as usize;
                let e = end.max(0) as usize;
                text_for_range
                    .chars()
                    .skip(s)
                    .take(e.saturating_sub(s))
                    .collect::<String>()
            });

        // *buffer-list* — list of (index name kind modified?)
        let buf_info: Vec<SteelVal> = editor
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| {
                SteelVal::ListV(
                    vec![
                        SteelVal::IntV(i as isize),
                        SteelVal::StringV(b.name.clone().into()),
                        SteelVal::StringV(format!("{:?}", b.kind).into()),
                        SteelVal::BoolV(b.modified),
                    ]
                    .into(),
                )
            })
            .collect();
        self.engine
            .register_value("*buffer-list*", SteelVal::ListV(buf_info.into()));

        // (get-buffer-by-name NAME) — returns index or #f
        let buffer_names: Vec<(usize, String)> = editor
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| (i, b.name.clone()))
            .collect();
        self.engine
            .register_fn("get-buffer-by-name", move |name: String| -> SteelVal {
                buffer_names
                    .iter()
                    .find(|(_, n)| n == &name)
                    .map(|(i, _)| SteelVal::IntV(*i as isize))
                    .unwrap_or(SteelVal::BoolV(false))
            });

        // *window-count*
        self.engine.register_value(
            "*window-count*",
            SteelVal::IntV(editor.window_mgr.window_count() as isize),
        );

        // *window-list* — list of (id buffer-idx cursor-row cursor-col)
        let win_info: Vec<SteelVal> = editor
            .window_mgr
            .iter_windows()
            .map(|w| {
                SteelVal::ListV(
                    vec![
                        SteelVal::IntV(w.id as isize),
                        SteelVal::IntV(w.buffer_idx as isize),
                        SteelVal::IntV(w.cursor_row as isize),
                        SteelVal::IntV(w.cursor_col as isize),
                    ]
                    .into(),
                )
            })
            .collect();
        self.engine
            .register_value("*window-list*", SteelVal::ListV(win_info.into()));

        // *option-list* — list of (name kind default doc)
        let opt_info: Vec<SteelVal> = editor
            .option_registry
            .list()
            .iter()
            .map(|o| {
                SteelVal::ListV(
                    vec![
                        SteelVal::StringV(o.name.as_ref().into()),
                        SteelVal::StringV(format!("{}", o.kind).into()),
                        SteelVal::StringV(o.default_value.as_ref().into()),
                        SteelVal::StringV(o.doc.as_ref().into()),
                    ]
                    .into(),
                )
            })
            .collect();
        self.engine
            .register_value("*option-list*", SteelVal::ListV(opt_info.into()));

        // Populate SharedState option_values so get-option has initial data.
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
            self.shared.lock().unwrap().option_values = values;
        }

        // (get-option NAME) — returns current value as string, or #f
        // Reads from SharedState so values are fresh after sync_scheme_state.
        let s = self.shared.clone();
        self.engine
            .register_fn("get-option", move |name: String| -> SteelVal {
                let state = s.lock().unwrap();
                state
                    .option_values
                    .iter()
                    .find(|(n, _)| n == &name)
                    .map(|(_, v)| SteelVal::StringV(v.clone().into()))
                    .unwrap_or(SteelVal::BoolV(false))
            });

        // *command-list* — list of (name doc source)
        let cmd_info: Vec<SteelVal> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| {
                SteelVal::ListV(
                    vec![
                        SteelVal::StringV(c.name.clone().into()),
                        SteelVal::StringV(c.doc.clone().into()),
                        SteelVal::StringV(format!("{:?}", c.source).into()),
                    ]
                    .into(),
                )
            })
            .collect();
        self.engine
            .register_value("*command-list*", SteelVal::ListV(cmd_info.into()));

        // (command-exists? NAME)
        let cmd_names: Vec<String> = editor
            .commands
            .list_commands()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        self.engine
            .register_fn("command-exists?", move |name: String| -> bool {
                cmd_names.iter().any(|n| n == &name)
            });

        // *keymap-list* — list of keymap names
        let keymap_names: Vec<SteelVal> = editor
            .keymaps
            .keys()
            .map(|k| SteelVal::StringV(k.clone().into()))
            .collect();
        self.engine
            .register_value("*keymap-list*", SteelVal::ListV(keymap_names.into()));

        // (keymap-bindings MAP-NAME) — list of (key-display command-name)
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
        self.engine
            .register_fn("keymap-bindings", move |name: String| -> SteelVal {
                keymaps_snapshot
                    .get(&name)
                    .map(|bindings: &Vec<(String, String)>| {
                        SteelVal::ListV(
                            bindings
                                .iter()
                                .map(|(k, c): &(String, String)| {
                                    SteelVal::ListV(
                                        vec![
                                            SteelVal::StringV(k.clone().into()),
                                            SteelVal::StringV(c.clone().into()),
                                        ]
                                        .into(),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .into(),
                        )
                    })
                    .unwrap_or(SteelVal::ListV(vec![].into()))
            });

        // (buffer-string) — return full text of the active buffer (ERT naming).
        let active_text = buf.text();
        self.engine
            .register_fn("buffer-string", move || -> String { active_text.clone() });

        // (buffer-text NAME) — return full text of a named buffer.
        let all_buf_texts: Vec<(String, String)> = editor
            .buffers
            .iter()
            .map(|b| (b.name.clone(), b.text()))
            .collect();
        self.engine
            .register_fn("buffer-text", move |name: String| -> SteelVal {
                all_buf_texts
                    .iter()
                    .find(|(n, _)| n == &name || n.ends_with(&name))
                    .map(|(_, t)| SteelVal::StringV(t.clone().into()))
                    .unwrap_or(SteelVal::BoolV(false))
            });

        // (collab-status) — returns an alist with current collaboration state.
        // Returns: ((status . "off") (server . "127.0.0.1:9473") (synced-docs . 0) (peer-count . 0))
        let collab_status_str = editor.collab_status.as_str().to_string();
        let collab_server_addr = editor.collab_server_address.clone();
        let collab_synced_docs = editor.collab_synced_docs;
        self.engine
            .register_fn("collab-status", move || -> SteelVal {
                let make_pair = |k: &str, v: SteelVal| -> SteelVal {
                    SteelVal::ListV(vec![SteelVal::StringV(k.into()), v].into())
                };
                SteelVal::ListV(
                    vec![
                        make_pair(
                            "status",
                            SteelVal::StringV(collab_status_str.clone().into()),
                        ),
                        make_pair(
                            "server",
                            SteelVal::StringV(collab_server_addr.clone().into()),
                        ),
                        make_pair("synced-docs", SteelVal::IntV(collab_synced_docs as isize)),
                        make_pair("peer-count", SteelVal::IntV(0)),
                    ]
                    .into(),
                )
            });

        // (collab-synced-buffers) — returns a list of synced buffer names.
        let synced_names: Vec<String> = editor.collab_synced_buffers.iter().cloned().collect();
        self.engine
            .register_fn("collab-synced-buffers", move || -> SteelVal {
                SteelVal::ListV(
                    synced_names
                        .iter()
                        .map(|n| SteelVal::StringV(n.clone().into()))
                        .collect::<Vec<_>>()
                        .into(),
                )
            });

        // --- Sync/CRDT state inspection ---

        // (buffer-sync-enabled?) — #t if sync_doc is active on the current buffer.
        let sync_enabled = buf.sync_doc.is_some();
        self.engine
            .register_value("*buffer-sync-enabled?*", SteelVal::BoolV(sync_enabled));
        self.engine
            .register_fn("buffer-sync-enabled?", move || sync_enabled);

        // (buffer-pending-updates) — number of pending sync updates on active buffer.
        let pending_count = buf.pending_sync_updates.len() as isize;
        self.engine
            .register_fn("buffer-pending-updates", move || pending_count);

        // (buffer-sync-content) — read content from the yrs doc (not the rope).
        let sync_content = buf.sync_doc.as_ref().map(|s| s.content());
        self.engine
            .register_fn("buffer-sync-content", move || -> SteelVal {
                match &sync_content {
                    Some(c) => SteelVal::StringV(c.clone().into()),
                    None => SteelVal::BoolV(false),
                }
            });

        // (buffer-drain-updates) — request drain of pending sync updates.
        // Sets a flag in SharedState; apply_to_editor drains the actual updates
        // and stores them as base64 strings. Returns the previously drained list.
        let s = self.shared.clone();
        self.engine
            .register_fn("buffer-drain-updates", move || -> SteelVal {
                let mut state = s.lock().unwrap();
                state.pending_drain_sync_updates = true;
                // Return previously drained updates (from last apply cycle).
                let updates = std::mem::take(&mut state.drained_sync_updates);
                SteelVal::ListV(
                    updates
                        .into_iter()
                        .map(|s| SteelVal::StringV(s.into()))
                        .collect::<Vec<_>>()
                        .into(),
                )
            });

        // (buffer-encode-state) — return full yrs document state as base64.
        let encoded_state = buf.sync_doc.as_ref().map(|s| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(s.encode_state())
        });
        self.engine
            .register_fn("buffer-encode-state", move || -> SteelVal {
                match &encoded_state {
                    Some(s) => SteelVal::StringV(s.clone().into()),
                    None => SteelVal::BoolV(false),
                }
            });

        // (undo-available?) — #t if undo stack is non-empty.
        let has_undo = buf.has_undo();
        self.engine.register_fn("undo-available?", move || has_undo);

        // (redo-available?) — #t if redo stack is non-empty.
        let has_redo = buf.has_redo();
        self.engine.register_fn("redo-available?", move || has_redo);
    }

    /// Apply accumulated config changes to the editor.
    /// Call this after loading init.scm or after REPL eval.
    pub fn apply_to_editor(&mut self, editor: &mut Editor) {
        let mut state = self.shared.lock().unwrap();

        // Create new keymaps (must come before bindings so define-key can target them)
        for (name, parent) in state.keymap_defs.drain(..) {
            if !editor.keymaps.contains_key(&name) {
                debug!(keymap = %name, parent = %parent, "creating scheme keymap");
                editor
                    .keymaps
                    .insert(name.clone(), mae_core::Keymap::with_parent(&name, &parent));
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
            editor.kb.insert(node);
            debug!(id = %id, "kb node registered from scheme");
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

        // (buffer-drain-updates) — drain pending sync updates from active buffer
        if state.pending_drain_sync_updates {
            state.pending_drain_sync_updates = false;
            let idx = editor.active_buffer_idx();
            let updates: Vec<String> = editor.buffers[idx]
                .pending_sync_updates
                .drain(..)
                .map(|u| {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&u)
                })
                .collect();
            state.drained_sync_updates = updates;
        }

        // (buffer-apply-update BUFFER-NAME UPDATE-BYTES)
        let sync_applies: Vec<(String, Vec<u8>)> = state.pending_sync_applies.drain(..).collect();
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
                editor.alternate_buffer_idx = Some(prev);
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
        let commands: Vec<String> = state.pending_commands.drain(..).collect();

        // (execute-ex CMD) — dispatch through ex-command parser (supports args).
        let ex_commands: Vec<String> = state.pending_ex_commands.drain(..).collect();

        // (message TEXT) — append to message log
        for msg in state.pending_messages.drain(..) {
            info!("[scheme] {}", msg);
        }

        // (shell-send-input BUF-IDX TEXT) — queue shell terminal input.
        for (buf_idx, text) in state.pending_shell_inputs.drain(..) {
            editor.pending_shell_inputs.push((buf_idx, text));
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
        let mut ai_tools: Vec<mae_core::SchemeToolDef> = state.pending_ai_tools.drain(..).collect();
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
                .scheme_ai_tools
                .iter_mut()
                .find(|t| t.name == tool.name)
            {
                *existing = tool;
            } else {
                editor.scheme_ai_tools.push(tool);
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

        // Note: We do NOT call inject_editor_state here because Steel's
        // register_value creates new binding cells. Closures captured in
        // previous evals would still reference old cells. The test runner
        // uses sync_scheme_state (with set!) to mutate existing cells.
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
            let state = self.shared.lock().unwrap();
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

        self.engine
            .run(content)
            .map_err(|e| format!("Error loading feature '{}': {}", name, e))?;

        // Check if provide was called during loading.
        {
            let state = self.shared.lock().unwrap();
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
            let mut state = self.shared.lock().unwrap();
            state.pending_requires.drain(..).collect()
        };

        // Sync load_path from SharedState (add-to-load-path! may have modified it).
        {
            let state = self.shared.lock().unwrap();
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

    /// Isolate Steel's filesystem state so tests don't race with other
    /// test binaries accessing `~/.steel/cached-modules/`.
    fn isolate_steel_home() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let dir =
                std::env::temp_dir().join(format!("steel-scheme-test-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            std::env::set_var("STEEL_HOME", &dir);
        });
    }

    fn new_runtime() -> SchemeRuntime {
        isolate_steel_home();
        SchemeRuntime::new().unwrap()
    }

    #[test]
    fn new_runtime_creates_successfully() {
        isolate_steel_home();
        let rt = SchemeRuntime::new();
        assert!(rt.is_ok());
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
        editor.shell_cwds.insert(1, "/home/user".to_string());
        rt.inject_editor_state(&editor);
        let result = rt.eval("(shell-cwd 1)").unwrap();
        assert_eq!(result, "/home/user");
    }

    #[test]
    fn test_shell_read_output_returns_viewport() {
        let mut rt = new_runtime();
        let mut editor = Editor::new();
        editor
            .shell_viewports
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

        // Evaluate scheme calls
        runtime
            .eval("(recent-files-add! \"/tmp/test.txt\")")
            .unwrap();
        runtime
            .eval("(recent-projects-add! \"/tmp/project\")")
            .unwrap();

        // Apply to editor
        runtime.apply_to_editor(&mut editor);

        // Verify editor state updated
        assert_eq!(editor.recent_files.len(), 1);
        assert_eq!(
            editor.recent_files.list()[0],
            std::path::PathBuf::from("/tmp/test.txt")
        );
        assert_eq!(editor.recent_projects.len(), 1);
        assert_eq!(
            editor.recent_projects.list()[0],
            std::path::PathBuf::from("/tmp/project")
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
        // Steel's built-in `provide` shadows any redefinition, so packages
        // must use `provide-feature`.
        rt.eval(r#"(provide-feature "my-feature")"#).unwrap();
        {
            let state = rt.shared.lock().unwrap();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        isolate_steel_home();
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
        let state = rt.shared.lock().unwrap();
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

        let node = editor.kb.get("module:test:guide");
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
