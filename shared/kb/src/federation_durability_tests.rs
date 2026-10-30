//! KB write contract, part 6: the KB registry never loses KBs.
//!
//! The registry (`kb-registry.toml`) is the only record of which KBs exist.
//! Contract: an update never persists a registry it could not read (an
//! unreadable or newer-schema file must not be overwritten with "empty plus my
//! change"), and an update never runs without the advisory lock that
//! serialises the processes sharing it — contention is an error to the caller,
//! not a warning followed by an unlocked write.

use super::*;

fn registry_with_two_kbs(dir: &std::path::Path) -> String {
    let (_, r) = KbRegistry::update(dir, |reg| {
        reg.register_native("first".into(), dir, None)?;
        reg.register_native("second".into(), dir, None)
    })
    .expect("save");
    r.expect("register");
    std::fs::read_to_string(dir.join("kb-registry.toml")).unwrap()
}

/// Guard: the fixture really persists two KBs.
#[test]
fn the_fixture_persists_two_kbs() {
    let tmp = tempfile::TempDir::new().unwrap();
    registry_with_two_kbs(tmp.path());
    let reg = KbRegistry::load(tmp.path());
    assert!(reg.find("first").is_some() && reg.find("second").is_some());
}

/// A registry this build cannot parse (corrupt, or written by a newer MAE)
/// must not be replaced by an "empty + my change" registry. One test per kind
/// of damage, so one cannot mask the other.
fn assert_update_leaves_damaged_registry_alone(label: &str, damage: impl Fn(&str) -> String) {
    let tmp = tempfile::TempDir::new().unwrap();
    let original = registry_with_two_kbs(tmp.path());
    let damaged = damage(&original);
    assert_ne!(
        damaged, original,
        "{label}: the damage must change the file"
    );
    std::fs::write(tmp.path().join("kb-registry.toml"), &damaged).unwrap();

    let _ = KbRegistry::update(tmp.path(), |reg| {
        reg.register_native("third".into(), tmp.path(), None)
    });

    let after = std::fs::read_to_string(tmp.path().join("kb-registry.toml")).unwrap();
    assert_eq!(
        after, damaged,
        "{label}: an update rewrote a registry it could not read, dropping every KB in it"
    );
}

#[test]
fn proof_an_update_never_overwrites_a_truncated_registry() {
    assert_update_leaves_damaged_registry_alone("truncated", |s| s[..s.len() / 2].to_string());
}

#[test]
fn proof_an_update_never_overwrites_a_newer_schema_registry() {
    assert_update_leaves_damaged_registry_alone("newer schema value", |s| {
        s.replacen("store-is-truth", "some-future-policy", 1)
    });
}

/// Env var that turns a re-executed test binary into the lock holder.
const HOLDER_ENV: &str = "MAE_TEST_REGISTRY_LOCK_HOLDER_DIR";
/// How long the holder keeps the lock if the update never returns: long
/// enough that a "retry for a while, then write without the lock" fallback
/// gives up first, and bounded so a correct "wait" implementation finishes.
const HOLDER_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// Holder mode (child process): take the registry lock, announce it, keep it
/// until the parent says its update returned (or `HOLDER_CAP` passes), then
/// report whether the registry changed while the lock was held — exit 0 if
/// not, 3 if it did, 2 if the lock could not be taken.
fn hold_registry_lock(dir: &std::path::Path) -> i32 {
    let path = dir.join("kb-registry.toml");
    let held = mae_mcp::file_lock::LockGuard::try_acquire(&path);
    if !held.acquired() {
        return 2;
    }
    let at_lock = std::fs::read(&path).unwrap();
    std::fs::write(dir.join("locked"), b"").unwrap();
    let deadline = std::time::Instant::now() + HOLDER_CAP;
    while !dir.join("release").exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let changed = std::fs::read(&path).unwrap() != at_lock;
    drop(held);
    if changed {
        3
    } else {
        0
    }
}

/// While ANOTHER PROCESS holds the registry lock, an update never writes the
/// registry. It may fail (and then wrote nothing), or wait for the lock; it
/// may not give up waiting and write anyway.
///
/// The holder is a separate process (this test binary re-executed), because
/// the contract is between processes and a per-process lock (POSIX `fcntl`)
/// would let a thread of the same process through. The holder keeps the lock
/// until the update returns, so the oracle — the registry's bytes at lock and
/// at release — does not depend on timing. A correct waiting implementation
/// passes after `HOLDER_CAP`; a failing one passes at once.
#[test]
fn proof_an_update_never_runs_without_the_lock() {
    if let Ok(dir) = std::env::var(HOLDER_ENV) {
        std::process::exit(hold_registry_lock(std::path::Path::new(&dir)));
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let original = registry_with_two_kbs(tmp.path());
    let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "federation::durability_tests::proof_an_update_never_runs_without_the_lock",
            "--test-threads=1",
        ])
        .env(HOLDER_ENV, tmp.path())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn the holder process");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !tmp.path().join("locked").exists() {
        if let Some(status) = holder.try_wait().unwrap() {
            panic!("fixture: the holder exited before locking: {status}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fixture: holder never locked"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let saved = KbRegistry::update(tmp.path(), |reg| {
        reg.register_native("third".into(), tmp.path(), None)
    })
    .map(|_| ());
    std::fs::write(tmp.path().join("release"), b"").unwrap();
    let status = holder.wait().unwrap();
    assert_ne!(
        status.code(),
        Some(2),
        "fixture: the holder could not take the lock"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "the update wrote the registry while another process held the lock \
         (update result: {saved:?})"
    );
    if saved.is_err() {
        let after = std::fs::read_to_string(tmp.path().join("kb-registry.toml")).unwrap();
        assert_eq!(after, original, "a failed update still wrote the registry");
    }
}

/// A LEGACY registry (`kb/registry.toml`) that cannot be read, with no live
/// file yet: plain `load` must neither promote it (that created an EMPTY live
/// registry, after which the legacy file was ignored for good) nor touch it, and
/// a later `update` must refuse rather than save "empty + my change".
#[test]
fn an_unreadable_legacy_registry_is_neither_promoted_nor_overwritten() {
    let tmp = tempfile::TempDir::new().unwrap();
    let legacy = tmp.path().join("kb").join("registry.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let damaged = b"[[instances]]\nname = \"kept\"\nnot toml".to_vec();
    std::fs::write(&legacy, &damaged).unwrap();
    let live = tmp.path().join("kb-registry.toml");

    let loaded = KbRegistry::load(tmp.path());
    assert!(
        loaded.instances.is_empty(),
        "fixture: the legacy file is unreadable"
    );
    assert!(
        !live.exists(),
        "load promoted an unreadable legacy registry as an empty live one"
    );

    let r = KbRegistry::update(tmp.path(), |reg| {
        reg.register_native("new".into(), tmp.path(), None)
    });
    assert!(
        r.is_err(),
        "update saved over an unreadable legacy registry: {r:?}"
    );
    assert!(!live.exists(), "update created a live registry");
    assert_eq!(
        std::fs::read(&legacy).unwrap(),
        damaged,
        "the legacy file changed"
    );
}
