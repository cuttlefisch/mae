//! ADR-115 D1: a KB's attachments in `mae-daemon backup`.
//!
//! The metadata database is snapshotted like every other SQLite file
//! (`VACUUM INTO`); then exactly the files a row of THAT SNAPSHOT names are
//! copied. A blob file is immutable and is in place before its row is
//! written, so every file the snapshot names exists and is complete, and a
//! blob committed after the snapshot is in neither. A file no row names (left
//! by a crash between rename and row) is not backed up: nothing references it.
//!
//! When P2's sweep deletes blobs, a blob swept between the snapshot and its
//! copy fails the backup with a missing source rather than archiving a row
//! without its file; a retry takes a fresh snapshot.

use std::path::Path;

use mae_daemon::blob_store::{list_rows, valid_kb_id, valid_sha256, DB_FILE, DIR};

use super::create::{Kind, Source};

/// The blob root inside the archive, mirroring the collab data directory.
pub(crate) fn archive_root() -> String {
    format!("collab/{DIR}")
}

/// The metadata database's path inside the archive.
pub(crate) fn db_archive_path() -> String {
    format!("{}/{DB_FILE}", archive_root())
}

/// The metadata database's source, if the store exists.
pub(crate) fn db_source(collab_dir: &Path) -> Option<Source> {
    let src = collab_dir.join(DIR).join(DB_FILE);
    src.is_file().then(|| Source {
        kind: Kind::Sqlite,
        src,
        archive_path: db_archive_path(),
    })
}

/// The blob files the staged database snapshot names, as plain copies from
/// the live blob root `live` (the database source's directory).
pub(crate) fn file_sources(staged_db: &Path, live: &Path) -> Result<Vec<Source>, String> {
    let rows = list_rows(staged_db).map_err(|e| format!("{}: {e}", staged_db.display()))?;
    rows.into_iter()
        .map(|(kb, sha)| {
            if !valid_kb_id(&kb) || !valid_sha256(&sha) {
                return Err(format!(
                    "the blob database names an invalid blob {kb}/{sha}"
                ));
            }
            let rel = format!("{kb}/{}/{sha}", &sha[..2]);
            Ok(Source {
                kind: Kind::Plain,
                src: live.join(&rel),
                archive_path: format!("{}/{rel}", archive_root()),
            })
        })
        .collect()
}
