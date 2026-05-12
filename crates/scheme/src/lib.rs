//! mae-scheme: Embedded Scheme runtime for configuration and packages.
//!
//! @stability: stable
//! @since: 0.2.0

pub mod runtime;

pub use runtime::{DeclaredPackage, SchemeError, SchemeErrorSnapshot, SchemeRuntime};
