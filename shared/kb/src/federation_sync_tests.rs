//! `IngestMode::Sync`: the daemon watcher's mode after its first `Full`.
//!
//! The claim it must keep is "what `Full` would produce, at an incremental
//! pass's cost", so the oracle is `Full` itself: after any mix of edits,
//! additions, deletions and renames, a store brought forward by `Sync` holds
//! exactly what a fresh `Full` of the same directory holds. And the cost
//! claim is checked where it matters: an unchanged directory commits nothing.

use super::*;
use crate::store::KbStore as _;
use tempfile::TempDir;

fn note(id: &str, body: &str) -> String {
    format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\n{body}\n")
}

fn ingest(dir: &Path, store: &crate::CozoKbStore, mode: IngestMode) -> ImportReport {
    import_org_dir_to_store(dir, store, &mode)
        .expect("ingest")
        .1
}

/// Every node as `(id, title, body)`, sorted: what a reader of the store sees.
fn contents(store: &crate::CozoKbStore) -> Vec<(String, String, String)> {
    let mut out: Vec<_> = store
        .load_all()
        .unwrap()
        .into_iter()
        .map(|n| (n.id, n.title, n.body))
        .collect();
    out.sort();
    out
}

fn sqlite_store(dir: &TempDir) -> (crate::CozoKbStore, std::path::PathBuf) {
    let path = dir.path().join("kb.sqlite");
    (crate::CozoKbStore::open(&path).unwrap(), path)
}

#[test]
fn sync_ends_where_a_fresh_full_ingest_would() {
    let org = TempDir::new().unwrap();
    let p = |f: &str| org.path().join(f);
    std::fs::write(p("edit.org"), note("n:edit", "old")).unwrap();
    std::fs::write(p("keep.org"), note("n:keep", "same")).unwrap();
    std::fs::write(p("gone.org"), note("n:gone", "bye")).unwrap();
    std::fs::write(p("old-name.org"), note("n:moved", "moves")).unwrap();
    std::fs::write(
        p("leaves.org"),
        format!("{}{}", note("n:stays-here", "one"), note("n:leaves", "two")),
    )
    .unwrap();
    let store = crate::CozoKbStore::open_mem().unwrap();
    ingest(org.path(), &store, IngestMode::Full);

    std::fs::write(p("edit.org"), note("n:edit", "new")).unwrap();
    std::fs::remove_file(p("gone.org")).unwrap();
    std::fs::write(p("added.org"), note("n:added", "hi")).unwrap();
    std::fs::rename(p("old-name.org"), p("new-name.org")).unwrap();
    // A node moves out of one file (which stays) into a new one.
    std::fs::write(p("leaves.org"), note("n:stays-here", "one")).unwrap();
    std::fs::write(p("arrived.org"), note("n:leaves", "two")).unwrap();
    let report = ingest(org.path(), &store, IngestMode::Sync);

    let fresh = crate::CozoKbStore::open_mem().unwrap();
    ingest(org.path(), &fresh, IngestMode::Full);
    assert_eq!(contents(&store), contents(&fresh));
    assert!(contents(&store)
        .iter()
        .any(|(id, _, b)| id == "n:edit" && b.contains("new")));
    assert_eq!(
        report.nodes_removed, 1,
        "only n:gone is removed: {report:?}"
    );
}

/// Incremental keeps a deleted file's nodes, Sync removes them: the one
/// difference between the two, pinned so neither drifts into the other.
#[test]
fn sync_removes_a_deleted_files_nodes_and_incremental_still_does_not() {
    for (mode, kept) in [(IngestMode::Incremental, true), (IngestMode::Sync, false)] {
        let org = TempDir::new().unwrap();
        std::fs::write(org.path().join("gone.org"), note("n:gone", "x")).unwrap();
        std::fs::write(org.path().join("stay.org"), note("n:stay", "y")).unwrap();
        let store = crate::CozoKbStore::open_mem().unwrap();
        ingest(org.path(), &store, IngestMode::Full);
        std::fs::remove_file(org.path().join("gone.org")).unwrap();
        ingest(org.path(), &store, mode.clone());
        assert_eq!(
            store.get_node("n:gone").unwrap().is_some(),
            kept,
            "{mode:?}"
        );
        assert!(store.get_node("n:stay").unwrap().is_some(), "{mode:?}");
    }
}

/// Sync deletes, so a directory that is gone (moved, unmounted) must be
/// refused as Full's is, not read as "every file was deleted".
#[test]
fn sync_of_a_missing_directory_is_refused_and_deletes_nothing() {
    let org = TempDir::new().unwrap();
    std::fs::write(org.path().join("a.org"), note("n:a", "x")).unwrap();
    let store = crate::CozoKbStore::open_mem().unwrap();
    ingest(org.path(), &store, IngestMode::Full);
    let gone = org.path().to_path_buf();
    drop(org);
    assert!(import_org_dir_to_store(&gone, &store, &IngestMode::Sync).is_err());
    assert!(store.get_node("n:a").unwrap().is_some());
}

/// The cost claim, measured by SQLite itself: `PRAGMA data_version` (read on
/// a separate connection) changes only when another connection commits. A
/// Sync over an unchanged directory commits nothing; a Full of the same
/// directory rewrites it (the write amplification Sync exists to avoid).
#[test]
fn sync_of_an_unchanged_directory_commits_nothing() {
    let org = TempDir::new().unwrap();
    for i in 0..20 {
        std::fs::write(
            org.path().join(format!("n{i}.org")),
            note(&format!("n:{i}"), "x"),
        )
        .unwrap();
    }
    let db_dir = TempDir::new().unwrap();
    let (store, path) = sqlite_store(&db_dir);
    ingest(org.path(), &store, IngestMode::Full);

    let observer = sqlite::Connection::open(&path).unwrap();
    let version = || {
        let mut v = 0i64;
        observer
            .iterate("PRAGMA data_version;", |row| {
                v = row[0].1.unwrap().parse().unwrap();
                true
            })
            .unwrap();
        v
    };
    let before = version();
    // Rows carry a whole-second timestamp, and SQLite skips rewriting a row
    // whose bytes are identical: without moving the clock, a Sync that wrongly
    // re-wrote everything would still look like it committed nothing.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ingest(org.path(), &store, IngestMode::Sync);
    assert_eq!(
        version(),
        before,
        "a Sync of an unchanged directory wrote to the store"
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    ingest(org.path(), &store, IngestMode::Full);
    assert_ne!(
        version(),
        before,
        "premise: a Full of the same directory does write"
    );
}

/// A node split out of a file that still exists, into a new file the walk
/// reaches first, survives a reimport in every mode. The old file's ingest
/// used to retract the id it no longer holds, deleting a node that is still
/// on disk — under `Full` until the next pass, under `Sync` indefinitely.
#[test]
fn a_node_moved_into_a_file_walked_earlier_survives_a_reimport() {
    for mode in [IngestMode::Full, IngestMode::Sync, IngestMode::Incremental] {
        let org = TempDir::new().unwrap();
        let p = |f: &str| org.path().join(f);
        let both = format!("{}{}", note("n:stays-here", "one"), note("n:leaves", "two"));
        std::fs::write(p("leaves.org"), both).unwrap();
        let store = crate::CozoKbStore::open_mem().unwrap();
        ingest(org.path(), &store, IngestMode::Full);
        std::fs::write(p("leaves.org"), note("n:stays-here", "one")).unwrap();
        std::fs::write(p("arrived.org"), note("n:leaves", "two")).unwrap();
        ingest(org.path(), &store, mode.clone());
        assert!(
            store.get_node("n:leaves").unwrap().is_some(),
            "{mode:?} lost the moved node"
        );
    }
}

/// Control: retraction still works. An `:ID:` changed in place removes the
/// old id when no other file claims it.
#[test]
fn an_id_no_file_claims_any_more_is_still_retracted() {
    for mode in [IngestMode::Full, IngestMode::Sync] {
        let org = TempDir::new().unwrap();
        std::fs::write(org.path().join("a.org"), note("n:old", "x")).unwrap();
        let store = crate::CozoKbStore::open_mem().unwrap();
        ingest(org.path(), &store, IngestMode::Full);
        std::fs::write(org.path().join("a.org"), note("n:new", "x")).unwrap();
        ingest(org.path(), &store, mode.clone());
        assert!(
            store.get_node("n:old").unwrap().is_none(),
            "{mode:?} kept a retracted id"
        );
        assert!(store.get_node("n:new").unwrap().is_some(), "{mode:?}");
    }
}

/// A node copied into a second file, picked up by an `Incremental` pass (so
/// both files' records list it), survives `Sync` after the original file is
/// deleted: the copy is unchanged, so `Sync` does not re-parse it, and must
/// count the ids its record lists as still on disk before sweeping.
#[test]
fn sync_keeps_a_node_an_unchanged_file_still_declares_when_another_is_deleted() {
    let org = TempDir::new().unwrap();
    std::fs::write(org.path().join("a.org"), note("n:copied", "text")).unwrap();
    let store = crate::CozoKbStore::open_mem().unwrap();
    ingest(org.path(), &store, IngestMode::Full);
    std::fs::write(org.path().join("b.org"), note("n:copied", "text")).unwrap();
    ingest(org.path(), &store, IngestMode::Incremental);
    std::fs::remove_file(org.path().join("a.org")).unwrap();
    ingest(org.path(), &store, IngestMode::Sync);
    assert!(
        store.get_node("n:copied").unwrap().is_some(),
        "Sync removed a node b.org declares"
    );
}
