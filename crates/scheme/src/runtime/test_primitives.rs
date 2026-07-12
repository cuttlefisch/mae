//! Test framework primitives, test introspection via `SharedState`, E2E
//! key-injection (test harness), and CRDT/sync test primitives.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_int, arg_string};
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register test-framework, test-introspection, E2E key-injection, and
/// CRDT/sync test primitives.
pub(super) fn register_test_primitive_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
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
}
