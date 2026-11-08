//! Demonstrate that WAL, set out of band, actually reaches cozo's connections.
//!
//! ADR-108's verification section requires this be *demonstrated, not asserted*,
//! before the in-code claims are corrected — the previous claim ("there is no
//! hook this crate could use to set the pragma") was reached from a sound
//! premise and a wrong conclusion, which is what an unverified fix looks like.

use super::*;

fn journal_mode(db: &std::path::Path) -> String {
    let conn = sqlite::Connection::open(db).expect("open for pragma read");
    let mut mode = String::new();
    conn.iterate("PRAGMA journal_mode;", |row| {
        if let Some((_, Some(v))) = row.first() {
            mode = v.to_ascii_lowercase();
        }
        true
    })
    .expect("pragma read");
    mode
}

/// The property: a store MAE creates is in WAL mode from its first open, and
/// stays there across a close/reopen through cozo.
///
/// This fails on revert — without `wal::ensure_wal`, cozo creates the file in
/// `delete` (rollback-journal) mode, where a writer's exclusive lock blocks
/// readers file-wide.
#[test]
fn a_fresh_store_is_created_in_wal_mode_and_stays_there() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("kb.sqlite");

    {
        let store = CozoKbStore::open_with_engine(&db, "sqlite").expect("create");
        store
            .insert_node(&Node::new("a", "A", NodeKind::Note, "body"))
            .expect("write on a fresh WAL store");
    }
    assert_eq!(
        journal_mode(&db),
        "wal",
        "a store MAE created must be in WAL mode from the first open"
    );

    // Reopen through cozo: WAL is a header property, so it must persist, and
    // reads/writes must still work through it.
    {
        let store = CozoKbStore::open_with_engine(&db, "sqlite").expect("reopen");
        let node = store.get_node("a").expect("read").expect("node present");
        assert_eq!(node.title, "A", "content must survive the mode change");
        store
            .insert_node(&Node::new("b", "B", NodeKind::Note, "body"))
            .expect("write after reopen");
    }
    assert_eq!(
        journal_mode(&db),
        "wal",
        "WAL must persist across a cozo close/reopen — that is the whole premise"
    );
}

/// `ensure_wal` must never fail an open. A store that cannot go into WAL (a
/// network filesystem, or another process holding the file) is the status quo:
/// slower under contention, but working. Refusing to open the KB would trade a
/// performance property for an availability one.
#[test]
fn a_store_that_cannot_take_wal_still_opens() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("kb.sqlite");

    // A path that is not a usable sqlite file: `ensure_wal` must shrug.
    std::fs::write(&db, b"this is not a database").unwrap();
    let mode = super::super::wal::ensure_wal(&db);
    assert!(
        mode.as_deref() != Some("wal"),
        "a non-database file cannot be in WAL mode, so this must not claim it is"
    );
}

/// `(frames in the WAL, frames checkpointed, frames a checkpoint last
/// attempted)` from the WAL-index header in the `-shm` file (SQLite's
/// documented layout: `mxFrame` at 16, `nBackfill` at 96,
/// `nBackfillAttempted` at 128; native byte order).
fn wal_index(db: &std::path::Path) -> (u32, u32, u32) {
    let b = std::fs::read(format!("{}-shm", db.display())).expect("shm");
    let u = |o: usize| u32::from_ne_bytes(b[o..o + 4].try_into().unwrap());
    (u(16), u(96), u(128))
}

fn wal_bytes(db: &std::path::Path) -> u64 {
    std::fs::metadata(format!("{}-wal", db.display()))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// The mechanism behind `wal::Checkpointer`, in plain SQLite: a statement
/// left mid-result when its connection commits (what cozo's `get` does) keeps
/// a read transaction open, and SQLite's automatic checkpoint on that
/// connection then never even attempts. Reset the statement first and the
/// same writes are checkpointed.
#[test]
fn a_statement_left_mid_result_at_commit_stops_the_automatic_checkpoint() {
    let run = |reset_first: bool| {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let conn = sqlite::Connection::open(&db).unwrap();
        conn.execute("PRAGMA journal_mode=WAL; CREATE TABLE t(k INTEGER PRIMARY KEY, v BLOB);")
            .unwrap();
        conn.execute("INSERT INTO t VALUES (0, zeroblob(10));")
            .unwrap();
        for round in 0..4 {
            conn.execute("BEGIN;").unwrap();
            let mut probe = conn.prepare("SELECT k FROM t WHERE k = 0;").unwrap();
            assert_eq!(probe.next().unwrap(), sqlite::State::Row);
            for i in 0..500 {
                conn.execute(format!(
                    "INSERT INTO t VALUES ({}, zeroblob(4000));",
                    1 + round * 500 + i
                ))
                .unwrap();
            }
            if reset_first {
                probe.reset().unwrap();
            }
            conn.execute("COMMIT;").unwrap();
            drop(probe);
        }
        wal_index(&db)
    };
    let (frames, _, attempted) = run(false);
    assert!(
        frames > 1000,
        "premise: past the 1,000-frame autocheckpoint ({frames})"
    );
    assert_eq!(
        attempted, 0,
        "a checkpoint was attempted despite the open statement"
    );
    let (_, backfilled, attempted) = run(true);
    assert!(
        attempted > 0 && backfilled > 0,
        "the autocheckpoint never ran ({attempted})"
    );
}

/// The fix: a store held open across thousands of writes keeps its WAL
/// bounded, because it checkpoints itself. Without `Checkpointer`, the WAL
/// holds every page ever written (here tens of MB).
#[test]
fn a_store_held_open_keeps_its_wal_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("kb.sqlite");
    let store = CozoKbStore::open(&db).unwrap();
    let body = "lorem ipsum dolor sit amet ".repeat(80);
    for round in 0..3 {
        for i in 0..1000 {
            let id = format!("n{i}");
            let node = crate::Node::new(&id, format!("T{i} {round}"), crate::NodeKind::Note, &body);
            store.insert_node(&node).unwrap();
        }
    }
    let (frames, backfilled, _) = wal_index(&db);
    let bytes = wal_bytes(&db);
    assert!(
        bytes < 8_000_000,
        "WAL grew to {bytes} bytes ({frames} frames, {backfilled} checkpointed) while the store was open"
    );
    // And an explicit checkpoint folds in whatever is left.
    let (busy, log, done) = store.checkpoint().expect("a sqlite store checkpoints");
    assert_eq!(
        (busy, log),
        (0, done),
        "a PASSIVE checkpoint with no reader left frames behind"
    );
}
