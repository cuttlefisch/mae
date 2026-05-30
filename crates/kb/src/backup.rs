//! KB backup and restore — periodic SQLite snapshots with retention.
//!
//! Backups are stored as `backups/{slug}/{timestamp}.sqlite` under the KB
//! data directory. Each backup is a simple copy of the live `kb.sqlite`.
//!
//! - Periodic: configurable interval (default: daily, option `kb_backup_interval`)
//! - Retention: keep last N (default: 7, option `kb_backup_retention`)
//! - Pre-sync: auto-backup before first remote sync of a local KB
//! - Recovery: `:kb-restore {slug} {timestamp}` replaces the live DB

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

    std::fs::copy(source_db, &backup_path)?;
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
        std::fs::copy(&target, &pre_restore_path)?;
        info!(
            slug,
            path = %pre_restore_path.display(),
            "created pre-restore backup"
        );
    }

    std::fs::copy(&backup_path, &target)?;
    info!(slug, timestamp, "restored KB from backup");
    Ok(target)
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
            node_count: 10,
            org_dir: None,
        };
        let db_path = dir.init_local_kb(slug, &meta).unwrap();
        std::fs::write(&db_path, b"fake sqlite content").unwrap();
        (dir, slug.to_string())
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
        std::fs::write(&backup_path, b"backup data").unwrap();

        // Restore it
        let target = restore_backup(&dir, &slug, backup_ts).unwrap();
        assert!(target.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"backup data");

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
