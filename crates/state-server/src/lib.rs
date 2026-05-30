//! Library interface for mae-state-server integration tests.
//!
//! The primary entry point is the binary (`main.rs`). This lib re-exports
//! modules needed by integration tests.

pub mod auth;
pub mod doc_store;
pub mod handler;
pub mod storage;
