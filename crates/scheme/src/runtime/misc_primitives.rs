//! String utilities, process execution, and the advice system.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::sync::Arc;

use parking_lot::Mutex;

use tracing::warn;

use crate::ffi::{arg_string, list_to_strings};
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register string-utility, process-execution, and advice-system primitives.
pub(super) fn register_misc_primitive_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
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
}
