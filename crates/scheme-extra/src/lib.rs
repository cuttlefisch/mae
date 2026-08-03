//! Extension point for out-of-tree Scheme kernel primitives (#521).
//!
//! `crates/mae/src/main.rs` calls [`register_all`] right after constructing
//! a `SchemeRuntime`, gated behind the `extra-kernel-crates` Cargo feature —
//! see that file. This crate ships as a no-op; a downstream fork wanting to
//! compile in additional primitives:
//!
//! 1. Writes a crate exposing `pub fn register_fns(vm: &mut Vm, shared:
//!    &Arc<Mutex<SharedState>>)` — the exact shape every in-tree
//!    `crates/scheme/src/runtime/*.rs` module already uses (see
//!    `EXTENSION_GUIDE.md`'s "Adding a Kernel Primitive" section for that
//!    pattern).
//! 2. Adds it as a `[dependencies]` entry in THIS crate's `Cargo.toml`.
//! 3. Calls it from `register_all` below.
//! 4. Builds mae with `--features mae/extra-kernel-crates`.
//!
//! No file inside `crates/scheme` itself is ever touched for steps 2-4 —
//! only this crate. `SharedState`'s fields stay private; primitives here
//! use the same narrow accessor methods (e.g. `SharedState::kb_store()`)
//! any other out-of-tree consumer would.
//!
//! `crates/scheme` deliberately does not call into this crate directly --
//! that would be a circular crate dependency (this crate already depends
//! on `mae-scheme` for the `Vm`/`SharedState` types). `crates/mae` is what
//! wires the two together, via `SchemeRuntime::shared_state()` +
//! `SchemeRuntime::vm_mut()`, right after construction.
//!
//! This is single-slot compile-time indirection, not true N-crate
//! config-driven registration -- Cargo cannot resolve a dependency list
//! computed at build time. `crates/scheme`/`crates/mae` never change again
//! once this mechanism exists; the actual list of extra crates lives one
//! hop away, here.
//!
//! @stability: unstable (#521)

use std::sync::Arc;

use parking_lot::Mutex;

use mae_scheme::permission::tier;
use mae_scheme::runtime::SharedState;
use mae_scheme::vm::Vm;

/// Called once from `crates/mae/src/main.rs` right after `SchemeRuntime::
/// new()`, when `extra-kernel-crates` is enabled. No-op by default --
/// downstream forks add calls here.
pub fn register_all(_vm: &mut Vm, _shared: &Arc<Mutex<SharedState>>) {
    // register_html_export::register_fns(_vm, _shared);  // example
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_scheme::lisp_error::Arity;
    use mae_scheme::value::Value;

    /// Compile-time-only assertion that `register_all` genuinely has the
    /// documented shape, and that `SharedState` is reachable as
    /// `mae_scheme::runtime::SharedState` from outside `crates/scheme` --
    /// if this doesn't compile, the extension point's core claim is false.
    #[test]
    fn register_all_matches_the_documented_registrar_shape() {
        let _: fn(&mut Vm, &Arc<Mutex<SharedState>>) = register_all;
    }

    /// End-to-end validation of the actual mechanism a real downstream
    /// crate would rely on: `Vm::register_fn` called from OUTSIDE
    /// `crates/scheme` genuinely registers a primitive callable via
    /// `Vm::eval`, and a closure defined here can read `SharedState`
    /// through its own narrow accessor -- not just that the types compile.
    #[test]
    fn a_primitive_registered_from_this_crate_is_callable_via_eval() {
        let mut vm = Vm::new();
        let shared: Arc<Mutex<SharedState>> = Arc::new(Mutex::new(SharedState::default()));
        let shared_for_closure = shared.clone();
        vm.register_fn(
            "extra-kernel-crate-test-primitive",
            "Validation-only primitive proving the extension point works end to end",
            Arity::Fixed(0),
            tier::PURE,
            move |_args: &[Value]| {
                let has_store = shared_for_closure.lock().kb_store().is_some();
                Ok(Value::Bool(has_store))
            },
        );
        let result = vm.eval("(extra-kernel-crate-test-primitive)").unwrap();
        assert_eq!(
            result,
            Value::Bool(false),
            "no KB store was injected into this SharedState, so kb_store() should read None"
        );
    }
}
