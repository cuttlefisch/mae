//! Put a Cozo sqlite store into WAL mode, out of band.
//!
//! @ai-caution: [storage] cozo 0.7.6 never configures `journal_mode` or
//! `busy_timeout` — verified by direct source read of `storage/sqlite.rs` — so a
//! store it creates runs in **rollback-journal** mode, where a writer's
//! exclusive lock blocks *readers* file-wide. That is what the 45-second
//! busy-retry loop in `db.rs` exists to paper over, and why a two-writer
//! experiment measured ~14% raw write failures.
//!
//! `db.rs` previously concluded that "there is no hook this crate could use to
//! set the pragma even if it wanted to". The premise is right — cozo never
//! exposes its `Connection` — but the conclusion does not follow, because **WAL
//! is a property of the database file's header, not of the connection**. From
//! <https://www.sqlite.org/wal.html>:
//!
//! > Unlike the other journaling modes, `PRAGMA journal_mode=WAL` is
//! > persistent… applications can be converted to using SQLite in WAL mode
//! > **without making any changes to the application itself**.
//!
//! So MAE opens the file once with the `sqlite` crate (the same one cozo uses),
//! sets the pragma, closes, and hands the file to cozo. Demonstrated rather than
//! assumed — see `wal_tests`.
//!
//! **Not universally applicable, and the check is the pragma itself.** WAL
//! requires every process on one host and a writable `-shm` file, and does not
//! work on a network filesystem. Rather than sniffing the filesystem type — which
//! is unportable and would need its own per-OS table — this asks SQLite: the
//! pragma *returns the mode actually in force*, so a filesystem that cannot
//! support WAL simply reports back the old mode and MAE leaves it alone.

use std::path::Path;

/// Best-effort: put `db_path` into WAL mode, returning the mode now in force.
///
/// Never fails the caller. A store that stays in rollback-journal mode is the
/// status quo, which works — it is merely slower under cross-process
/// contention. Refusing to open a KB because a pragma did not take would trade a
/// performance property for an availability one.
pub(super) fn ensure_wal(db_path: &Path) -> Option<String> {
    let conn = sqlite::Connection::open(db_path).ok()?;

    // Already WAL? Then this is a reopen and there is nothing to do. Checked
    // first so the common path does not write to the header on every open.
    if read_mode(&conn).as_deref() == Some("wal") {
        return Some("wal".to_string());
    }

    // The pragma RETURNS the resulting mode. On a filesystem that cannot support
    // WAL, or while another connection holds the file, it reports the unchanged
    // mode instead of erroring -- which is exactly the portable capability check.
    let mut applied = None;
    let _ = conn.iterate("PRAGMA journal_mode=WAL;", |row| {
        applied = row
            .first()
            .and_then(|(_, v)| *v)
            .map(|v| v.to_ascii_lowercase());
        true
    });

    match applied.as_deref() {
        Some("wal") => tracing::debug!(path = %db_path.display(), "KB store: WAL enabled"),
        other => tracing::debug!(
            path = %db_path.display(),
            mode = other.unwrap_or("<unknown>"),
            "KB store: WAL not available; leaving journal mode as-is \
             (network filesystem, or another process holds the file)"
        ),
    }
    applied
}

fn read_mode(conn: &sqlite::Connection) -> Option<String> {
    let mut mode = None;
    let _ = conn.iterate("PRAGMA journal_mode;", |row| {
        mode = row
            .first()
            .and_then(|(_, v)| *v)
            .map(|v| v.to_ascii_lowercase());
        true
    });
    mode
}

/// Mutating scripts between explicit checkpoints. SQLite's own automatic
/// checkpoint runs every 1,000 WAL frames; a mutating script writes a handful
/// to a few hundred, so this keeps a store's WAL in the same order of size.
pub(super) const CHECKPOINT_EVERY_WRITES: u32 = 64;

/// Checkpoints a store's WAL from MAE's own connection, because cozo's never
/// do.
///
/// @ai-caution: [storage] SQLite's automatic checkpoint runs on the
/// connection that just committed, and gives up (`SQLITE_LOCKED`, silently,
/// without even recording an attempt) if that connection still has a
/// transaction open. cozo 0.7.6's `SqliteTx::get` and `exists` step their
/// cached statement once and return on the first row without resetting it,
/// and `commit` runs while those statements are still alive, so after nearly
/// every commit (an upsert reads before it writes) the committing connection
/// still holds a read transaction. Its WAL was therefore never checkpointed
/// while the store stayed open: a daemon holding one KB for two days grew a
/// 12 GB WAL with not one checkpoint attempted (2026-11-08). The WAL is
/// folded back only when the last connection closes — never, for a daemon.
/// `wal_tests` reproduces the mechanism with plain SQLite.
///
/// A PASSIVE checkpoint from a separate connection, after cozo's transaction
/// has ended, is unaffected: it copies what no reader still needs and never
/// blocks or waits.
pub(super) struct Checkpointer {
    path: std::path::PathBuf,
    writes: std::sync::atomic::AtomicU32,
    conn: std::sync::Mutex<Option<sqlite::ConnectionThreadSafe>>,
}

impl Checkpointer {
    pub(super) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            writes: std::sync::atomic::AtomicU32::new(0),
            conn: std::sync::Mutex::new(None),
        }
    }

    /// Count one committed mutating script; checkpoint every
    /// [`CHECKPOINT_EVERY_WRITES`].
    pub(super) fn note_write(&self) {
        use std::sync::atomic::Ordering;
        if self.writes.fetch_add(1, Ordering::Relaxed) + 1 >= CHECKPOINT_EVERY_WRITES {
            self.writes.store(0, Ordering::Relaxed);
            self.checkpoint();
        }
    }

    /// A PASSIVE checkpoint: `(busy, wal_frames, checkpointed_frames)` as
    /// SQLite reports them, or `None` if it could not run. Never fails the
    /// caller: an unchecked WAL is the status quo, not a fault.
    pub(super) fn checkpoint(&self) -> Option<(i64, i64, i64)> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = sqlite::Connection::open_thread_safe(&self.path).ok();
        }
        let conn = guard.as_ref()?;
        let mut out = None;
        let ran = conn.iterate("PRAGMA wal_checkpoint(PASSIVE);", |row| {
            let n = |i: usize| {
                row.get(i)
                    .and_then(|(_, v)| *v)
                    .and_then(|v| v.parse().ok())
            };
            if let (Some(busy), Some(log), Some(done)) = (n(0), n(1), n(2)) {
                out = Some((busy, log, done));
            }
            true
        });
        if let Err(e) = ran {
            tracing::debug!(path = %self.path.display(), error = %e, "KB store: checkpoint did not run");
            return None;
        }
        out
    }
}
