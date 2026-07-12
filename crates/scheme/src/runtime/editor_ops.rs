//! Live editing primitives, editor options, visual buffer operations,
//! buffer editing/list/keymap introspection, and buffer creation/kill.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_bool, arg_float, arg_int, arg_opt_string, arg_string};
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::{SharedState, VisualOp};

/// Register live-editing, editor-option, visual-buffer, buffer-list/keymap
/// introspection, and buffer creation/kill primitives.
pub(super) fn register_editor_ops_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
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
            use mae_core::display_policy::{action_to_string, parse_buffer_kind, DisplayPolicy};
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
}
