//! KB Lifecycle E2E tests — SQLite persistence, CRDT integration, offline queue,
//! import/export, and performance.
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_KB_LIFECYCLE=1 cargo test -p mae --test kb_lifecycle_e2e -- --ignored --nocapture

use mae_kb::{KbStore, Node, NodeKind, SqliteKbStore};
use std::time::Instant;

fn should_run() -> bool {
    std::env::var("MAE_KB_LIFECYCLE").is_ok()
}

fn make_store() -> (tempfile::TempDir, SqliteKbStore) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test_kb.db");
    let store = SqliteKbStore::open(&path).unwrap();
    (tmp, store)
}

// ============================================================
// Category 1: SQLite Persistence
// ============================================================

#[test]
#[ignore]
fn test_node_create_persists_to_sqlite() {
    if !should_run() {
        return;
    }
    let (tmp, store) = make_store();
    let node = Node::new(
        "persist:1",
        "Persistent Node",
        NodeKind::Note,
        "Hello world",
    )
    .with_tags(["concept", "test"]);
    store.insert_node(&node).unwrap();

    // Verify node in store
    let loaded = store.get_node("persist:1").unwrap().unwrap();
    assert_eq!(loaded.title, "Persistent Node");
    assert_eq!(loaded.body, "Hello world");
    assert_eq!(loaded.tags, vec!["concept", "test"]);

    // Simulate restart: open a new store at the same path
    drop(store);
    let store2 = SqliteKbStore::open(tmp.path().join("test_kb.db")).unwrap();
    let reloaded = store2.get_node("persist:1").unwrap().unwrap();
    assert_eq!(reloaded.title, "Persistent Node");
    assert_eq!(reloaded.body, "Hello world");
}

#[test]
#[ignore]
fn test_node_update_persists_immediately() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let node = Node::new("upd:1", "Original", NodeKind::Note, "old body");
    store.insert_node(&node).unwrap();

    let updated = Node::new("upd:1", "Updated Title", NodeKind::Note, "new body");
    store.update_node(&updated).unwrap();

    let loaded = store.get_node("upd:1").unwrap().unwrap();
    assert_eq!(loaded.title, "Updated Title");
    assert_eq!(loaded.body, "new body");
}

#[test]
#[ignore]
fn test_fts5_updated_on_mutation() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "fts:1",
            "Quantum Entanglement",
            NodeKind::Concept,
            "Spooky action at a distance",
        ))
        .unwrap();

    // FTS finds "quantum"
    let hits = store.fts_search("quantum", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "fts:1");

    // Update body, removing "quantum" context but keeping it in title
    let updated = Node::new(
        "fts:1",
        "Classical Mechanics",
        NodeKind::Concept,
        "Newton's laws of motion",
    );
    store.update_node(&updated).unwrap();

    // FTS should no longer find "quantum"
    let hits = store.fts_search("quantum", 10).unwrap();
    assert_eq!(hits.len(), 0);

    // But should find "classical"
    let hits = store.fts_search("classical", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "fts:1");
}

#[test]
#[ignore]
fn test_sqlite_restart_recovery() {
    if !should_run() {
        return;
    }
    let (tmp, store) = make_store();
    // Insert 10 nodes
    for i in 0..10 {
        let node = Node::new(
            format!("recover:{i}"),
            format!("Node {i}"),
            NodeKind::Note,
            format!("Body {i}"),
        );
        store.insert_node(&node).unwrap();
    }

    // Reopen (simulates restart, not crash, but validates WAL recovery)
    drop(store);
    let store2 = SqliteKbStore::open(tmp.path().join("test_kb.db")).unwrap();
    let all = store2.load_all().unwrap();
    assert_eq!(all.len(), 10);
}

#[test]
#[ignore]
fn test_concurrent_sqlite_access() {
    if !should_run() {
        return;
    }
    let (tmp, _store) = make_store();
    let path = tmp.path().join("test_kb.db");

    // Spawn 4 threads writing different nodes simultaneously
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let p = path.clone();
            std::thread::spawn(move || {
                let s = SqliteKbStore::open(&p).unwrap();
                for i in 0..10 {
                    let id = format!("thread{t}:node{i}");
                    let node = Node::new(&id, format!("T{t}N{i}"), NodeKind::Note, "body");
                    s.insert_node(&node).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    // Verify all 40 nodes present
    let store = SqliteKbStore::open(&path).unwrap();
    let all = store.load_all().unwrap();
    assert_eq!(all.len(), 40);
}

#[test]
#[ignore]
fn test_delete_removes_fts_and_links() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "del:a",
            "Alpha",
            NodeKind::Note,
            "Links to [[del:b]]",
        ))
        .unwrap();
    store
        .insert_node(&Node::new("del:b", "Beta", NodeKind::Note, "target"))
        .unwrap();

    // Verify link exists
    let links = store.links_from("del:a").unwrap();
    assert_eq!(links.len(), 1);

    // Delete source
    store.delete_node("del:a").unwrap();

    // FTS should not find it
    let hits = store.fts_search("Alpha", 10).unwrap();
    assert_eq!(hits.len(), 0);

    // Links from deleted node should be gone
    let links = store.links_from("del:a").unwrap();
    assert_eq!(links.len(), 0);
}

// ============================================================
// Category 2: CRDT + SQLite Integration
// ============================================================

#[test]
#[ignore]
fn test_crdt_doc_column_round_trip() {
    if !should_run() {
        return;
    }
    let (tmp, store) = make_store();
    let mut node = Node::new("crdt:rt", "CRDT Round Trip", NodeKind::Note, "body");
    // Simulate CRDT doc bytes
    let crdt_bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 1, 2, 3, 4];
    node.crdt_doc = Some(crdt_bytes.clone());
    store.insert_node(&node).unwrap();

    // Read back
    let doc = store.get_crdt_doc("crdt:rt").unwrap();
    assert_eq!(doc, Some(crdt_bytes.clone()));

    // Survive restart
    drop(store);
    let store2 = SqliteKbStore::open(tmp.path().join("test_kb.db")).unwrap();
    let doc = store2.get_crdt_doc("crdt:rt").unwrap();
    assert_eq!(doc, Some(crdt_bytes));
}

#[test]
#[ignore]
fn test_update_crdt_doc_preserves_node() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let node = Node::new("crdt:upd", "CRDT Update", NodeKind::Note, "original body");
    store.insert_node(&node).unwrap();

    // Update just CRDT doc
    store.update_crdt_doc("crdt:upd", &[10, 20, 30]).unwrap();

    // Node text columns should be unchanged
    let loaded = store.get_node("crdt:upd").unwrap().unwrap();
    assert_eq!(loaded.title, "CRDT Update");
    assert_eq!(loaded.body, "original body");
    // crdt_doc column was updated in-place
    assert_eq!(loaded.crdt_doc, Some(vec![10, 20, 30]));

    // get_crdt_doc also returns updated bytes
    let doc = store.get_crdt_doc("crdt:upd").unwrap();
    assert_eq!(doc, Some(vec![10, 20, 30]));
}

#[test]
#[ignore]
fn test_local_node_has_null_crdt() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let node = Node::new("local:1", "Local Only", NodeKind::Note, "not shared");
    store.insert_node(&node).unwrap();

    let doc = store.get_crdt_doc("local:1").unwrap();
    assert!(doc.is_none(), "Local nodes should have NULL crdt_doc");
}

// ============================================================
// Category 3: Offline Queue in SQLite
// ============================================================

#[test]
#[ignore]
fn test_offline_edits_persist_to_pending_table() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();

    for i in 0..5 {
        store
            .push_pending_update("kb-main", &format!("node-{i}"), &[i as u8, i as u8 + 1])
            .unwrap();
    }

    let pending = store.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 5);
    assert_eq!(pending[0].kb_id, "kb-main");
    assert_eq!(pending[0].node_id, "node-0");
}

#[test]
#[ignore]
fn test_pending_survives_restart() {
    if !should_run() {
        return;
    }
    let (tmp, store) = make_store();
    store.push_pending_update("kb-1", "n1", &[1, 2, 3]).unwrap();
    store.push_pending_update("kb-1", "n2", &[4, 5, 6]).unwrap();

    // Simulate restart
    drop(store);
    let store2 = SqliteKbStore::open(tmp.path().join("test_kb.db")).unwrap();
    let pending = store2.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].node_id, "n1");
    assert_eq!(pending[1].node_id, "n2");
}

#[test]
#[ignore]
fn test_ack_removes_pending() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    for i in 0..10 {
        store
            .push_pending_update("kb-1", &format!("n{i}"), &[i as u8])
            .unwrap();
    }

    let pending = store.drain_pending_updates().unwrap();
    assert_eq!(pending.len(), 10);

    // Ack first 7
    for p in &pending[..7] {
        store.ack_pending_update(p.rowid).unwrap();
    }

    let remaining = store.drain_pending_updates().unwrap();
    assert_eq!(remaining.len(), 3);
    assert_eq!(remaining[0].node_id, "n7");
}

#[test]
#[ignore]
fn test_pending_ordering_is_fifo() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    store.push_pending_update("kb-1", "first", &[1]).unwrap();
    store.push_pending_update("kb-1", "second", &[2]).unwrap();
    store.push_pending_update("kb-1", "third", &[3]).unwrap();

    let pending = store.drain_pending_updates().unwrap();
    assert_eq!(pending[0].node_id, "first");
    assert_eq!(pending[1].node_id, "second");
    assert_eq!(pending[2].node_id, "third");
}

#[test]
#[ignore]
fn test_drain_is_nondestructive() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    store.push_pending_update("kb-1", "durable", &[42]).unwrap();

    // Drain twice without ack — both should return the entry
    let first = store.drain_pending_updates().unwrap();
    let second = store.drain_pending_updates().unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].node_id, second[0].node_id);
}

// ============================================================
// Category 4: Import/Export Lifecycle
// ============================================================

#[test]
#[ignore]
fn test_save_all_and_load_all() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let nodes: Vec<Node> = (0..10)
        .map(|i| {
            Node::new(
                format!("bulk:{i}"),
                format!("Bulk Node {i}"),
                NodeKind::Note,
                format!("Body content {i}"),
            )
            .with_tags([format!("tag{}", i % 3)])
        })
        .collect();

    let refs: Vec<&Node> = nodes.iter().collect();
    store.save_all(&refs).unwrap();

    let loaded = store.load_all().unwrap();
    assert_eq!(loaded.len(), 10);

    // Verify content round-trips
    for (i, node) in loaded.iter().enumerate() {
        assert_eq!(node.title, format!("Bulk Node {i}"));
        assert_eq!(node.body, format!("Body content {i}"));
    }
}

#[test]
#[ignore]
fn test_save_all_replaces_existing() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let n1 = Node::new("old:1", "Old Node", NodeKind::Note, "old");
    store.insert_node(&n1).unwrap();

    // save_all with different nodes should replace everything
    let n2 = Node::new("new:1", "New Node", NodeKind::Note, "new");
    store.save_all(&[&n2]).unwrap();

    assert!(store.get_node("old:1").unwrap().is_none());
    assert!(store.get_node("new:1").unwrap().is_some());
}

#[test]
#[ignore]
fn test_incremental_upsert_preserves_crdt() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let mut node = Node::new("upsert:1", "Original", NodeKind::Note, "body");
    node.crdt_doc = Some(vec![0xDE, 0xAD]);
    store.insert_node(&node).unwrap();

    // Upsert with updated title — crdt_doc carried through if included in node
    let mut updated = Node::new("upsert:1", "Updated", NodeKind::Note, "new body");
    updated.crdt_doc = Some(vec![0xDE, 0xAD]); // preserve crdt
    store.update_node(&updated).unwrap();

    let loaded = store.get_node("upsert:1").unwrap().unwrap();
    assert_eq!(loaded.title, "Updated");
    assert_eq!(loaded.crdt_doc, Some(vec![0xDE, 0xAD]));
}

#[test]
#[ignore]
fn test_links_updated_on_body_change() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "link:src",
            "Source",
            NodeKind::Note,
            "Links to [[link:a]] and [[link:b]]",
        ))
        .unwrap();

    let links = store.links_from("link:src").unwrap();
    assert_eq!(links.len(), 2);

    // Update body, change links
    store
        .update_node(&Node::new(
            "link:src",
            "Source",
            NodeKind::Note,
            "Now only [[link:c]]",
        ))
        .unwrap();

    let links = store.links_from("link:src").unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].dst, "link:c");
}

// ============================================================
// Category 5: Scale & Performance
// ============================================================

#[test]
#[ignore]
fn test_startup_1000_nodes_under_500ms() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();

    // Pre-populate
    let nodes: Vec<Node> = (0..1000)
        .map(|i| {
            Node::new(
                format!("perf:{i:04}"),
                format!("Performance Test Node {i}"),
                NodeKind::Note,
                format!("This is the body of node {i}. It contains enough text to be realistic for a knowledge base entry. Keywords: testing, performance, {i}."),
            )
            .with_tags([format!("group{}", i % 10)])
        })
        .collect();

    let refs: Vec<&Node> = nodes.iter().collect();
    store.save_all(&refs).unwrap();

    // Time the load
    let start = Instant::now();
    let loaded = store.load_all().unwrap();
    let elapsed = start.elapsed();

    assert_eq!(loaded.len(), 1000);
    assert!(
        elapsed.as_millis() < 500,
        "load_all took {}ms, expected <500ms",
        elapsed.as_millis()
    );
}

#[test]
#[ignore]
fn test_fts_search_1000_nodes_under_10ms() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();

    let nodes: Vec<Node> = (0..1000)
        .map(|i| {
            Node::new(
                format!("perf:{i:04}"),
                format!("Node {i}"),
                NodeKind::Note,
                format!("Body with unique keyword_{i} and common text about knowledge management systems."),
            )
        })
        .collect();

    let refs: Vec<&Node> = nodes.iter().collect();
    store.save_all(&refs).unwrap();

    let start = Instant::now();
    let hits = store.fts_search("knowledge management", 20).unwrap();
    let elapsed = start.elapsed();

    assert!(!hits.is_empty(), "FTS should find matches");
    assert!(
        elapsed.as_millis() < 10,
        "FTS search took {}ms, expected <10ms",
        elapsed.as_millis()
    );
}

#[test]
#[ignore]
fn test_insert_1000_nodes_under_5s() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();

    let start = Instant::now();
    for i in 0..1000 {
        let node = Node::new(
            format!("ins:{i:04}"),
            format!("Inserted Node {i}"),
            NodeKind::Note,
            format!("Body {i} with content."),
        );
        store.insert_node(&node).unwrap();
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_secs() < 5,
        "1000 inserts took {}s, expected <5s",
        elapsed.as_secs()
    );

    let all = store.load_all().unwrap();
    assert_eq!(all.len(), 1000);
}

// ============================================================
// Category 6: Edge Cases
// ============================================================

#[test]
#[ignore]
fn test_empty_store_operations() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();

    assert!(store.get_node("nonexistent").unwrap().is_none());
    assert!(store.load_all().unwrap().is_empty());
    assert!(store.list_ids(None).unwrap().is_empty());
    assert!(store.fts_search("anything", 10).unwrap().is_empty());
    assert!(store.links_from("x").unwrap().is_empty());
    assert!(store.links_to("x").unwrap().is_empty());
    assert!(store.drain_pending_updates().unwrap().is_empty());
    assert!(store.get_crdt_doc("x").unwrap().is_none());
}

#[test]
#[ignore]
fn test_node_with_all_fields() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    let mut node = Node::new("full:1", "Full Node", NodeKind::Concept, "rich body")
        .with_tags(["tag1", "tag2", "tag3"])
        .with_aliases(["alias-one", "alias-two"])
        .with_properties(std::collections::HashMap::from([(
            "key".to_string(),
            "value".to_string(),
        )]));
    node.todo_state = Some("TODO".to_string());
    node.priority = Some('A');
    node.source = Some(mae_kb::NodeSource::Manual);
    node.source_version = Some(42);
    node.crdt_doc = Some(vec![1, 2, 3, 4, 5]);

    store.insert_node(&node).unwrap();
    let loaded = store.get_node("full:1").unwrap().unwrap();

    assert_eq!(loaded.title, "Full Node");
    assert_eq!(loaded.kind, NodeKind::Concept);
    assert_eq!(loaded.body, "rich body");
    assert_eq!(loaded.tags, vec!["tag1", "tag2", "tag3"]);
    assert_eq!(loaded.aliases, vec!["alias-one", "alias-two"]);
    assert_eq!(loaded.properties.get("key"), Some(&"value".to_string()));
    assert_eq!(loaded.todo_state, Some("TODO".to_string()));
    assert_eq!(loaded.priority, Some('A'));
    assert_eq!(loaded.source_version, Some(42));
    assert_eq!(loaded.crdt_doc, Some(vec![1, 2, 3, 4, 5]));
}

#[test]
#[ignore]
fn test_backend_name() {
    if !should_run() {
        return;
    }
    let (_tmp, store) = make_store();
    assert_eq!(store.backend_name(), "sqlite");
}
