//! KB backup and restore — periodic SQLite snapshots with retention.
//!
//! Backups are stored as `backups/{slug}/{timestamp}.sqlite` under the KB
//! data directory, each a self-contained snapshot made with `VACUUM INTO`.
//!
//! The per-KB half is NOT YET WIRED (#263): no task calls `create_backup` /
//! `restore_backup`, and the options named for them (`kb_backup_interval`,
//! `kb_backup_retention`) are registered as RESERVED. There is no `:kb-restore`
//! command — an earlier version of this comment said there was. [`snapshot`]
//! IS used: `mae-daemon backup create` copies every store with it.
//!
//! @ai-caution: [kb-truth] A KB store is SQLite in WAL mode, so it is NOT one
//! file: recent commits live in `kb.sqlite-wal` until a checkpoint. This module
//! used `fs::copy` of `kb.sqlite` alone, which silently drops every
//! uncheckpointed write — a backup that looks complete and is not (the same
//! "a store is not one file" failure that has broken other things in this
//! tree). `VACUUM INTO` asks SQLite itself for a consistent, WAL-free copy.
//! Restoring snapshots the live DB first, which recovers any stale WAL into
//! it, so no old WAL is left to be replayed over the restored file.

use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::data_dir::KbDataDir;

/// A single backup entry.
#[derive(Debug, Clone)]
pub struct BackupEntry {
    pub slug: String,
    pub timestamp: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// Create a backup of a KB's SQLite database.
///
/// Returns the backup path on success.
pub fn create_backup(data_dir: &KbDataDir, slug: &str) -> std::io::Result<PathBuf> {
    let source_db = data_dir.local_kb_db(slug);
    if !source_db.exists() {
        // Try shared
        let shared_db = data_dir.shared_kb_db(slug);
        if shared_db.exists() {
            return create_backup_from(&shared_db, data_dir, slug);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no KB database found for slug '{slug}'"),
        ));
    }
    create_backup_from(&source_db, data_dir, slug)
}

fn create_backup_from(
    source_db: &Path,
    data_dir: &KbDataDir,
    slug: &str,
) -> std::io::Result<PathBuf> {
    let backup_dir = data_dir.backup_dir(slug);
    std::fs::create_dir_all(&backup_dir)?;

    let timestamp = iso_timestamp();
    let backup_path = backup_dir.join(format!("{timestamp}.sqlite"));

    snapshot(source_db, &backup_path)?;
    let size = std::fs::metadata(&backup_path)?.len();

    info!(slug, timestamp, size_bytes = size, "created KB backup");
    Ok(backup_path)
}

/// List all backups for a KB, sorted newest first.
pub fn list_backups(data_dir: &KbDataDir, slug: &str) -> Vec<BackupEntry> {
    let backup_dir = data_dir.backup_dir(slug);
    let Ok(entries) = std::fs::read_dir(&backup_dir) else {
        return Vec::new();
    };

    let mut backups: Vec<BackupEntry> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_str()?.to_string();
            if !name.ends_with(".sqlite") {
                return None;
            }
            let timestamp = name.strip_suffix(".sqlite")?.to_string();
            let size_bytes = e.metadata().ok()?.len();
            Some(BackupEntry {
                slug: slug.to_string(),
                timestamp,
                path: e.path(),
                size_bytes,
            })
        })
        .collect();

    // Sort newest first
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    backups
}

/// Prune old backups, keeping at most `retain` newest entries.
///
/// Returns number of backups removed.
pub fn prune_backups(data_dir: &KbDataDir, slug: &str, retain: usize) -> std::io::Result<usize> {
    let backups = list_backups(data_dir, slug);
    if backups.len() <= retain {
        return Ok(0);
    }

    let to_remove = &backups[retain..];
    let mut removed = 0;
    for entry in to_remove {
        match std::fs::remove_file(&entry.path) {
            Ok(()) => {
                debug!(slug, timestamp = entry.timestamp, "pruned old backup");
                removed += 1;
            }
            Err(e) => {
                warn!(
                    slug,
                    timestamp = entry.timestamp,
                    error = %e,
                    "failed to prune backup"
                );
            }
        }
    }

    if removed > 0 {
        info!(slug, removed, retained = retain, "pruned KB backups");
    }
    Ok(removed)
}

/// Restore a KB from a backup. Copies the backup over the live database.
///
/// Creates a pre-restore backup of the current live DB first.
pub fn restore_backup(
    data_dir: &KbDataDir,
    slug: &str,
    timestamp: &str,
) -> std::io::Result<PathBuf> {
    let backup_path = data_dir
        .backup_dir(slug)
        .join(format!("{timestamp}.sqlite"));
    if !backup_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("backup not found: {slug}/{timestamp}"),
        ));
    }

    // Determine target (local or shared)
    let target = if data_dir.local_kb_dir(slug).exists() {
        data_dir.local_kb_db(slug)
    } else if data_dir.shared_kb_dir(slug).exists() {
        data_dir.shared_kb_db(slug)
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no KB directory found for slug '{slug}'"),
        ));
    };

    // Pre-restore backup of current state
    if target.exists() {
        let pre_restore_dir = data_dir.backup_dir(slug);
        std::fs::create_dir_all(&pre_restore_dir)?;
        let pre_restore_path =
            pre_restore_dir.join(format!("{}-pre-restore.sqlite", iso_timestamp()));
        snapshot(&target, &pre_restore_path)?;
        info!(
            slug,
            path = %pre_restore_path.display(),
            "created pre-restore backup"
        );
    }

    // A stale WAL beside the restored file would be replayed over it on the next
    // open. The pre-restore snapshot above OPENS the live DB, so SQLite recovers
    // any WAL into it first — the pre-restore backup holds even uncheckpointed
    // writes — and only then are the sidecars removed. Removed EXPLICITLY:
    // "SQLite deletes the WAL on close" is a build option, not a property, and
    // the macOS system SQLite keeps it (this test went red there when it relied
    // on that). The KB must not be open elsewhere while restoring — a live
    // connection would write a new WAL after this point.
    for side in crate::kb_build::wal_sidecars(&target) {
        match std::fs::remove_file(&side) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::fs::copy(&backup_path, &target)?;
    info!(slug, timestamp, "restored KB from backup");
    Ok(target)
}

/// A consistent, self-contained copy of the SQLite database at `src`,
/// including writes still in its WAL. `dst` must not exist.
pub fn snapshot(src: &Path, dst: &Path) -> std::io::Result<()> {
    let to_io = |e: sqlite::Error| std::io::Error::other(e.to_string());
    let conn = sqlite::open(src).map_err(to_io)?;
    let quoted = dst.to_string_lossy().replace('\'', "''");
    conn.execute(format!("VACUUM INTO '{quoted}'"))
        .map_err(to_io)
}

fn iso_timestamp() -> String {
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as ISO-ish: YYYYMMDDTHHMMSS (avoids colons in filenames)
    // We don't have chrono, so use a simple numeric format
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_dir::LocalKbMeta;

    fn setup_test_kb(tmp: &Path) -> (KbDataDir, String) {
        let dir = KbDataDir::from_root(tmp.join("kb")).unwrap();
        let slug = "test-kb";
        let meta = LocalKbMeta {
            name: "Test KB".to_string(),
            uuid: "test-uuid".to_string(),
            created_at: "2026-01-01".to_string(),
            org_dir: None,
        };
        let db_path = dir.init_local_kb(slug, &meta).unwrap();
        // A real SQLite database: `snapshot` asks SQLite for the copy, so a
        // file of arbitrary bytes is (correctly) rejected.
        let conn = sqlite::open(&db_path).unwrap();
        conn.execute("CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('live');")
            .unwrap();
        (dir, slug.to_string())
    }

    fn count_rows(db: &Path) -> i64 {
        let conn = sqlite::open(db).unwrap();
        let mut st = conn.prepare("SELECT count(*) FROM t").unwrap();
        st.next().unwrap();
        st.read::<i64, _>(0).unwrap()
    }

    /// The failure this module had: rows committed but still in the WAL (no
    /// checkpoint yet — the writer is still open) must be IN the backup. A
    /// plain file copy of `kb.sqlite` loses them and still looks like a backup.
    #[test]
    fn a_backup_includes_writes_still_in_the_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, slug) = setup_test_kb(tmp.path());
        let db = dir.local_kb_db(&slug);
        let writer = sqlite::open(&db).unwrap();
        writer
            .execute("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        for i in 0..50 {
            writer
                .execute(format!("INSERT INTO t VALUES ('wal-{i}')"))
                .unwrap();
        }
        let wal = PathBuf::from(format!("{}-wal", db.display()));
        assert!(
            std::fs::metadata(&wal)
                .map(|m| m.len() > 0)
                .unwrap_or(false),
            "premise: the new rows are in the WAL, not yet checkpointed"
        );

        let backup = create_backup(&dir, &slug).unwrap();
        drop(writer);

        assert_eq!(count_rows(&backup), 51, "1 checkpointed + 50 WAL-only rows");
    }

    #[test]
    fn create_and_list_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, slug) = setup_test_kb(tmp.path());

        let path = create_backup(&dir, &slug).unwrap();
        assert!(path.exists());

        let backups = list_backups(&dir, &slug);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].slug, slug);
    }

    #[test]
    fn prune_respects_retention() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, slug) = setup_test_kb(tmp.path());

        // Create 5 backups
        let backup_dir = dir.backup_dir(&slug);
        std::fs::create_dir_all(&backup_dir).unwrap();
        for i in 0..5 {
            let path = backup_dir.join(format!("{:010}.sqlite", 1000 + i));
            std::fs::write(&path, b"data").unwrap();
        }

        assert_eq!(list_backups(&dir, &slug).len(), 5);

        let removed = prune_backups(&dir, &slug, 3).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(list_backups(&dir, &slug).len(), 3);
    }

    #[test]
    fn restore_creates_pre_restore_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, slug) = setup_test_kb(tmp.path());

        // Create a backup
        let backup_dir = dir.backup_dir(&slug);
        std::fs::create_dir_all(&backup_dir).unwrap();
        let backup_ts = "1234567890";
        let backup_path = backup_dir.join(format!("{backup_ts}.sqlite"));
        snapshot(&dir.local_kb_db(&slug), &backup_path).unwrap();
        // Leave a VALID stale WAL beside the live DB, holding a write made
        // after the backup — what a store that crashed mid-session has. (Junk
        // bytes would prove nothing: SQLite discards an invalid WAL itself.)
        let live = dir.local_kb_db(&slug);
        let live_wal = PathBuf::from(format!("{}-wal", live.display()));
        let writer = sqlite::open(&live).unwrap();
        writer
            .execute("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        writer
            .execute("INSERT INTO t VALUES ('after-backup')")
            .unwrap();
        let stale = std::fs::read(&live_wal).unwrap();
        drop(writer); // checkpoints and removes the WAL...
        std::fs::write(&live_wal, &stale).unwrap(); // ...which a crash would not have

        // Restore it
        let target = restore_backup(&dir, &slug, backup_ts).unwrap();
        assert!(target.exists());
        assert!(
            !live_wal.exists(),
            "no stale WAL is left beside the restore"
        );
        assert_eq!(
            count_rows(&target),
            1,
            "the backup's content, not the live one's"
        );

        // The write that was only in the stale WAL is not lost: the
        // pre-restore snapshot recovered it.
        let pre = list_backups(&dir, &slug)
            .into_iter()
            .find(|b| b.timestamp.ends_with("-pre-restore"))
            .expect("a pre-restore backup");
        assert_eq!(
            count_rows(&pre.path),
            2,
            "pre-restore holds the WAL-only write"
        );

        // Should have created a pre-restore backup
        let backups = list_backups(&dir, &slug);
        assert!(
            backups.len() >= 2,
            "should have original + pre-restore backup"
        );
    }

    #[test]
    fn restore_missing_backup_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, slug) = setup_test_kb(tmp.path());

        let result = restore_backup(&dir, &slug, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn create_backup_missing_db_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = KbDataDir::from_root(tmp.path().join("kb")).unwrap();

        let result = create_backup(&dir, "nonexistent");
        assert!(result.is_err());
    }
}
