use super::*;

#[test]
fn record_source_file_retracts_ids_dropped_from_a_still_existing_path() {
    // Full-directory `:kb-reimport`'s counterpart to the watcher-path fix:
    // a file's id-set can change (in-place `:ID:` rename) without the file
    // ever being deleted, so deletion-detection keyed on "path vanished"
    // (federation.rs) never fires — record_source_file itself must diff
    // old vs. new ids and retract the difference.
    let (_tmp, store) = make_store();
    let old_node = Node::new("user:t-jenkinsp", "jenkinsp", NodeKind::Note, "Jenkins");
    store.insert_node(&old_node).unwrap();
    store
        .record_source_file("jenkinsp.org", "hash1", 1, &["user:t-jenkinsp".to_string()])
        .unwrap();
    assert!(store.get_node("user:t-jenkinsp").unwrap().is_some());

    // Same path, renamed id — as if the file's :ID: was hand-edited and the
    // directory got re-walked without the path itself ever changing.
    let new_node = Node::new("user:t-jenkins", "jenkins", NodeKind::Note, "Jenkins");
    store.insert_node(&new_node).unwrap();
    store
        .record_source_file("jenkinsp.org", "hash2", 2, &["user:t-jenkins".to_string()])
        .unwrap();

    assert!(
        store.get_node("user:t-jenkinsp").unwrap().is_none(),
        "the old id must be retracted once the file no longer produces it"
    );
    assert!(
        store.get_node("user:t-jenkins").unwrap().is_some(),
        "the current id must remain"
    );
    assert_eq!(
        store.get_source_file_node_ids("jenkinsp.org").unwrap(),
        vec!["user:t-jenkins".to_string()]
    );
}

#[test]
fn source_file_by_node_id_maps_every_tracked_id_back_to_its_path() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a:1", "A", NodeKind::Note, "body"))
        .unwrap();
    store
        .insert_node(&Node::new("a:2", "A2", NodeKind::Note, "body"))
        .unwrap();
    store
        .insert_node(&Node::new("b:1", "B", NodeKind::Note, "body"))
        .unwrap();
    store
        .record_source_file(
            "a.org",
            "hash-a",
            1,
            &["a:1".to_string(), "a:2".to_string()],
        )
        .unwrap();
    store
        .record_source_file("b.org", "hash-b", 1, &["b:1".to_string()])
        .unwrap();

    let index = store.source_file_by_node_id().unwrap();
    assert_eq!(index.get("a:1").unwrap(), &PathBuf::from("a.org"));
    assert_eq!(index.get("a:2").unwrap(), &PathBuf::from("a.org"));
    assert_eq!(index.get("b:1").unwrap(), &PathBuf::from("b.org"));
    assert_eq!(index.len(), 3);
}

#[test]
fn load_all_reconstructs_source_file_from_the_source_files_table() {
    // Regression guard: `row_to_node` never sets `source_file` — the `nodes`
    // relation has no such column. Before this fix, EVERY node reloaded from
    // a persisted CozoKbStore (the normal path on every editor relaunch
    // after the first import — see `bootstrap.rs`) had `source_file: None`
    // regardless of whether ingest stamped it on the in-memory `Node`, so
    // `kb_node_source_file`/`help_edit_source` reported "No source file" for
    // a node whose backing file plainly existed on disk. Simulated here via
    // a fresh open + `load_all()` rather than reusing the live `store`, to
    // exercise the actual reload path instead of relying on in-memory state
    // that never touched the persisted relations.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("kb.db");
    let store = CozoKbStore::open(&db_path).unwrap();
    store
        .insert_node(&Node::new(
            "user:reload-1",
            "Reload Me",
            NodeKind::Note,
            "body",
        ))
        .unwrap();
    store
        .record_source_file(
            "/home/user/notes/reload.org",
            "hash1",
            1,
            &["user:reload-1".to_string()],
        )
        .unwrap();
    drop(store);

    let reopened = CozoKbStore::open(&db_path).unwrap();
    let nodes = reopened.load_all().unwrap();
    let node = nodes
        .iter()
        .find(|n| n.id == "user:reload-1")
        .expect("node must survive reload");
    assert_eq!(
        node.source_file.as_deref(),
        Some(std::path::Path::new("/home/user/notes/reload.org")),
        "a node reloaded from a persisted store must have source_file reconstructed \
         from the source_files table, not silently None"
    );
}
