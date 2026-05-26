//! `(mae async)` library — yield-based async primitives.
//!
//! Provides cooperative multitasking primitives that yield control to the
//! host event loop instead of blocking the thread. When used with `eval()`,
//! they block synchronously (backwards-compatible). When used with
//! `eval_yielding()`, they return `EvalResult::Yield` so the host can
//! drain events, refresh editor state, etc.
//!
//! ## Exports
//!
//! - `(sleep-ms n)` — yield for `n` milliseconds
//! - `(wait-for-file path timeout-ms)` — yield until file exists or timeout
//! - `(current-milliseconds)` — monotonic clock (no yield)
//!
//! @stability: unstable (Phase 13f)
//! @since: 0.12.0

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::library::{Library, LibraryName};
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

/// Exported function names from this library.
const EXPORTS: &[&str] = &["sleep-ms", "wait-for-file", "current-milliseconds"];

/// Register the `(mae async)` library in the VM's library registry.
pub fn register(vm: &mut Vm) {
    // Register the primitive functions as globals first
    register_primitives(vm);

    // Then register as a proper R7RS library
    let mut exports = HashMap::new();
    for name in EXPORTS {
        if let Some(val) = vm.globals.get(name) {
            exports.insert(name.to_string(), val.clone());
        }
    }

    vm.libraries.register(Library {
        name: LibraryName(vec!["mae".to_string(), "async".to_string()]),
        exports,
    });
}

/// Register async primitives as globals (available without import).
fn register_primitives(vm: &mut Vm) {
    // (sleep-ms N) — yield for N milliseconds
    vm.register_fn(
        "sleep-ms",
        "Yield for N milliseconds. In blocking eval, sleeps synchronously. \
         In yielding eval, returns control to the host event loop.",
        Arity::Fixed(1),
        |args| {
            let ms = args[0]
                .as_int()
                .map_err(|_| LispError::type_error("integer", args[0].type_name()))?;
            Err(LispError::yield_sleep(std::time::Duration::from_millis(
                ms.max(0) as u64,
            )))
        },
    );

    // (wait-for-file PATH TIMEOUT-MS) — yield until file exists
    vm.register_fn(
        "wait-for-file",
        "Yield until file at PATH exists, or TIMEOUT-MS elapses. \
         Returns #t if file appeared, raises error on timeout in blocking mode.",
        Arity::Fixed(2),
        |args| {
            let path = args[0]
                .as_str()
                .map_err(|_| LispError::type_error("string", args[0].type_name()))?;
            let timeout_ms = args[1]
                .as_int()
                .map_err(|_| LispError::type_error("integer", args[1].type_name()))?;
            Err(LispError::yield_wait_for_file(
                std::path::PathBuf::from(path),
                std::time::Duration::from_millis(timeout_ms.max(0) as u64),
            ))
        },
    );

    // (current-milliseconds) — monotonic clock, no yield
    vm.register_fn(
        "current-milliseconds",
        "Return the current time in milliseconds since the Unix epoch.",
        Arity::Fixed(0),
        |_args| {
            let ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            Ok(Value::Int(ms))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib;
    use crate::vm::{EvalResult, YieldRequest};

    fn make_vm() -> Vm {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        register(&mut vm);
        vm
    }

    #[test]
    fn sleep_ms_yields() {
        let mut vm = make_vm();
        let r = vm.eval_yielding("(sleep-ms 50)").unwrap();
        match r {
            EvalResult::Yield(YieldRequest::Sleep(d)) => {
                assert_eq!(d.as_millis(), 50);
            }
            _ => panic!("expected Sleep yield"),
        }
    }

    #[test]
    fn sleep_ms_blocking_works() {
        let mut vm = make_vm();
        let start = std::time::Instant::now();
        let result = vm.eval("(sleep-ms 5)").unwrap();
        assert!(start.elapsed().as_millis() >= 5);
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn sleep_ms_negative_clamps_to_zero() {
        let mut vm = make_vm();
        let r = vm.eval_yielding("(sleep-ms -10)").unwrap();
        match r {
            EvalResult::Yield(YieldRequest::Sleep(d)) => {
                assert_eq!(d.as_millis(), 0);
            }
            _ => panic!("expected Sleep yield"),
        }
    }

    #[test]
    fn sleep_ms_type_error_on_non_integer() {
        let mut vm = make_vm();
        let err = vm.eval("(sleep-ms \"nope\")").unwrap_err();
        assert!(err.message().contains("type error"));
    }

    #[test]
    fn sleep_ms_arity_error() {
        let mut vm = make_vm();
        let err = vm.eval("(sleep-ms)").unwrap_err();
        assert!(err.message().contains("expected 1"));
    }

    #[test]
    fn wait_for_file_yields() {
        let mut vm = make_vm();
        let r = vm
            .eval_yielding(r#"(wait-for-file "/tmp/test-wait" 3000)"#)
            .unwrap();
        match r {
            EvalResult::Yield(YieldRequest::WaitForFile(p, t)) => {
                assert_eq!(p.to_str().unwrap(), "/tmp/test-wait");
                assert_eq!(t.as_millis(), 3000);
            }
            _ => panic!("expected WaitForFile yield"),
        }
    }

    #[test]
    fn wait_for_file_type_errors() {
        let mut vm = make_vm();
        let err = vm.eval(r#"(wait-for-file 42 1000)"#).unwrap_err();
        assert!(err.message().contains("type error"));
    }

    #[test]
    fn wait_for_file_arity_error() {
        let mut vm = make_vm();
        let err = vm.eval(r#"(wait-for-file "/tmp/x")"#).unwrap_err();
        assert!(err.message().contains("expected 2"));
    }

    #[test]
    fn current_milliseconds_returns_positive() {
        let mut vm = make_vm();
        let result = vm.eval("(current-milliseconds)").unwrap();
        match result {
            Value::Int(ms) => assert!(ms > 1_000_000_000_000), // post-2001
            _ => panic!("expected integer"),
        }
    }

    #[test]
    fn current_milliseconds_no_yield() {
        let mut vm = make_vm();
        let r = vm.eval_yielding("(current-milliseconds)").unwrap();
        assert!(matches!(r, EvalResult::Done(Value::Int(_))));
    }

    #[test]
    fn library_importable() {
        let mut vm = make_vm();
        // The library should be importable via (import (mae async))
        let result = vm.eval("(import (mae async)) (current-milliseconds)");
        assert!(result.is_ok());
    }

    #[test]
    fn sleep_then_compute() {
        let mut vm = make_vm();
        // In blocking mode: sleep then return a value
        let result = vm.eval("(sleep-ms 1) (+ 1 2)").unwrap();
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn yield_resume_loop_with_sleep() {
        let mut vm = make_vm();
        vm.eval(
            "(define (count-sleeps n)
               (if (<= n 0)
                   0
                   (begin (sleep-ms 1)
                          (+ 1 (count-sleeps (- n 1))))))",
        )
        .unwrap();

        let mut r = vm.eval_yielding("(count-sleeps 3)").unwrap();
        let mut yields = 0;
        loop {
            match r {
                EvalResult::Done(v) => {
                    assert_eq!(v, Value::Int(3));
                    break;
                }
                EvalResult::Yield(YieldRequest::Sleep(_)) => {
                    yields += 1;
                    r = vm.resume(Value::Bool(true)).unwrap();
                }
                _ => panic!("unexpected yield type"),
            }
        }
        assert_eq!(yields, 3);
    }
}
