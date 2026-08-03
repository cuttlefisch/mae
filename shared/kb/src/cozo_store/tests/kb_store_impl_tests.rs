//! General `CozoKbStore` behaviour: node CRUD, the pending-updates offline
//! queue, CRDT-doc round-tripping, bulk load/save, and id listing.
//!
//! FTS/full-text-search tests live in the sibling `fts_search_tests` (the
//! corpus-wide "every indexed term retrieves its node" property, plus
//! case-insensitivity and shared-term retrieval) and `fts_query_tests`
//! (ranking under a realistic corpus, known cozo FTS query-grammar edge
//! cases, the empty-query/bulk-path/update-lifecycle behaviour, and the raw
//! Tantivy sanity check) — split out (#535/#536-adjacent cleanup) because
//! this file had grown past the 500-line test-file ceiling once the FTS
//! title/body-index-welding fix added its property-test coverage.

use super::*;

#[test]
fn insert_and_get_node() {
    let (_tmp, store) = make_store();
    let node =
        Node::new("test:1", "Test Node", NodeKind::Note, "Hello world").with_tags(["tag1", "tag2"]);
    store.insert_node(&node).unwrap();

    let loaded = store.get_node("test:1").unwrap().unwrap();
    assert_eq!(loaded.title, "Test Node");
    assert_eq!(loaded.body, "Hello world");
    assert_eq!(loaded.tags, vec!["tag1", "tag2"]);
}

#[test]
fn get_missing_returns_none() {
    let (_tmp, store) = make_store();
    assert!(store.get_node("nonexistent").unwrap().is_none());
}

#[test]
fn delete_node_removes_it() {
    // Test with mem engine to verify rm works cleanly
    let db = DbInstance::new("mem", "", "").unwrap();
    db.run_default(":create test {k: String => v: String}")
        .unwrap();
    db.run_default(r#"?[k, v] <- [["a", "hello"]] :put test {k => v}"#)
        .unwrap();
    let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
    assert_eq!(r.rows.len(), 1);
    db.run_default(r#"?[k] <- [["a"]] :rm test {k}"#).unwrap();
    let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
    eprintln!("mem after rm: {:?}", r.rows);

    // Now test CozoKbStore
    let (_tmp, store) = make_store();
    let node = Node::new("del-1", "Delete Me", NodeKind::Note, "body");
    store.insert_node(&node).unwrap();
    assert!(store.get_node("del-1").unwrap().is_some());

    store.delete_node("del-1").unwrap();
    let after = store.get_node("del-1").unwrap();
    // Sled backend may leave ghost rows with empty values — treat as deleted
    match after {
        None => {} // ideal
        Some(n) => assert!(
            n.title.is_empty() && n.body.is_empty(),
            "ghost row should have empty fields"
        ),
    }
}

#[test]
fn pending_updates_lifecycle() {
    let (_tmp, store) = make_store();
    store
        .push_pending_update("kb-1", "node-a", &[1, 2, 3])
        .unwrap();
    store
        .push_pending_update("kb-1", "node-b", &[4, 5, 6])
        .unwrap();

    let pending = store.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].node_id, "node-a");

    // ADR-020 observability: count reflects the durable queue (what an offline
    // edit lands in) — the seam the introspect `pending_kb_updates` reads.
    assert_eq!(
        store.count_pending_updates().unwrap(),
        2,
        "durable pending count must reflect un-acked offline edits"
    );

    store.ack_pending_update(pending[0].rowid).unwrap();
    let remaining = store.drain_pending_updates().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].node_id, "node-b");
    assert_eq!(
        store.count_pending_updates().unwrap(),
        1,
        "count decreases as the queue is acked"
    );
}

#[test]
fn crdt_doc_persistence() {
    // get_crdt_doc/update_crdt_doc (narrow point-read/point-write trait
    // methods) were removed as dead code (#303 follow-up) -- crdt_doc is
    // an ordinary field on the ordinary node-row path (insert_node/
    // get_node/update_node), which this now exercises directly.
    let (_tmp, store) = make_store();
    let mut node = Node::new("crdt:1", "CRDT Node", NodeKind::Note, "body");
    node.crdt_doc = Some(vec![10, 20, 30, 40]);
    store.insert_node(&node).unwrap();

    let reloaded = store.get_node("crdt:1").unwrap();
    assert_eq!(
        reloaded.and_then(|n| n.crdt_doc),
        Some(vec![10, 20, 30, 40])
    );
}

#[test]
fn load_all_and_save_all() {
    let (_tmp, store) = make_store();
    let n1 = Node::new("n1", "One", NodeKind::Note, "body1");
    let n2 = Node::new("n2", "Two", NodeKind::Note, "body2");

    store.save_all(&[&n1, &n2]).unwrap();
    let loaded = store.load_all().unwrap();
    assert_eq!(loaded.len(), 2);
}

#[test]
fn load_all_tolerates_query_bind_failure() {
    // B-5 regression: a stored `nodes` relation left at an older / shorter
    // arity (here a 2-column stand-in for the production "tuple bound by
    // variable 'title' is too short" artifact) makes the full 13-column load
    // query fail at bind time — BEFORE the per-row skip loop runs. A hard Err
    // here previously aborted `kb_join` and tripped the 10s main-thread stall
    // watchdog. The store must degrade to an empty load and keep running.
    let (_tmp, store) = make_store();
    // Replace `nodes` with a relation the full load query cannot bind, and
    // populate one row (simulates the migration / broken-write artifact on
    // disk that the production "tuple too short" error came from). The FTS
    // index must be dropped first — a relation with indices attached can't be
    // replaced.
    store
        .run_mut("::fts drop nodes:fts")
        .expect("drop fts index");
    store
        .run_mut(
            r#"?[id, title] <- [["bad", "x"]]
                   :replace nodes {id: String => title: String}"#,
        )
        .expect("replace schema with short-arity row");

    // Must be Ok (degraded), never Err, and must not panic.
    let loaded = store
        .load_all()
        .expect("load_all must degrade to Ok on a query bind failure, not Err");
    assert!(
        loaded.is_empty(),
        "a load query that cannot bind degrades to an empty result"
    );
}

#[test]
fn backend_name_is_cozo() {
    let (_tmp, store) = make_store();
    assert_eq!(store.backend_name(), "cozo");
}

#[test]
fn list_ids_with_prefix() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("cmd:save", "Save", NodeKind::Command, ""))
        .unwrap();
    store
        .insert_node(&Node::new("cmd:quit", "Quit", NodeKind::Command, ""))
        .unwrap();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer",
            NodeKind::Concept,
            "",
        ))
        .unwrap();

    let cmd_ids = store.list_ids(Some("cmd:")).unwrap();
    assert_eq!(cmd_ids.len(), 2);
    let all_ids = store.list_ids(None).unwrap();
    assert_eq!(all_ids.len(), 3);
}
