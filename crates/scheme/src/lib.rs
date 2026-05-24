//! mae-scheme: Embedded Scheme runtime for configuration and packages.
//!
//! The crate contains two parallel implementations:
//! - `runtime` — current Steel-based runtime (stable, in production)
//! - `value`, `reader`, `lisp_error` — new mae-scheme R7RS runtime (Phase 13, unstable)
//!
//! @stability: stable (runtime), unstable (new modules)
//! @since: 0.2.0

pub mod runtime;

// Phase 13: mae-scheme R7RS-small runtime (building alongside Steel)
pub mod compiler;
pub mod env;
pub mod lisp_error;
pub mod reader;
pub mod stdlib;
pub mod value;
pub mod vm;

pub use runtime::{DeclaredPackage, SchemeError, SchemeErrorSnapshot, SchemeRuntime};
