//! mae-scheme standard library.
//!
//! Registers all R7RS base primitives and mae-specific libraries
//! as foreign functions in the VM.
//!
//! @stability: unstable (Phase 13c)
//! @since: 0.12.0

mod base;
mod char;
mod io;
pub mod mae_async;
mod string;
mod vector;

use crate::vm::Vm;

/// Register all R7RS standard library primitives.
pub fn register_stdlib(vm: &mut Vm) {
    base::register(vm);
    base::register_inexact(vm);
    char::register(vm);
    string::register(vm);
    vector::register(vm);
    io::register(vm);
}

/// Register mae-specific libraries (beyond R7RS).
pub fn register_mae_libs(vm: &mut Vm) {
    mae_async::register(vm);
}
