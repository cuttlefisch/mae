//! ADR-040 §Recovery-key (B2): durable per-KB **collection op-log** store for an editing
//! member/owner.
//!
//! The collection (`kbc:`) doc is the key-blind, signed CRDT that carries a KB's membership
//! op-log, roster, policy, and (for E2e) the *ciphertext* content-key wraps — **no plaintext
//! secrets**. The network task holds it in memory (`kb_collections`), seeded from the join /
//! share response and advanced by inbound `kbc:` deltas. Persisting it here lets a member:
//!   - keep its collection view across an editor restart (durability), and
//!   - **recover a lost identity**: a member who restores its data dir on a new machine (with a
//!     fresh key) still holds the op-log it needs to author a recovery `Rebind`
//!     (`collab-recover-identity`) — without re-fetching it from the daemon (which only serves
//!     it to members), so a NON-member recovering peer is not blocked. See ADR-040 §Recovery.
//!
//! Unlike [`crate::content_key_store`], this holds no secret — but it lives in the member's
//! data dir under the same XDG-first, `0700`-dir / `0600`-file posture (the roster is not
//! world-readable). The **daemon never calls this** — it has its own authoritative store.

use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/mae/collab/collections` (HOME-relative fallback), or `None` if neither
/// `XDG_DATA_HOME` nor `HOME` is set.
pub fn collections_dir() -> Option<PathBuf> {
    crate::identity::default_collab_dir().map(|d| d.join("collections"))
}

/// The on-disk path for `kb_id`'s collection. The `kb_id` is hex-encoded into the filename so
/// an arbitrary id (which may contain `/`, `:`, `..`, …) can never escape the dir or collide
/// with a sanitized form of a different id.
fn coll_path(dir: &Path, kb_id: &str) -> PathBuf {
    dir.join(format!("{}.kbc", hex::encode(kb_id.as_bytes())))
}

/// Persist `kb_id`'s collection op-log bytes (`0600` in a `0700` dir, created if needed).
/// Overwrites any prior snapshot — the caller passes the latest full collection state.
///
/// **Crash-atomic.** The prior in-place `fs::write` could leave a truncated/corrupt op-log if
/// the process died mid-write — and this store is a member's *only* copy of the op-log it needs
/// to author a recovery `Rebind` (a non-member can't re-fetch it from the daemon). So we write
/// to a sibling temp on the SAME dir (same filesystem ⇒ `rename` is atomic), fsync it, then
/// rename over the target. A crash can only leave a stale `.tmp` (ignored by `load`/`load_all`,
/// which read only `.kbc`) — never a corrupt op-log at the real path.
pub fn save(dir: &Path, kb_id: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    secure_dir(dir);
    let path = coll_path(dir, kb_id);
    let tmp = path.with_extension("kbc.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        secure_file(&tmp);
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    secure_file(&path);
    Ok(())
}

/// Load `kb_id`'s persisted collection bytes, or `None` if absent.
pub fn load(dir: &Path, kb_id: &str) -> Option<Vec<u8>> {
    std::fs::read(coll_path(dir, kb_id)).ok()
}

/// Load EVERY persisted collection as `(kb_id, bytes)`. Used at connect to re-seed the network
/// task's `kb_collections` from disk so a restarted / restored member has its op-logs without a
/// re-fetch. Skips files whose name is not a valid hex-encoded id or that fail to read.
pub fn load_all(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("kbc") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(kb_bytes) = hex::decode(stem) else {
            continue;
        };
        let Ok(kb_id) = String::from_utf8(kb_bytes) else {
            continue;
        };
        if let Ok(bytes) = std::fs::read(&path) {
            out.push((kb_id, bytes));
        }
    }
    out
}

/// Remove `kb_id`'s persisted collection (e.g. on leave). Absent ⇒ `Ok(())`.
pub fn remove(dir: &Path, kb_id: &str) -> std::io::Result<()> {
    match std::fs::remove_file(coll_path(dir, kb_id)) {
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

#[cfg(unix)]
fn secure_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn secure_file(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    // A fresh, per-test directory (tests run in parallel — each needs its OWN dir, never a
    // shared/address-derived path).
    fn tmp(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mae-cs-{test}"))
    }

    #[test]
    fn save_load_round_trips_arbitrary_kb_ids() {
        let dir = tmp("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        // ids with bytes a sanitizer would mangle — hex naming must survive them.
        let cases: &[(&str, &[u8])] = &[
            ("kb/with:slashes::and..dots", b"ops-A"),
            ("plain", b"ops-B"),
            ("", b"ops-empty-id"),
        ];
        for (id, bytes) in cases {
            save(&dir, id, bytes).unwrap();
            assert_eq!(load(&dir, id).as_deref(), Some(*bytes));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_returns_every_persisted_collection() {
        let dir = tmp("loadall");
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, "alpha", b"a").unwrap();
        save(&dir, "beta/two", b"bb").unwrap();
        save(&dir, "gamma", b"ccc").unwrap();
        let mut got = load_all(&dir);
        got.sort();
        let mut want = vec![
            ("alpha".to_string(), b"a".to_vec()),
            ("beta/two".to_string(), b"bb".to_vec()),
            ("gamma".to_string(), b"ccc".to_vec()),
        ];
        want.sort();
        assert_eq!(got, want, "load_all round-trips every id verbatim");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_absent_is_none_and_remove_is_idempotent() {
        let dir = tmp("absent");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load(&dir, "nope").is_none());
        assert!(load_all(&dir).is_empty(), "no dir ⇒ empty, not a panic");
        remove(&dir, "nope").unwrap(); // absent ⇒ Ok
        save(&dir, "x", b"1").unwrap();
        remove(&dir, "x").unwrap();
        assert!(load(&dir, "x").is_none(), "removed ⇒ gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_ignores_foreign_files() {
        let dir = tmp("foreign");
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, "real", b"ok").unwrap();
        std::fs::write(dir.join("not-hex.kbc"), b"junk").unwrap(); // bad stem
        std::fs::write(dir.join("66.txt"), b"wrong-ext").unwrap(); // wrong ext
        let got = load_all(&dir);
        assert_eq!(
            got,
            vec![("real".to_string(), b"ok".to_vec())],
            "only well-formed .kbc files with hex-decodable stems are loaded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_is_atomic_a_stale_tmp_never_shadows_the_committed_op_log() {
        let dir = tmp("atomic");
        let _ = std::fs::remove_dir_all(&dir);
        // Commit a real snapshot.
        save(&dir, "kbA", b"committed-v1").unwrap();
        // Simulate a crash DURING a later save: a leftover temp sibling with garbage. It must
        // never be surfaced by load / load_all, and must not disturb the committed op-log.
        let tmp_sibling = coll_path(&dir, "kbA").with_extension("kbc.tmp");
        std::fs::write(&tmp_sibling, b"torn-write-garbage").unwrap();
        assert_eq!(
            load(&dir, "kbA").as_deref(),
            Some(&b"committed-v1"[..]),
            "the committed op-log is intact despite a stale .tmp"
        );
        assert_eq!(
            load_all(&dir),
            vec![("kbA".to_string(), b"committed-v1".to_vec())],
            "load_all ignores the .tmp and returns only the committed .kbc"
        );
        // A subsequent successful save commits the new bytes atomically (rename over target).
        save(&dir, "kbA", b"committed-v2").unwrap();
        assert_eq!(load(&dir, "kbA").as_deref(), Some(&b"committed-v2"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
