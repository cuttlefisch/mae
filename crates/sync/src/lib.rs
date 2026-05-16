//! mae-sync: Collaborative state synchronization via yrs (YATA CRDT).
//!
//! Wraps yrs with MAE-specific document schemas and provides a bridge
//! between yrs YText and ropey Rope for rendering.

pub mod encoding;
pub mod kb;
pub mod text;

pub use yrs;

use std::fmt;

/// Errors from sync operations.
#[derive(Debug)]
pub enum SyncError {
    Encoding(String),
    RopeRebuild(String),
    Schema(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encoding(msg) => write!(f, "yrs encoding error: {msg}"),
            Self::RopeRebuild(msg) => write!(f, "rope rebuild failed: {msg}"),
            Self::Schema(msg) => write!(f, "schema violation: {msg}"),
        }
    }
}

impl std::error::Error for SyncError {}
