//! Advisory file locking for multi-editor file contention.
//!
//! Re-exports the implementation from `mae_mcp::file_lock` (via `mae-kb`,
//! which `mae-core` already depends on) — the primitive lives there so
//! lower-level shared crates like `mae-kb` (used standalone by the `daemon`
//! workspace too) can reuse it without depending on `mae-core`. See
//! `shared/mcp/src/file_lock.rs` for the implementation and
//! `shared/kb/src/lib.rs`'s `pub use mae_mcp::file_lock;` for the relay.

pub use mae_kb::file_lock::*;

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    #[test]
    fn content_hash_on_buffer() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("hash_test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let buf = crate::buffer::Buffer::from_file(&file).unwrap();
        assert!(buf.content_hash.is_some());
        assert!(!buf.content_hash.as_ref().unwrap().is_empty());
    }
}
