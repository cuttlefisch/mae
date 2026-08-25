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
