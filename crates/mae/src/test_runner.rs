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

use crate::collab_bridge::{CollabCommand, CollabEvent};

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
            process_side_effects(editor, scheme, collab_event_rx, collab_command_tx).await;
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

    // Load and evaluate each test file.
    for file in &test_files {
        info!(file = %file.display(), "loading test file");
        scheme.inject_editor_state(editor);

        // Override buffer-string and buffer-text with mutable-cell versions.
        // inject_editor_state creates closure-captured snapshots via register_fn.
        // We replace them with Scheme functions that read from mutable variables.
        // This way, test thunks defined in the test file capture the forwarding
        // function, and sync_scheme_state can update *buffer-text* via set!.
        install_mutable_buffer_accessors(editor, scheme);

        if let Err(e) = scheme.load_file(file) {
            eprintln!("mae-test: error loading {}: {}", file.display(), e.message);
            return 2;
        }

        // Process side effects after loading (runs describe/it registrations).
        scheme.apply_to_editor(editor);
        process_side_effects(editor, scheme, collab_event_rx, collab_command_tx).await;

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
    run_tests_iteratively(editor, scheme, collab_event_rx, collab_command_tx).await
}

/// Run all registered tests one-by-one from the Rust side.
///
/// Between each test, we call inject_editor_state + apply_to_editor + process_side_effects
/// so that buffer mutations from one test are visible in subsequent tests.
async fn run_tests_iteratively(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
) -> i32 {
    // Query test count. Do NOT call inject_editor_state here — it would create
    // new bindings that shadow the ones test thunks captured at file-load time.
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

    let mut pass_count = 0usize;
    let mut fail_count = 0usize;

    for i in 0..count {
        // Get test name.
        let name = scheme
            .eval(&format!("(test-name {})", i))
            .unwrap_or_else(|_| format!("test-{}", i));
        let name = name.trim().trim_matches('"').to_string();

        // Run the test — do NOT call inject_editor_state here, as it creates
        // new bindings that shadow the ones test thunks captured. Instead,
        // sync_scheme_state (below) uses set! to mutate existing binding cells.
        let result = match scheme.eval(&format!("(run-nth-test {})", i)) {
            Ok(s) => s,
            Err(e) => format!("FAIL:{}", e.message),
        };
        let result = result.trim().trim_matches('"').to_string();

        // Apply side effects (buffer mutations, commands, sleeps, writes).
        scheme.apply_to_editor(editor);
        process_side_effects(editor, scheme, collab_event_rx, collab_command_tx).await;

        // Sync Scheme state variables via set! — register_value creates new bindings
        // that aren't visible to closures captured in previous evals. set! mutates
        // the existing binding cell that closures already reference.
        sync_scheme_state(editor, scheme);

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
            println!("  ...");
        }
    }

    // Summary.
    println!();
    println!("# {} passed, {} failed", pass_count, fail_count);

    if fail_count > 0 {
        1
    } else {
        0
    }
}

/// Install mutable buffer accessor functions in the Scheme environment.
///
/// After inject_editor_state (which uses register_fn to create closure-captured
/// snapshots), we override buffer-string and buffer-text with Scheme-defined
/// functions that read from mutable variables. This way:
/// 1. Test file closures capture these Scheme functions (not Rust closures)
/// 2. sync_scheme_state can update *buffer-text* etc. via set!
/// 3. Test thunks see fresh buffer contents between test steps
fn install_mutable_buffer_accessors(editor: &Editor, scheme: &mut SchemeRuntime) {
    // Build all-buffer-texts as a Scheme list of (name text) pairs.
    let mut all_bufs = String::from("(list");
    for b in &editor.buffers {
        let bname = b.name.replace('\\', "\\\\").replace('"', "\\\"");
        let btext = b.text().replace('\\', "\\\\").replace('"', "\\\"");
        all_bufs.push_str(&format!(" (list \"{}\" \"{}\")", bname, btext));
    }
    all_bufs.push(')');

    let code = format!(
        r#"(begin
          (define *all-buffer-texts* {all_bufs})
          (define (buffer-string) *buffer-text*)
          (define (buffer-text name)
            (let loop ((entries *all-buffer-texts*))
              (if (null? entries) #f
                  (if (string-contains? (car (car entries)) name)
                      (car (cdr (car entries)))
                      (loop (cdr entries)))))))"#,
        all_bufs = all_bufs,
    );
    let _ = scheme.eval(&code);
}

/// Sync Scheme state variables using `set!` instead of `register_value`.
///
/// Steel's `register_value` creates a new binding cell, but closures captured
/// in earlier evals reference the old cell. `set!` mutates in-place, so the
/// test thunks see updated values.
fn sync_scheme_state(editor: &Editor, scheme: &mut SchemeRuntime) {
    let buf = editor.active_buffer();
    let text = buf.text().replace('\\', "\\\\").replace('"', "\\\"");
    let name = buf.name.replace('\\', "\\\\").replace('"', "\\\"");
    let buf_count = editor.buffers.len();
    let win = editor.window_mgr.focused_window();

    // Build a single set! expression to update all state variables.
    let sync_code = format!(
        r#"(begin
          (set! *buffer-text* "{text}")
          (set! *buffer-name* "{name}")
          (set! *buffer-count* {buf_count})
          (set! *buffer-modified?* {modified})
          (set! *buffer-line-count* {lines})
          (set! *cursor-row* {crow})
          (set! *cursor-col* {ccol}))"#,
        text = text,
        name = name,
        buf_count = buf_count,
        modified = if buf.modified { "#t" } else { "#f" },
        lines = buf.line_count(),
        crow = win.cursor_row,
        ccol = win.cursor_col,
    );

    if let Err(e) = scheme.eval(&sync_code) {
        warn!(error = %e.message, "failed to sync scheme state variables");
    }

    // Also update all buffer text snapshots in the all-buffers list.
    // This is used by (buffer-text NAME) which searches by name.
    let mut all_bufs = String::from("(list");
    for b in &editor.buffers {
        let bname = b.name.replace('\\', "\\\\").replace('"', "\\\"");
        let btext = b.text().replace('\\', "\\\\").replace('"', "\\\"");
        all_bufs.push_str(&format!(" (list \"{}\" \"{}\")", bname, btext));
    }
    all_bufs.push(')');
    let sync2 = format!(
        r#"(begin
          (set! *all-buffer-texts* {all_bufs}))"#,
        all_bufs = all_bufs,
    );
    let _ = scheme.eval(&sync2);
}

/// Process all pending side effects: drain collab events, handle sleep-ms,
/// write-file, and re-inject editor state.
async fn process_side_effects(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
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

    // Handle pending sleep-ms: sleep while draining collab events.
    if let Some(ms) = scheme.take_sleep_ms() {
        drain_events_for(editor, collab_event_rx, collab_command_tx, ms).await;
    }

    // Drain any collab events that arrived.
    drain_collab_events(editor, collab_event_rx);

    // Drain collab intents from editor to background task.
    crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
}

/// Sleep for the given duration while draining collab events at 100Hz.
async fn drain_events_for(
    editor: &mut Editor,
    collab_event_rx: &mut mpsc::Receiver<CollabEvent>,
    collab_command_tx: &mpsc::Sender<CollabCommand>,
    ms: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    let tick_interval = Duration::from_millis(10);

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }

        let remaining = deadline - now;
        let wait = remaining.min(tick_interval);

        tokio::select! {
            Some(event) = collab_event_rx.recv() => {
                crate::collab_bridge::handle_collab_event(editor, event);
            }
            _ = tokio::time::sleep(wait) => {}
        }

        // Drain intents generated by event handling.
        crate::collab_bridge::drain_collab_intents(editor, collab_command_tx);
    }
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
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "scm"))
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_str().is_some_and(|s| s.starts_with("test")))
            })
            .collect();
        files.sort();
        return files;
    }
    vec![]
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
