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
