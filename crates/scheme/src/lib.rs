//! mae-scheme: Embedded Scheme runtime for configuration and packages.
//!
//! R7RS-small runtime with bytecode compiler and VM. All editor
//! primitives are registered as foreign functions in the VM.
//!
//! @stability: stable
//! @since: 0.12.0

// `lisp_error::LispError` sits right at clippy's 128-byte `result_large_err` threshold
// (ErrorKind's largest variant + SourceLocation + a Vec + a boxed Option adds up to
// ~128 bytes on its own) -- borderline enough that Rust's per-target layout algorithm
// apparently packs it just over the line on Windows while staying under it on Linux
// (confirmed empirically: `windows-latest` CI's `cargo clippy` fails here across ~600
// closures/functions throughout this crate that return `Result<_, LispError>`; the
// identical Linux `stable / clippy` job passes clean for the same commit). Properly
// fixing the root cause -- boxing `ErrorKind`'s largest variants -- would touch every
// construction and match site across this crate and is real, separately-scoped future
// work (tracked in issue #455), not something to attempt blind under Windows-CI
// iteration pressure with no local Windows toolchain to verify hundreds of call sites
// against. This is a lint about Ok-path stack bloat, not a correctness issue.
#![allow(clippy::result_large_err)]

pub mod runtime;

pub mod compiler;
pub mod env;
pub mod ffi;
pub mod introspect;
pub mod library;
pub mod lisp_error;
pub mod lsp;
pub mod macros;
pub mod permission;
pub mod reader;
pub mod stdlib;
pub mod value;
pub mod vm;

pub use runtime::{
    DeclaredPackage, SchemeError, SchemeErrorSnapshot, SchemeEvalResult, SchemeRuntime,
};
