//! Headless test runner for Scheme-based editor tests.
//!
//! Inspired by Emacs `--batch` + ERT and Neovim `--headless` + plenary.
//!
//! Architecture:
//! 1. Boot editor headless (no terminal, no GUI)
//! 2. Start full event loop (collab bridge, scheme runtime)
//! 3. Load `mae-test.scm` library automatically
//! 4. Load and evaluate test file(s)
//! 5. Between each Scheme eval, drain collab/shell events and process side effects
//! 6. Exit with code 0 (all pass) or 1 (any fail)

use std::path::Path;
use std::time::Duration;

use mae_core::Editor;
use mae_scheme::SchemeRuntime;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};

use crate::collab_bridge::{CollabCommand, CollabEvent};
use std::path::PathBuf;

/// A test file and the range of test indices it registered.
struct FileBoundary {
    file: PathBuf,
    start_idx: usize,
    end_idx: usize,
}

/// Snapshot of global editor state that should be consistent across test files.
/// Restored at file boundaries to prevent cross-file pollution.
struct EditorStateSnapshot {
    mode: mae_core::Mode,
    keymap_flavor: String,
    default_mode: String,
    option_line_numbers: String,
    option_word_wrap: String,
}

/// Run the Scheme test runner in headless mode.
///
/// Returns exit code: 0 = success, 1 = test failure, 2 = runtime error.
pub(crate) async fn run_scheme_tests(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    test_path: &str,
    test_filter: Option<&str>,
    _output_format: &str,
) -> i32 {
    info!(path = test_path, "starting scheme test runner");

    // Create a no-op broadcaster for drain_and_broadcast (no MCP clients in tests,
    // but the function needs it to forward pending_sync_updates to collab_command_tx).
    let broadcaster: SharedBroadcaster =
        std::sync::Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));

    // Load the mae-test.scm library.
    let lib_path = find_test_library();
    match &lib_path {
        Some(path) => {
            info!(path = %path.display(), "loading mae-test.scm");
            scheme.inject_editor_state(editor);
            if let Err(e) = scheme.load_file(path) {
                eprintln!("mae-test: failed to load mae-test.scm: {}", e.message);
                return 2;
            }
            scheme.apply_to_editor(editor);
            process_side_effects(
                editor,
                scheme,
                collab_event_rx,
                collab_command_tx,
                &broadcaster,
            )
            .await;
        }
        None => {
            eprintln!("mae-test: cannot find mae-test.scm library");
            return 2;
        }
    }

    // Determine test files to load.
    let test_files = collect_test_files(test_path);
    if test_files.is_empty() {
        eprintln!("mae-test: no .scm test files found at '{}'", test_path);
        return 2;
    }

    info!(count = test_files.len(), "found test files");

    // Set the test filter if provided.
    if let Some(filter) = test_filter {
        let filter_code = format!(
            r#"(define *test-filter* "{}")"#,
            filter.replace('"', "\\\"")
        );
        let _ = scheme.eval(&filter_code);
    }

    // Load and evaluate each test file, tracking file→test-index boundaries
    // so the runner can isolate state between files.
    let mut file_boundaries: Vec<FileBoundary> = Vec::new();
    let mut prev_count: usize = 0;

    for file in &test_files {
        info!(file = %file.display(), "loading test file");
        scheme.inject_editor_state(editor);

        if let Err(e) = scheme.load_file(file) {
            eprintln!("mae-test: error loading {}: {}", file.display(), e.message);
            return 2;
        }

        // Process side effects after loading (runs describe/it registrations).
        scheme.apply_to_editor(editor);
        process_side_effects(
            editor,
            scheme,
            collab_event_rx,
            collab_command_tx,
            &broadcaster,
        )
        .await;

        // Record how many tests this file registered.
        let cur_count: usize = scheme
            .eval("(test-count)")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(prev_count);
        if cur_count > prev_count {
            file_boundaries.push(FileBoundary {
                file: file.clone(),
                start_idx: prev_count,
                end_idx: cur_count,
            });
        }
        prev_count = cur_count;

        // Check for exit request.
        if let Some(code) = scheme.take_exit_code() {
            return code;
        }
    }

    // Check for exit request from test file (e.g., inline `(run-tests)` call).
    if let Some(code) = scheme.take_exit_code() {
        return code;
    }

    // Rust-side test iteration: run each test with inject/apply between them.
    // This ensures buffer-string/buffer-text see fresh state after mutations.
    run_tests_iteratively(
        editor,
        scheme,
        collab_event_rx,
        collab_command_tx,
        &broadcaster,
        &file_boundaries,
    )
    .await
}

impl EditorStateSnapshot {
    fn capture(editor: &Editor) -> Self {
        Self {
            mode: editor.mode,
            keymap_flavor: editor.keymap_flavor.clone(),
            default_mode: editor.default_mode.clone(),
            option_line_numbers: editor
                .get_option("line_numbers")
                .map(|(v, _)| v)
                .unwrap_or_default(),
            option_word_wrap: editor
                .get_option("word_wrap")
                .map(|(v, _)| v)
                .unwrap_or_default(),
        }
    }

    fn restore(&self, editor: &mut Editor, scheme: &mut SchemeRuntime) -> Vec<String> {
        let mut dirty = Vec::new();

        if editor.mode != self.mode {
            dirty.push(format!("mode: {:?} → {:?}", self.mode, editor.mode));
            editor.set_mode(self.mode);
        }
        if editor.keymap_flavor != self.keymap_flavor {
            dirty.push(format!(
                "keymap_flavor: {} → {}",
                self.keymap_flavor, editor.keymap_flavor
            ));
            crate::bootstrap::switch_keymap_flavor(scheme, editor, &self.keymap_flavor);
        }
        if editor.default_mode != self.default_mode {
            dirty.push(format!(
                "default_mode: {} → {}",
                self.default_mode, editor.default_mode
            ));
            let _ = editor.set_option("default_mode", &self.default_mode);
        }
        if let Some((cur, _)) = editor.get_option("line_numbers") {
            if cur != self.option_line_numbers {
                dirty.push(format!(
                    "line_numbers: {} → {}",
                    self.option_line_numbers, cur
                ));
                let _ = editor.set_option("line_numbers", &self.option_line_numbers);
            }
        }
        if let Some((cur, _)) = editor.get_option("word_wrap") {
            if cur != self.option_word_wrap {
                dirty.push(format!("word_wrap: {} → {}", self.option_word_wrap, cur));
                let _ = editor.set_option("word_wrap", &self.option_word_wrap);
            }
        }

        dirty
    }
}

/// Run all registered tests one-by-one from the Rust side.
///
/// Between each test, we call inject_editor_state + apply_to_editor + process_side_effects
/// so that buffer mutations from one test are visible in subsequent tests.
///
/// At file boundaries (tracked by `file_boundaries`), global editor state is
/// snapshot before the first test and restored after the last — preventing
/// cross-file pollution from mode, flavor, or option changes.
async fn run_tests_iteratively(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    broadcaster: &SharedBroadcaster,
    file_boundaries: &[FileBoundary],
) -> i32 {
    // Query test count.
    let count_str = match scheme.eval("(test-count)") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mae-test: error querying test count: {}", e.message);
            return 2;
        }
    };
    let count: usize = count_str.trim().parse().unwrap_or(0);
    if count == 0 {
        eprintln!("mae-test: no tests registered");
        return 2;
    }

    // TAP header.
    println!("TAP version 14");
    println!("1..{}", count);

    // Sync state so first test sees current editor state.
    scheme.inject_editor_state(editor);

    let mut pass_count = 0usize;
    let mut fail_count = 0usize;
    let mut file_snapshot: Option<EditorStateSnapshot> = None;
    let mut current_file_idx: Option<usize> = None;

    for i in 0..count {
        // File boundary: snapshot state at the start of a new file's tests,
        // restore at the end of the previous file's tests.
        let new_file_idx = file_boundaries
            .iter()
            .position(|fb| i >= fb.start_idx && i < fb.end_idx);
        if new_file_idx != current_file_idx {
            // Restore state from the file that just ended.
            if let Some(snapshot) = file_snapshot.take() {
                if let Some(prev_idx) = current_file_idx {
                    let prev_file = file_boundaries[prev_idx]
                        .file
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?");
                    let dirty = snapshot.restore(editor, scheme);
                    if !dirty.is_empty() {
                        eprintln!(
                            "# warning: {} leaked global state (auto-restored): {}",
                            prev_file,
                            dirty.join(", ")
                        );
                    }
                    scheme.inject_editor_state(editor);
                }
            }
            // Snapshot state for the new file.
            file_snapshot = Some(EditorStateSnapshot::capture(editor));
            current_file_idx = new_file_idx;
            if let Some(idx) = new_file_idx {
                let file_name = file_boundaries[idx]
                    .file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?");
                debug!(file = file_name, test_idx = i, "entering test file");
            }
        }

        // Get test name.
        let name = scheme
            .eval(&format!("(test-name {})", i))
            .unwrap_or_else(|_| format!("test-{}", i));
        let name = name.trim().trim_matches('"').to_string();

        // Run the test with yield support — sleep-ms yields control so we
        // can drain collab/shell events during the wait.
        let result = eval_with_yields(
            editor,
            scheme,
            &format!("(run-nth-test {})", i),
            collab_event_rx,
            collab_command_tx,
            broadcaster,
        )
        .await;
        let result = result.trim().trim_matches('"').to_string();

        // Apply remaining side effects (buffer mutations, commands, writes).
        scheme.apply_to_editor(editor);
        process_side_effects(
            editor,
            scheme,
            collab_event_rx,
            collab_command_tx,
            broadcaster,
        )
        .await;

        // E2E key injection: process any `(feed-keys ...)` queued this step
        // through the REAL handle_key pipeline (real loaded keymaps + routing +
        // which-key + dispatch), then drain hooks it fired.
        drain_feed_keys(editor, scheme);

        // Process module reloads / flavor switches queued this step — by commands
        // (`:reload-modules`, `:keymap-set-flavor`) OR by a fed keypress (e.g. the
        // flavor picker's palette selection). Runs AFTER feed-keys so picker-
        // triggered switches apply in the same step. Mirrors the real event loops.
        drain_module_reloads(editor, scheme);

        // Refresh editor state so the next test sees updated globals.
        scheme.inject_editor_state(editor);

        // Check for exit request mid-test.
        if let Some(code) = scheme.take_exit_code() {
            return code;
        }

        // Print TAP line.
        let test_num = i + 1;
        if result == "PASS" {
            pass_count += 1;
            println!("ok {} - {}", test_num, name);
        } else {
            fail_count += 1;
            let msg = result.strip_prefix("FAIL:").unwrap_or(&result);
            println!("not ok {} - {}", test_num, name);
            println!("  ---");
            println!("  message: {}", msg);
            // Dump active buffer state on failure for diagnostics.
            let ab = editor.active_buffer();
            println!("  active_buffer: {}", ab.name);
            println!("  text_len: {}", ab.text().len());
            println!(
                "  text_preview: {:?}",
                ab.text().chars().take(200).collect::<String>()
            );
            println!("  sync_enabled: {}", ab.sync_doc.is_some());
            println!("  collab_doc_id: {:?}", ab.collab_doc_id);
            println!("  buffer_count: {}", editor.buffers.len());
            for (bi, b) in editor.buffers.iter().enumerate() {
                println!(
                    "  buf[{}]: name={:?} text_len={} sync={} collab_id={:?}",
                    bi,
                    b.name,
                    b.text().len(),
                    b.sync_doc.is_some(),
                    b.collab_doc_id
                );
            }
            println!("  ...");
        }
    }

    // Restore state from the last file.
    if let Some(snapshot) = file_snapshot.take() {
        if let Some(prev_idx) = current_file_idx {
            let prev_file = file_boundaries
                .get(prev_idx)
                .and_then(|fb| fb.file.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("?");
            let dirty = snapshot.restore(editor, scheme);
            if !dirty.is_empty() {
                eprintln!(
                    "# warning: {} leaked global state (auto-restored): {}",
                    prev_file,
                    dirty.join(", ")
                );
            }
        }
    }

    // Summary.
    println!();
    println!("# {} passed, {} failed", pass_count, fail_count);

    // WU5: Dump *messages* buffer on failure for diagnostics.
    if fail_count > 0 {
        if let Some(msg_buf) = editor.buffers.iter().find(|b| b.name == "*messages*") {
            let messages = msg_buf.text();
            if !messages.is_empty() {
                eprintln!();
                eprintln!("--- *messages* buffer ({} chars) ---", messages.len());
                for line in messages.lines().rev().take(50) {
                    eprintln!("  {}", line);
                }
                eprintln!("--- end *messages* ---");
            }
        }
        1
    } else {
        0
    }
}

/// Evaluate Scheme code with yield support.
///
/// Uses `eval_yielding` so that `sleep-ms` and `wait-for-file` yield control
/// back to Rust. During yields, we drain collab events — enabling collab tests
/// to observe state changes between sleep intervals.
async fn eval_with_yields(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    code: &str,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    broadcaster: &SharedBroadcaster,
) -> String {
    use mae_scheme::{vm::YieldRequest, SchemeEvalResult};

    let mut eval_result = match scheme.eval_yielding(code) {
        Ok(r) => r,
        Err(e) => return format!("FAIL:{}", e.message),
    };

    loop {
        match eval_result {
            SchemeEvalResult::Done(s) => return s,
            SchemeEvalResult::Yield(ref req) => {
                match req {
                    YieldRequest::Sleep(d) => {
                        let ms = d.as_millis() as u64;
                        // Apply side effects before sleeping (buffer mutations
                        // from code that ran before the yield).
                        scheme.apply_to_editor(editor);
                        // Drain collab intents (share/join/etc.) so they reach
                        // the bridge during the sleep, not just between steps.
                        crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
                        // Forward pending sync updates to state server.
                        crate::sync_broadcast::drain_and_broadcast(
                            editor,
                            broadcaster,
                            Some(collab_command_tx),
                        );
                        drain_events_for(
                            editor,
                            collab_event_rx,
                            collab_command_tx,
                            broadcaster,
                            ms,
                        )
                        .await;
                        scheme.inject_editor_state(editor);
                    }
                    YieldRequest::WaitForFile(path, timeout) => {
                        let deadline = tokio::time::Instant::now()
                            + Duration::from_millis(timeout.as_millis() as u64);
                        let poll_interval = Duration::from_millis(50);
                        loop {
                            if path.exists() {
                                break;
                            }
                            if tokio::time::Instant::now() >= deadline {
                                return format!("FAIL:wait-for-file timed out: {}", path.display());
                            }
                            // Drain events during the wait
                            drain_collab_events(editor, collab_event_rx);
                            tokio::time::sleep(poll_interval).await;
                        }
                    }
                    YieldRequest::Breakpoint(_) => {
                        // In test mode, breakpoints can't pause — just resume.
                    }
                    YieldRequest::Flush => {
                        // Apply pending ops (buffer-insert, create-buffer, etc.)
                        // and refresh editor state so subsequent reads see updates.
                        scheme.apply_to_editor(editor);
                        process_side_effects(
                            editor,
                            scheme,
                            collab_event_rx,
                            collab_command_tx,
                            broadcaster,
                        )
                        .await;
                        scheme.inject_editor_state(editor);
                    }
                    YieldRequest::Tick => {
                        // Drain hooks and side effects, then resume.
                        scheme.apply_to_editor(editor);
                        crate::key_handling::drain_hook_evals(editor, scheme);
                        process_side_effects(
                            editor,
                            scheme,
                            collab_event_rx,
                            collab_command_tx,
                            broadcaster,
                        )
                        .await;
                        scheme.inject_editor_state(editor);
                    }
                    YieldRequest::AwaitHook(ref hook_name, timeout) => {
                        // In the test runner, we drain hooks each tick until the
                        // target hook fires or we time out.
                        let deadline = tokio::time::Instant::now()
                            + Duration::from_millis(timeout.as_millis() as u64);
                        let hook_name = hook_name.clone();
                        let mut fired = false;
                        while tokio::time::Instant::now() < deadline {
                            scheme.apply_to_editor(editor);
                            // Check if the hook has pending evals matching our name.
                            let had_hook = editor
                                .pending_hook_evals
                                .iter()
                                .any(|(h, _)| h == &hook_name);
                            crate::key_handling::drain_hook_evals(editor, scheme);
                            process_side_effects(
                                editor,
                                scheme,
                                collab_event_rx,
                                collab_command_tx,
                                broadcaster,
                            )
                            .await;
                            scheme.inject_editor_state(editor);
                            if had_hook {
                                fired = true;
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        // Resume with #t if fired, #f if timeout.
                        eval_result =
                            match scheme.resume_yield(mae_scheme::value::Value::Bool(fired)) {
                                Ok(r) => r,
                                Err(e) => return format!("FAIL:{}", e.message),
                            };
                        continue;
                    }
                }
                // Resume the VM after handling the yield
                eval_result = match scheme.resume_yield(mae_scheme::value::Value::Bool(true)) {
                    Ok(r) => r,
                    Err(e) => return format!("FAIL:{}", e.message),
                };
            }
        }
    }
}

/// Process all pending side effects: drain collab events,
/// write-file, and re-inject editor state.
async fn process_side_effects(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    broadcaster: &SharedBroadcaster,
) {
    // Handle pending write-file operations.
    for (path, content) in scheme.drain_write_files() {
        if let Some(parent) = Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, &content) {
            Ok(()) => debug!(path = path.as_str(), "write-file completed"),
            Err(e) => warn!(path = path.as_str(), error = %e, "write-file failed"),
        }
    }

    // Drain collab intents BEFORE the sleep loop — pending_intent is a single
    // slot that gets overwritten by GapDetected events during collab event
    // processing. Draining first ensures ShareBuffer/JoinDoc intents from the
    // test step are sent to the collab bridge before any event handling.
    crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);

    // Capture pending sync updates for Scheme (buffer-drain-updates) BEFORE
    // drain_and_broadcast consumes them. This preserves updates for test
    // assertions while still forwarding remaining updates to the collab bridge.
    scheme.capture_pending_sync_updates(editor);

    // Forward pending sync updates to state server (mirrors IdleTick in main loop).
    crate::sync_broadcast::drain_and_broadcast(editor, broadcaster, Some(collab_command_tx));

    // Drain any collab events that arrived (non-blocking).
    drain_collab_events(editor, collab_event_rx);

    // Final drain of intents generated by event handling during the sleep.
    crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);

    // Final sync update drain.
    crate::sync_broadcast::drain_and_broadcast(editor, broadcaster, Some(collab_command_tx));
}

/// Convert an abstract `KeyPress` back into a crossterm `KeyEvent` for E2E key
/// injection. Inverse of `key_handling::crossterm_to_keypress`.
fn keypress_to_keyevent(kp: &mae_core::KeyPress) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mae_core::keymap::Key;
    let code = match &kp.key {
        Key::Char(c) => KeyCode::Char(*c),
        Key::Escape => KeyCode::Esc,
        Key::Enter => KeyCode::Enter,
        Key::Backspace => KeyCode::Backspace,
        Key::Tab => KeyCode::Tab,
        Key::BackTab => KeyCode::BackTab,
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Delete => KeyCode::Delete,
        Key::F(n) => KeyCode::F(*n),
    };
    let mut mods = KeyModifiers::NONE;
    if kp.ctrl {
        mods |= KeyModifiers::CONTROL;
    }
    if kp.alt {
        mods |= KeyModifiers::ALT;
    }
    if kp.shift {
        mods |= KeyModifiers::SHIFT;
    }
    KeyEvent::new(code, mods)
}

/// Drain `editor.pending_module_reloads` (mirrors the terminal/GUI event loops)
/// so module-reload + keymap-flavor-switch commands take effect in the test
/// harness — exercising the real `switch_keymap_flavor`/`reload_all_modules`.
fn drain_module_reloads(editor: &mut Editor, scheme: &mut SchemeRuntime) {
    let reloads = std::mem::take(&mut editor.pending_module_reloads);
    if reloads.is_empty() {
        return;
    }
    for r in reloads {
        if r == "__all__" {
            crate::bootstrap::reload_all_modules(scheme, editor);
        } else if let Some(flavor) = r.strip_prefix("__flavor:") {
            crate::bootstrap::switch_keymap_flavor(scheme, editor, flavor);
        } else {
            crate::bootstrap::reload_module(&r, scheme, editor);
        }
    }
    // Drain hooks fired by the reload/switch (e.g. keymap-flavor-changed).
    scheme.apply_to_editor(editor);
    crate::key_handling::drain_hook_evals(editor, scheme);
}

/// Drain `(feed-keys ...)` queues and dispatch each parsed key through the real
/// `handle_key` pipeline — the E2E key-injection harness. Each queued string
/// (e.g. `"C-; b s"`) is parsed into a key sequence that shares one
/// `pending_keys` accumulator, so multi-key bindings resolve exactly as they
/// would from a live keyboard. Hooks fired by the dispatched commands are drained.
fn drain_feed_keys(editor: &mut Editor, scheme: &mut SchemeRuntime) {
    let seqs = scheme.take_pending_feed_keys();
    if seqs.is_empty() {
        return;
    }
    let ai_tx: Option<tokio::sync::mpsc::Sender<mae_ai::AiCommand>> = None;
    for seq_str in seqs {
        let mut pending_keys: Vec<mae_core::KeyPress> = Vec::new();
        let mut pending_interactive: Option<crate::ai_event_handler::PendingInteractiveEvent> =
            None;
        for kp in mae_core::keymap::parse_key_seq_spaced(&seq_str) {
            let key = keypress_to_keyevent(&kp);
            crate::key_handling::handle_key(
                editor,
                scheme,
                key,
                &mut pending_keys,
                &ai_tx,
                &mut pending_interactive,
            );
        }
    }
    // Apply any state the dispatched commands queued, and run hooks they fired.
    scheme.apply_to_editor(editor);
    crate::key_handling::drain_hook_evals(editor, scheme);
}

/// Sleep for the given duration while draining collab events at 100Hz.
async fn drain_events_for(
    editor: &mut Editor,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    broadcaster: &SharedBroadcaster,
    ms: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    let tick_interval = Duration::from_millis(10);
    let mut event_count = 0u64;

    debug!(ms, "drain_events_for: starting sleep loop");

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }

        let remaining = deadline - now;
        let wait = remaining.min(tick_interval);

        tokio::select! {
            Some(event) = collab_event_rx.recv() => {
                event_count += 1;
                debug!(event_count, event = ?event, "drain_events_for: received collab event");
                crate::collab_bridge::handle_collab_event(editor, event);
                // Log active buffer state after event handling.
                let ab = editor.active_buffer();
                debug!(
                    active_buf = %ab.name,
                    text_len = ab.text().len(),
                    text_preview = %ab.text().chars().take(100).collect::<String>(),
                    "drain_events_for: buffer state after event"
                );
            }
            _ = tokio::time::sleep(wait) => {}
        }

        // Drain intents generated by event handling.
        crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
        // Forward pending sync updates to state server (mirrors IdleTick).
        crate::sync_broadcast::drain_and_broadcast(editor, broadcaster, Some(collab_command_tx));
    }

    debug!(ms, event_count, "drain_events_for: sleep loop complete");
}

/// Non-blocking drain of all pending collab events.
fn drain_collab_events(editor: &mut Editor, collab_event_rx: &mut mpsc::Receiver<CollabEvent>) {
    while let Ok(event) = collab_event_rx.try_recv() {
        crate::collab_bridge::handle_collab_event(editor, event);
    }
}

/// Find the mae-test.scm library file.
fn find_test_library() -> Option<std::path::PathBuf> {
    // Search order:
    // 1. scheme/lib/mae-test.scm relative to the binary
    // 2. scheme/lib/mae-test.scm relative to CWD
    // 3. /usr/share/mae/lib/mae-test.scm (installed)

    let cwd_path = std::env::current_dir()
        .ok()?
        .join("scheme/lib/mae-test.scm");
    if cwd_path.exists() {
        return Some(cwd_path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let exe_path = dir.join("../../scheme/lib/mae-test.scm");
            if exe_path.exists() {
                return Some(exe_path);
            }
        }
    }

    let installed = Path::new("/usr/share/mae/lib/mae-test.scm");
    if installed.exists() {
        return Some(installed.to_path_buf());
    }

    None
}

/// Collect .scm test files from a path (file or directory).
fn collect_test_files(path: &str) -> Vec<std::path::PathBuf> {
    let p = Path::new(path);
    if p.is_file() && path.ends_with(".scm") {
        return vec![p.to_path_buf()];
    }
    if p.is_dir() {
        let mut files = Vec::new();
        collect_test_files_recursive(p, &mut files);
        files.sort();
        return files;
    }
    vec![]
}

/// Recursively collect test .scm files from a directory.
fn collect_test_files_recursive(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            collect_test_files_recursive(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "scm")
            && path
                .file_name()
                .is_some_and(|n| n.to_str().is_some_and(|s| s.starts_with("test")))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_files_nonexistent() {
        let files = collect_test_files("/nonexistent/path");
        assert!(files.is_empty());
    }

    #[test]
    fn find_test_library_from_cwd() {
        // When running from the workspace root, the library should be found.
        let lib = find_test_library();
        // This may or may not exist depending on CWD, so just test the function runs.
        let _ = lib;
    }
}
