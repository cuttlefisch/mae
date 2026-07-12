//! Keybinding registration and hook system primitives.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::arg_string;
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register keybinding-definition and hook-system primitives.
pub(super) fn register_keybinding_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
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
}
