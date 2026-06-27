//! ADR-037 Phase 3b: durable per-KB **content-key** store for a KB owner/member.
//!
//! An E2E-encrypted KB has a per-KB symmetric content key. The OWNER generates it when
//! enabling encryption and must hold it durably to (a) wrap it to future members and
//! (b) decrypt its own content after a restart. A member persists the key it recovered
//! from the membership op-log so it need not re-derive every session.
//!
//! This is the ONLY place a content key is persisted, and it lives ONLY in an editing
//! member's data dir. The **daemon stays key-blind** — it never calls into this module
//! (enforced by the daemon code + tests, and by the fact that the daemon never holds a
//! content key to persist). Files are `0600` in a `0700` dir, XDG-first (cross-OS), the
//! same posture as the PSK [`crate::keystore`].

use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/mae/collab/content_keys` (HOME-relative fallback), or `None` if
/// neither `XDG_DATA_HOME` nor `HOME` is set.
pub fn content_keys_dir() -> Option<PathBuf> {
    crate::identity::default_collab_dir().map(|d| d.join("content_keys"))
}

/// The on-disk path for `kb_id`'s key. The `kb_id` is hex-encoded into the filename so
/// an arbitrary KB id (which may contain `/`, `:`, `..`, …) can never escape the dir or
/// collide with a sanitized form of a different id.
fn key_path(dir: &Path, kb_id: &str) -> PathBuf {
    dir.join(format!("{}.key", hex::encode(kb_id.as_bytes())))
}

/// Persist the 32-byte content key for `kb_id` (hex, `0600`), creating the dir (`0700`)
/// if needed.
pub fn save(dir: &Path, kb_id: &str, key: &[u8; 32]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    secure_dir(dir);
    crate::keystore::write_secure(&key_path(dir, kb_id), &hex::encode(key))
}

/// Load `kb_id`'s content key, or `None` if absent / malformed (wrong length, bad hex).
pub fn load(dir: &Path, kb_id: &str) -> Option<[u8; 32]> {
    let content = std::fs::read_to_string(key_path(dir, kb_id)).ok()?;
    let bytes = hex::decode(content.trim()).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// Remove `kb_id`'s persisted key (e.g. on leave). Absent ⇒ `Ok(())`.
pub fn remove(dir: &Path, kb_id: &str) -> std::io::Result<()> {
    match std::fs::remove_file(key_path(dir, kb_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn secure_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn secure_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    // A fresh, per-test directory (tests run in parallel — each needs its OWN dir, so
    // the name is the test's, never a shared/address-derived path).
    fn tmp(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mae-ck-{test}"))
    }

    #[test]
    fn save_load_round_trips_distinct_keys_per_kb() {
        let dir = tmp("round_trip");
        let _ = std::fs::remove_dir_all(&dir);
        // Two DISTINCT keys for two DISTINCT KBs — not a single-value tautology.
        let mut k1 = [0u8; 32];
        let mut k2 = [0u8; 32];
        for i in 0..32 {
            k1[i] = i as u8;
            k2[i] = (255 - i) as u8;
        }
        save(&dir, "kbA", &k1).unwrap();
        save(&dir, "kbB", &k2).unwrap();
        assert_eq!(load(&dir, "kbA"), Some(k1), "kbA round-trips");
        assert_eq!(load(&dir, "kbB"), Some(k2), "kbB round-trips its OWN key");
        assert_ne!(load(&dir, "kbA"), load(&dir, "kbB"), "keys are not crossed");
        assert_eq!(load(&dir, "kbC"), None, "absent KB ⇒ None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kb_id_with_separators_cannot_escape_the_dir() {
        let dir = tmp("traversal");
        let _ = std::fs::remove_dir_all(&dir);
        let k = [7u8; 32];
        // A traversal-flavored id must stay INSIDE dir + round-trip, not write elsewhere.
        save(&dir, "../../etc/evil", &k).unwrap();
        assert_eq!(load(&dir, "../../etc/evil"), Some(k));
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1, "exactly one key file, inside the dir");
        let name = entries[0].file_name();
        assert!(
            !name.to_string_lossy().contains(".."),
            "the filename carries no path-traversal"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_file_loads_as_none() {
        let dir = tmp("malformed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Wrong length (16 bytes hex) ⇒ None, not a panic / truncated key.
        crate::keystore::write_secure(&key_path(&dir, "kbX"), &hex::encode([1u8; 16])).unwrap();
        assert_eq!(
            load(&dir, "kbX"),
            None,
            "a 16-byte file is not a 32-byte key"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("mode");
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, "kbP", &[9u8; 32]).unwrap();
        let mode = std::fs::metadata(key_path(&dir, "kbP"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "content-key file must be 0600");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
