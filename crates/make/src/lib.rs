//! mae-make: Build system detection and compiler error parsing.
//!
//! @stability: experimental
//! @since: 0.9.0
//!
//! Detects build systems by walking up from a file path, provides default
//! build/test/run commands per system, and parses compiler output into
//! structured error diagnostics.

pub mod detect;
pub mod errorformat;
pub mod systems;

pub use detect::{detect_build_system, BuildSystem, BuildSystemKind};
pub use errorformat::{parse_build_output, BuildError, ErrorSeverity};
pub use systems::default_commands;
