//! `mae-daemon backup create|restore|verify`.
//!
//! The oracle is always what the fixture INSERTED, never the manifest's own
//! numbers: a backup that faithfully records a wrong count must still fail
//! here. Every archive a test tampers with is one this code produced, so the
//! refusals are tested against the real format rather than a hand-built one.

use super::isolated;
use crate::backup::{self, check_archive_path, store_key, Manifest, MANIFEST_PATH};
use crate::config::DaemonConfig;
use mae_kb::federation::KbRegistry;
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use mae_mcp::identity::Identity;
use std::path::{Path, PathBuf};

/// Insert `n` distinct nodes into `store`, ids prefixed so stores never share.
fn fill(store: &CozoKbStore, prefix: &str, n: usize) {
    for i in 0..n {
        let id = format!("{prefix}:{i}");
        store
            .insert_node(&Node::new(
                &id,
                format!("{prefix} {i}"),
                NodeKind::Note,
                "body",
            ))
            .unwrap();
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    config: DaemonConfig,
    /// (expected key, inserted node count)
    expected: Vec<(String, u64)>,
}

/// A daemon instance with: an unnamed main store, two registered KBs of
/// different sizes (one named with spaces and non-ASCII, so its key is
/// sanitised), one registered KB whose store is missing, a collab store, an
/// identity and an authorized_keys file.
fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let config = isolated(&root);
    let data_dir = config.effective_data_dir();
    std::fs::create_dir_all(&data_dir).unwrap();

    let main = CozoKbStore::open_with_engine(data_dir.join("daemon-kb.cozo"), "sqlite").unwrap();
    fill(&main, "main", 7);
    let mut registry = KbRegistry::default();
    let mut expected = vec![("kb.daemon".to_string(), 7)];
    for (name, n) in [("ops", 3usize), ("Notas técnicas", 11)] {
        let uuid = registry
            .register_native(name.into(), &data_dir, None)
            .unwrap();
        let db = registry
            .instances
            .iter()
            .find(|i| i.uuid == uuid)
            .unwrap()
            .db_path
            .clone();
        let store = CozoKbStore::open_with_engine(&db, "sqlite").unwrap();
        fill(&store, name, n);
        expected.push((store_key(name), n as u64));
    }
    registry
        .register_native("gone".into(), &data_dir, None)
        .unwrap();
    registry.save(&data_dir).unwrap();

    let collab = config.collab_data_dir();
    std::fs::create_dir_all(&collab).unwrap();
    mae_daemon::storage::SqliteBackend::open_with_pool_size(&collab.join("state.db"), 1).unwrap();
    let paths = config.instance_paths();
    Identity::generate("fixture")
        .save(paths.identity_dir.as_ref().unwrap())
        .unwrap();
    let peer = Identity::generate("peer").public().to_line();
    std::fs::write(paths.authorized_keys.unwrap(), format!("{peer}\n")).unwrap();
    Fixture {
        _tmp: tmp,
        root,
        config,
        expected,
    }
}

fn lines(dir: &Path) -> Vec<String> {
    let report = crate::backup::verify_report(dir).unwrap();
    assert!(
        report.problems.is_empty(),
        "verify found problems: {:?}",
        report.problems
    );
    report.lines
}

#[test]
fn a_backup_round_trips_every_store_with_the_counts_that_were_inserted() {
    let f = fixture();
    let archive = f.root.join("out/backup.tar");
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    backup::run_create(&f.config, &archive).unwrap();
    let dir = f.root.join("restored");
    backup::run_restore(&archive, &dir).unwrap();

    let got = lines(&dir);
    for (key, n) in &f.expected {
        assert!(
            got.contains(&format!("{key}={n}")),
            "{key}={n} missing from {got:?}"
        );
    }
    assert!(got.contains(&"identity=1".to_string()), "{got:?}");
    assert_eq!(
        got.len(),
        f.expected.len() + 1,
        "no store beyond the fixture's: {got:?}"
    );
    assert!(
        got.iter().any(|l| l.starts_with("kb.Notas_t")),
        "the non-ASCII name is sanitised"
    );
    for side in [
        "kb-registry.toml",
        "collab/state.db",
        "identity/authorized_keys",
    ] {
        assert!(dir.join(side).is_file(), "{side} was not restored");
    }
}

/// The daemon is running while the sidecar backs it up: writes still sitting in
/// an open store's WAL must be in the archive.
#[test]
fn a_backup_of_a_store_held_open_with_unflushed_writes_includes_them() {
    let f = fixture();
    let live = CozoKbStore::open_with_engine(
        f.config.effective_data_dir().join("daemon-kb.cozo"),
        "sqlite",
    )
    .unwrap();
    fill(&live, "late", 5);
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();
    drop(live);
    let dir = f.root.join("r");
    backup::run_restore(&archive, &dir).unwrap();
    assert!(
        lines(&dir).contains(&"kb.daemon=12".to_string()),
        "7 + 5 unflushed"
    );
}

/// A restored store whose bytes changed must fail verification, and so must a
/// count that disagrees with the manifest even when every hash matches.
#[test]
fn verify_fails_on_a_changed_file_and_on_a_count_the_manifest_disagrees_with() {
    let f = fixture();
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();

    let dir = f.root.join("hash");
    backup::run_restore(&archive, &dir).unwrap();
    let m = read_manifest(&dir);
    let store = dir.join(&m.stores[1].file);
    let mut bytes = std::fs::read(&store).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&store, bytes).unwrap();
    assert_ne!(backup::run_verify(&dir), 0, "a flipped byte must fail");

    let dir = f.root.join("count");
    backup::run_restore(&archive, &dir).unwrap();
    let mut m = read_manifest(&dir);
    m.stores[0].nodes += 1;
    std::fs::write(dir.join(MANIFEST_PATH), serde_json::to_vec(&m).unwrap()).unwrap();
    let report = crate::backup::verify_report(&dir).unwrap();
    assert!(
        report.problems.iter().any(|p| p.contains("when backed up")),
        "{:?}",
        report.problems
    );
}

#[test]
fn an_identity_whose_public_key_belongs_to_someone_else_is_not_counted() {
    let f = fixture();
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();
    let dir = f.root.join("r");
    backup::run_restore(&archive, &dir).unwrap();
    let other = Identity::generate("other").public().to_line();
    std::fs::write(dir.join("identity/id_ed25519.pub"), format!("{other}\n")).unwrap();
    // Re-bless the hash so ONLY the key mismatch can fail this.
    rebless(&dir, "identity/id_ed25519.pub");
    let report = crate::backup::verify_report(&dir).unwrap();
    assert!(
        report.lines.contains(&"identity=0".to_string()),
        "{:?}",
        report.lines
    );
    assert!(!report.problems.is_empty());
}

#[test]
fn restore_refuses_a_directory_that_is_not_empty_and_leaves_it_untouched() {
    let f = fixture();
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();
    let dir = f.root.join("occupied");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep.txt"), "mine").unwrap();
    assert!(backup::run_restore(&archive, &dir).is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("keep.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn create_refuses_to_overwrite_and_leaves_nothing_behind_when_there_is_nothing_to_back_up() {
    let f = fixture();
    let archive = f.root.join("b.tar");
    std::fs::write(&archive, "an older backup").unwrap();
    assert!(backup::run_create(&f.config, &archive).is_err());
    assert_eq!(
        std::fs::read_to_string(&archive).unwrap(),
        "an older backup"
    );

    let empty = tempfile::TempDir::new().unwrap();
    let config = isolated(empty.path());
    let out = empty.path().join("none.tar");
    let err = backup::run_create(&config, &out).unwrap_err();
    assert!(err.contains("nothing"), "{err}");
    let left: Vec<_> = std::fs::read_dir(empty.path()).unwrap().collect();
    assert!(
        left.is_empty(),
        "no archive, .parcial or staging dir may remain: {left:?}"
    );
}

#[test]
fn archive_paths_that_could_escape_or_shadow_are_rejected() {
    for bad in [
        "",
        "/etc/passwd",
        "../x",
        "a/../../x",
        "..",
        "a\\b",
        "./a",
        MANIFEST_PATH,
    ] {
        assert!(check_archive_path(bad).is_err(), "{bad:?} must be rejected");
    }
    for good in [
        "stores/daemon-kb.cozo",
        "identity/id_ed25519",
        "kb-registry.toml",
    ] {
        assert!(check_archive_path(good).is_ok(), "{good:?} is a valid path");
    }
}

/// Tampered archives, each built by taking a REAL archive's entries and
/// changing one thing.
#[test]
fn restore_refuses_archives_that_are_not_exactly_what_their_manifest_lists() {
    let f = fixture();
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();
    let entries = read_entries(&archive);

    let mut extra = entries.clone();
    extra.push(("stores/daemon-kb.cozo-wal".into(), b"stale wal".to_vec()));
    let mut missing = entries.clone();
    missing.pop();
    let mut twice = entries.clone();
    twice.push(entries[1].clone());
    let mut reordered = entries.clone();
    reordered.swap(0, 1);
    let mut traversal = entries.clone();
    traversal.push(("../outside".into(), b"x".to_vec()));
    let cases = [
        ("an unlisted -wal file", extra),
        ("a listed file missing", missing),
        ("a listed file twice", twice),
        ("the manifest not first", reordered),
        ("a path outside the directory", traversal),
    ];
    for (i, (what, list)) in cases.into_iter().enumerate() {
        let tar = f.root.join(format!("bad-{i}.tar"));
        write_raw(&tar, &list);
        let dir = f.root.join(format!("bad-{i}"));
        assert!(
            backup::run_restore(&tar, &dir).is_err(),
            "{what} must be refused"
        );
        assert!(
            !f.root.join("outside").exists(),
            "{what}: wrote outside the target"
        );
    }
    let tar = f.root.join("symlink.tar");
    write_with_symlink(&tar, &entries);
    let result = backup::run_restore(&tar, &f.root.join("sym"));
    assert!(result.is_err(), "a symlink must be refused: {result:?}");
}

#[cfg(unix)]
#[test]
fn the_archive_and_the_restored_private_key_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let archive = f.root.join("b.tar");
    backup::run_create(&f.config, &archive).unwrap();
    let dir = f.root.join("r");
    backup::run_restore(&archive, &dir).unwrap();
    for p in [archive, dir.join("identity/id_ed25519")] {
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} is {mode:o}", p.display());
    }
}

// --- helpers ------------------------------------------------------------------

fn read_manifest(dir: &Path) -> Manifest {
    Manifest::parse(&std::fs::read(dir.join(MANIFEST_PATH)).unwrap()).unwrap()
}

fn rebless(dir: &Path, path: &str) {
    let mut m = read_manifest(dir);
    let (sha, bytes) = crate::backup::hash_file(&dir.join(path)).unwrap();
    let f = m.files.iter_mut().find(|f| f.path == path).unwrap();
    f.sha256 = sha;
    f.bytes = bytes;
    std::fs::write(dir.join(MANIFEST_PATH), serde_json::to_vec(&m).unwrap()).unwrap();
}

fn read_entries(archive: &Path) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let mut tar = tar::Archive::new(std::fs::File::open(archive).unwrap());
    tar.entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let path = String::from_utf8(e.path_bytes().to_vec()).unwrap();
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).unwrap();
            (path, buf)
        })
        .collect()
}

/// Write entries with raw header names, so a `..` path is written as given
/// rather than refused by the tar crate's own path setter.
fn write_raw(path: &Path, entries: &[(String, Vec<u8>)]) {
    let mut b = tar::Builder::new(std::fs::File::create(path).unwrap());
    for (name, data) in entries {
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_size(data.len() as u64);
        h.set_mode(0o600);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, data.as_slice()).unwrap();
    }
    b.finish().unwrap();
}

/// The real entries, with the second one replaced by a symlink of the same name.
fn write_with_symlink(path: &Path, entries: &[(String, Vec<u8>)]) {
    let mut b = tar::Builder::new(std::fs::File::create(path).unwrap());
    for (i, (name, data)) in entries.iter().enumerate() {
        let mut h = tar::Header::new_gnu();
        if i == 1 {
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, name, "/etc/passwd").unwrap();
            continue;
        }
        h.set_size(data.len() as u64);
        h.set_mode(0o600);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, name, data.as_slice()).unwrap();
    }
    b.finish().unwrap();
}
