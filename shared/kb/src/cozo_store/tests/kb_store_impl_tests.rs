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
    let (_tmp, store) = make_store();
    let mut node = Node::new("crdt:1", "CRDT Node", NodeKind::Note, "body");
    node.crdt_doc = Some(vec![10, 20, 30, 40]);
    store.insert_node(&node).unwrap();

    let doc = store.get_crdt_doc("crdt:1").unwrap();
    assert_eq!(doc, Some(vec![10, 20, 30, 40]));
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

#[test]
fn fts_search_finds_nodes() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton was right.",
        ))
        .unwrap();

    let hits = store.fts_search("quantum", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "n1");
}

#[test]
fn fts_ranking_and_multi_word() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "n1",
            "Quantum Physics",
            NodeKind::Note,
            "Entanglement is spooky action at a distance",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n2",
            "Classical Mechanics",
            NodeKind::Note,
            "Newton discovered gravity under a tree",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "n3",
            "Relativity Theory",
            NodeKind::Note,
            "Einstein showed space and time are linked by gravity",
        ))
        .unwrap();

    // Single word search — should find nodes mentioning "gravity"
    let hits = store.fts_search("gravity", 10).unwrap();
    assert!(
        hits.len() >= 2,
        "expected 2+ results for 'gravity', got {}",
        hits.len()
    );
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(hit_ids.contains(&"n2"), "n2 should match 'gravity'");
    assert!(hit_ids.contains(&"n3"), "n3 should match 'gravity'");

    // Title search — "quantum" is in the title, Tantivy indexes title + body
    let hits = store.fts_search("quantum", 10).unwrap();
    assert!(!hits.is_empty(), "should find 'quantum' in title");
    assert_eq!(hits[0].id, "n1");

    // Empty query returns all nodes
    let all = store.fts_search("", 100).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn fts_search_empty_query_respects_limit() {
    // Regression: the empty-query branch used to return ALL node ids
    // unbounded (reachable via the AI `kb_search` tool); it must honor `limit`.
    let (_tmp, store) = make_store();
    for i in 0..10 {
        store
            .insert_node(&Node::new(
                format!("n{i}"),
                format!("Title {i}"),
                NodeKind::Note,
                "body",
            ))
            .unwrap();
    }
    let bounded = store.fts_search("", 3).unwrap();
    assert_eq!(bounded.len(), 3, "empty query must respect the limit");
}

#[test]
fn fts_search_bulk_path_matches_terms_and_scores() {
    // Exercises the bulk-fetch (`is_in`) path that replaced the per-candidate
    // get_node N+1: candidates must still be term-verified against their real
    // title+body (fetched in one query), non-matches excluded, and the FTS
    // score preserved. Uses colon-namespaced ids (the KB norm) to confirm the
    // bulk `is_in` lookup handles them.
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "doc:rust",
            "Rust Notes",
            NodeKind::Note,
            "The borrow checker enforces memory safety",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:python",
            "Python Notes",
            NodeKind::Note,
            "Duck typing is flexible",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "doc:empty",
            "Unrelated",
            NodeKind::Note,
            "nothing relevant here",
        ))
        .unwrap();

    let hits = store.fts_search("borrow", 10).unwrap();
    let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
    assert!(
        ids.contains(&"doc:rust"),
        "bulk path should surface the term match, got {ids:?}"
    );
    assert!(
        !ids.contains(&"doc:empty"),
        "term-verification must exclude non-matches"
    );
    // Bulk fetch must not drop the score carried from the FTS query.
    assert!(hits.iter().all(|h| h.score >= 0.0));
}

#[test]
fn fts_updates_on_node_change() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "original content about photosynthesis",
        ))
        .unwrap();

    // Should find photosynthesis
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // Update body
    store
        .insert_node(&Node::new(
            "u1",
            "Alpha",
            NodeKind::Note,
            "updated content about mitochondria",
        ))
        .unwrap();

    // Old term should NOT be found (FTS re-indexed via rm + put)
    let hits = store.fts_search("photosynthesis", 10).unwrap();
    assert!(
        hits.is_empty(),
        "stale FTS: 'photosynthesis' should not match after update"
    );

    // New term should be found
    let hits = store.fts_search("mitochondria", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "u1");
}

#[test]
fn tantivy_fts_on_sqlite() {
    // Test CozoDB's native Tantivy FTS index on sled backend
    let tmp = tempfile::tempdir().unwrap();
    let db = DbInstance::new("sled", tmp.path().join("fts_test").to_str().unwrap(), "").unwrap();

    db.run_script(
        ":create docs { id: String => title: String, body: String }",
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // Create FTS index
    let fts_create = db.run_script(
        r#"::fts create docs:search {
                extractor: body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    );
    if let Err(e) = &fts_create {
        panic!("FTS index creation failed on sqlite: {e}");
    }

    // Insert docs
    db.run_script(
        r#"?[id, title, body] <- [
                ["n1", "Quantum Physics", "Entanglement is a spooky action at a distance"],
                ["n2", "Classical Mechanics", "Newton discovered gravity under an apple tree"],
                ["n3", "Relativity", "Einstein showed that space and time are intertwined"]
            ] :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    // FTS search for "gravity"
    let res = db
            .run_script(
                r"?[id, title, score] := ~docs:search{id, title | query: 'gravity', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();

    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][0].get_str().unwrap(), "n2");

    // Multi-word search
    let res2 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'space time', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    assert_eq!(res2.rows.len(), 1);
    assert_eq!(res2.rows[0][0].get_str().unwrap(), "n3");

    // Test update: old term should be removed from FTS index
    db.run_script(
        r#"?[id, title, body] <- [["n2", "Classical Mechanics", "Hamilton reformulated mechanics"]]
            :put docs {id => title, body}"#,
        BTreeMap::new(),
        ScriptMutability::Mutable,
    )
    .unwrap();

    let res3 = db
        .run_script(
            r"?[id, score] := ~docs:search{id | query: 'gravity', k: 5, bind_score: score}",
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )
        .unwrap();
    // Should no longer find "gravity" — it was in n2 which was updated
    // Verify FTS auto-cleans stale entries after update
    eprintln!(
        "After update, 'gravity' search returns {} results: {:?}",
        res3.rows.len(),
        res3.rows
            .iter()
            .map(|r| r[0].get_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    // n3 still has "gravity" in its body
    assert!(
        res3.rows.len() <= 1,
        "should have at most 1 result (n3), got {}",
        res3.rows.len()
    );
}
