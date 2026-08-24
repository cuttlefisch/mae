//! Operator subcommands for ADR-032 checkpoints: `mae-daemon checkpoint` and
//! `mae-daemon restore`.
//!
//! Split out of `main.rs` rather than blessed past the size ceiling — these are
//! self-contained operator commands with no coupling to the serve path, which is
//! exactly the seam the ceiling exists to encourage.
//!
//! Wired because `checkpoint.rs` previously had **zero production callers**
//! (#632): the CRDT-truth rollback artifact had never been produced or restored
//! outside a unit test.

use std::sync::Arc;

use mae_daemon::{checkpoint, doc_store, storage};

use crate::config::DaemonConfig;

/// Open the collab doc store for an OFFLINE operator command.
///
/// Deliberately separate from `init_doc_store`: that one is the serve path and
/// also wires broadcasters, blocklists and recovery. A checkpoint/restore run
/// wants the store and nothing else, and must not look like a second daemon.
///
/// @ai-caution: [daemon-state] SQLite is single-writer. Run these subcommands
/// with the daemon STOPPED — `restore` in particular replaces documents, and a
/// running daemon holds its own view of them in memory.
async fn open_doc_store_offline(config: &DaemonConfig) -> Result<Arc<doc_store::DocStore>, String> {
    let db_path = config.resolve_collab_data_dir().join("state.db");
    let backend =
        storage::SqliteBackend::open_with_pool_size(&db_path, config.collab.storage.shard_count)
            .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    Ok(Arc::new(doc_store::DocStore::new(
        Arc::new(backend),
        config.collab.storage.compact_threshold,
    )))
}

/// `mae-daemon checkpoint <kb_id> <out-file>` — write an ADR-032 checkpoint.
pub(crate) async fn run_checkpoint(config: &DaemonConfig, rest: &[String]) -> i32 {
    let (Some(kb_id), Some(out)) = (rest.first(), rest.get(1)) else {
        eprintln!("Usage: mae-daemon checkpoint <kb_id> <out-file>");
        return 2;
    };
    let doc_store = match open_doc_store_offline(config).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };
    match checkpoint::export_kb(&doc_store, kb_id).await {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(out, &bytes) {
                eprintln!("Error: write {out}: {e}");
                return 1;
            }
            // Report what was captured, not just that a file appeared — a
            // zero-node checkpoint written successfully is the failure this
            // command exists to make visible.
            match checkpoint::KbCheckpoint::from_bytes(&bytes) {
                Ok(cp) => println!(
                    "checkpoint: kb={} nodes={} bytes={} hash={} -> {out}",
                    cp.kb_id,
                    cp.node_count(),
                    bytes.len(),
                    cp.content_hash
                ),
                Err(e) => {
                    eprintln!("Error: artifact written but does not verify: {e}");
                    return 1;
                }
            }
            0
        }
        Err(e) => {
            eprintln!("Error: checkpoint '{kb_id}': {e}");
            1
        }
    }
}

/// `mae-daemon restore <artifact>` — restore a KB from a checkpoint.
///
/// Restore REPLACES the KB the artifact names. Refuses to run against a daemon
/// that is already serving, because SQLite is single-writer and a live daemon
/// holds its own in-memory view of the documents this rewrites.
pub(crate) async fn run_restore(config: &DaemonConfig, rest: &[String]) -> i32 {
    let Some(path) = rest.first() else {
        eprintln!("Usage: mae-daemon restore <artifact-file>");
        return 2;
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error: read {path}: {e}");
            return 1;
        }
    };
    let doc_store = match open_doc_store_offline(config).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e} (is the daemon running? stop it first)");
            return 1;
        }
    };
    match checkpoint::import_kb(&doc_store, &bytes).await {
        Ok(cp) => {
            println!(
                "restored: kb={} nodes={} hash={}",
                cp.kb_id,
                cp.node_count(),
                cp.content_hash
            );
            0
        }
        Err(e) => {
            eprintln!("Error: restore from {path}: {e}");
            1
        }
    }
}
