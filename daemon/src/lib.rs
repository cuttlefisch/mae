//! Library interface for mae-daemon integration tests.
//!
//! The primary entry point is the binary (`main.rs`). This lib re-exports
//! modules needed by integration tests.

pub mod checkpoint;
pub mod collab_handler;
pub mod doc_store;
pub mod projector;
pub mod storage;

/// Short git SHA of this build (`-dirty` if the tree had uncommitted changes,
/// "unknown" if built outside a git checkout). Set by `build.rs`. Reported in
/// the startup log, `--version`, and the `$/debug` response so an editor's
/// `collab-doctor` can detect an editor↔daemon build mismatch across machines.
pub const BUILD_SHA: &str = match option_env!("MAE_BUILD_SHA") {
    Some(s) => s,
    None => "unknown",
};
