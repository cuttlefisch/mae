//! R7RS-small standard library for mae-scheme.
//!
//! Registers all R7RS base primitives as foreign functions in the VM.
//!
//! @stability: unstable (Phase 13c)
//! @since: 0.12.0

mod base;
mod char;
mod io;
mod string;
mod vector;

use crate::vm::Vm;

/// Register all R7RS standard library primitives.
pub fn register_stdlib(vm: &mut Vm) {
    base::register(vm);
    char::register(vm);
    string::register(vm);
    vector::register(vm);
    io::register(vm);
}
