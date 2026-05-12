//! mae-lookup: Definition lookup fallbacks and documentation URL builders.
//!
//! Provides grep-based "dumb jump" for definition finding when no LSP is
//! available, plus URL builders for online documentation (docs.rs, MDN,
//! devdocs.io, cppreference).

pub mod dumb_jump;
pub mod online;

pub use dumb_jump::{dumb_jump, DumbJumpResult};
pub use online::docs_url;
