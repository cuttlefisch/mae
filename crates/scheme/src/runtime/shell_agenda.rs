//! Shell terminal bindings and agenda file management primitives.
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

/// Register shell-terminal and agenda-file-management primitives.
pub(super) fn register_shell_agenda_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
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
}
