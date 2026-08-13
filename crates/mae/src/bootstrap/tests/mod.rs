//! Tests for [`super`] — startup bootstrap.
//!
//! Split out of `bootstrap.rs` (and then by subject) so both halves stay under
//! the size ceilings. Kept as CHILD modules so `use super::super::*` still
//! reaches bootstrap's private helpers with no visibility widening — the same
//! shape as `shared/kb/src/migrate/tests.rs`.

/// `SchemeRuntime::new()` with the panic message the module tests want.
///
/// Lives in the parent module, above the `mod` declarations, because
/// `macro_rules!` is textually scoped: a child module only sees macros defined
/// before its own declaration.
macro_rules! require_scheme {
    () => {
        SchemeRuntime::new().expect("SchemeRuntime::new() should not fail")
    };
}

mod eviction;
mod kb_federation;
mod memory;
mod modules;
mod startup;
