//! mae-scheme: Embedded Scheme runtime for configuration and packages.
//!
//! R7RS-small runtime with bytecode compiler and VM. All editor
//! primitives are registered as foreign functions in the VM.
//!
//! @stability: stable
//! @since: 0.12.0

pub mod runtime;

pub mod compiler;
pub mod env;
pub mod ffi;
pub mod library;
pub mod lisp_error;
pub mod macros;
pub mod reader;
pub mod stdlib;
pub mod value;
pub mod vm;

pub use runtime::{DeclaredPackage, SchemeError, SchemeErrorSnapshot, SchemeRuntime};
