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

/// Growth-ratio timing helper for the scaling tests (issue #947).
#[cfg(test)]
pub(crate) mod timing {
    use std::time::Instant;

    /// Best-of-`rounds` time of `small` and of `large`, interleaved so a load
    /// burst slows both sides rather than one. Returns (ratio, small_s per run,
    /// large_s). Timed in equal windows (#947): the small case runs `small_reps` times (the size
    /// ratio) inside one timed window, so both windows last about as long and are
    /// equally exposed to preemption. Timing one short small run against one long
    /// large run let a busy runner inflate only the large side: 93x for 16x on
    /// macOS CI (91 us -> 8.5 ms) where the code grows ~16x.
    pub fn growth_ratio(
        rounds: usize,
        small_reps: usize,
        mut small: impl FnMut(),
        mut large: impl FnMut(),
    ) -> (f64, f64, f64) {
        let reps = small_reps.max(1);
        let (mut bs, mut bl) = (f64::MAX, f64::MAX);
        for _ in 0..rounds {
            let t = Instant::now();
            for _ in 0..reps {
                small();
            }
            bs = bs.min(t.elapsed().as_secs_f64() / reps as f64);
            let t = Instant::now();
            large();
            bl = bl.min(t.elapsed().as_secs_f64());
        }
        (bl / bs.max(1e-9), bs, bl)
    }
}
