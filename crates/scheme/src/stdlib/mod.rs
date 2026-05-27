//! mae-scheme standard library.
//!
//! Registers all R7RS base primitives as VM globals, then creates
//! proper R7RS library facades in the LibraryRegistry.
//!
//! ## Architecture
//!
//! 1. `register_stdlib()` — populate globals (interaction environment)
//! 2. `register_r7rs_libraries()` — create R7RS library facades
//! 3. `register_mae_libs()` — create mae-specific libraries
//!
//! This follows the Chibi-Scheme pattern: primitives are available
//! in the interaction environment (globals), and libraries are
//! curated export facades for use with `(import ...)`.
//!
//! @stability: unstable (Phase 13i)
//! @since: 0.12.0

mod base;
mod char;
mod io;
pub mod libraries;
pub mod mae_async;
mod string;
mod vector;

use crate::vm::Vm;

/// Register all R7RS standard library primitives into the interaction environment.
pub fn register_stdlib(vm: &mut Vm) {
    base::register(vm);
    base::register_inexact(vm);
    char::register(vm);
    string::register(vm);
    vector::register(vm);
    io::register(vm);
}

/// Register R7RS standard library facades in the library registry.
///
/// Must be called AFTER `register_stdlib()`.
pub fn register_r7rs_libraries(vm: &mut Vm) {
    libraries::register_r7rs_libraries(vm);
}

/// Register mae-specific libraries (beyond R7RS).
pub fn register_mae_libs(vm: &mut Vm) {
    mae_async::register(vm);
}
