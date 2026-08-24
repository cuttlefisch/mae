use super::*;

#[test]
fn node_versioning_lifecycle() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("v:1", "Original", NodeKind::Note, "First body"))
        .unwrap();

    // Snapshot v1
    let v1 = store.snapshot_version("v:1", "initial").unwrap();
    assert_eq!(v1, 1);

    // Update
    let mut updated = Node::new("v:1", "Updated", NodeKind::Note, "Second body");
    updated.todo_state = Some("DONE".to_string());
    store.update_node(&updated).unwrap();

    // Snapshot v2
    let v2 = store
        .snapshot_version("v:1", "updated title and body")
        .unwrap();
    assert_eq!(v2, 2);

    // History
    let history = store.node_history("v:1", 10).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version, 2); // newest first
    assert_eq!(history[0].title, "Updated");
    assert_eq!(history[1].version, 1);
    assert_eq!(history[1].title, "Original");

    // Restore to v1
    store.restore_version("v:1", 1).unwrap();
    let restored = store.get_node("v:1").unwrap().unwrap();
    assert_eq!(restored.title, "Original");
    assert_eq!(restored.body, "First body");

    // History should now have 4 entries (v1, v2, pre-restore, post-restore)
    let history2 = store.node_history("v:1", 10).unwrap();
    assert_eq!(history2.len(), 4);
}

#[test]
fn version_checksum_integrity() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "cs:1",
            "Checksummed",
            NodeKind::Note,
            "Body content",
        ))
        .unwrap();

    // Snapshot creates a content hash
    store.snapshot_version("cs:1", "initial").unwrap();
    let history = store.node_history("cs:1", 10).unwrap();
    assert_eq!(history.len(), 1);

    // Verify hash is non-empty and deterministic
    let v = &history[0];
    assert!(
        !v.content_hash.is_empty(),
        "content_hash should be populated"
    );
    assert_eq!(
        v.content_hash.len(),
        64,
        "hash should be SHA-256 hex (64 chars)"
    );

    // Verify integrity check passes
    assert!(
        v.verify_integrity(),
        "freshly created version should pass integrity check"
    );

    // Compute expected hash independently
    let expected_hash = NodeVersion::compute_hash("Checksummed", "Body content", "[]", "", "");
    assert_eq!(
        v.content_hash, expected_hash,
        "stored hash should match computed hash"
    );

    // Determinism: same content always produces same hash
    let hash2 = NodeVersion::compute_hash("Checksummed", "Body content", "[]", "", "");
    assert_eq!(expected_hash, hash2, "hash function must be deterministic");
}

#[test]
fn version_checksum_detects_different_content() {
    // Verify that different content produces different hashes
    let h1 = NodeVersion::compute_hash("Title A", "Body A", "[]", "", "");
    let h2 = NodeVersion::compute_hash("Title B", "Body A", "[]", "", "");
    let h3 = NodeVersion::compute_hash("Title A", "Body B", "[]", "", "");
    let h4 = NodeVersion::compute_hash("Title A", "Body A", "[]", "TODO", "");
    let h5 = NodeVersion::compute_hash("Title A", "Body A", "[]", "", "A");

    assert_ne!(h1, h2, "different title should produce different hash");
    assert_ne!(h1, h3, "different body should produce different hash");
    assert_ne!(h1, h4, "different todo_state should produce different hash");
    assert_ne!(h1, h5, "different priority should produce different hash");
}

#[test]
fn restore_verifies_checksum() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("rv:1", "Original", NodeKind::Note, "Content"))
        .unwrap();
    store.snapshot_version("rv:1", "initial").unwrap();

    // Update and snapshot v2
    store
        .update_node(&Node::new("rv:1", "Updated", NodeKind::Note, "New content"))
        .unwrap();
    store.snapshot_version("rv:1", "update").unwrap();

    // Restore to v1 should succeed (hash is valid)
    store.restore_version("rv:1", 1).unwrap();
    let node = store.get_node("rv:1").unwrap().unwrap();
    assert_eq!(node.title, "Original");
    assert_eq!(node.body, "Content");

    // Verify the restored version has a valid hash too
    let history = store.node_history("rv:1", 10).unwrap();
    for v in &history {
        assert!(
            v.verify_integrity(),
            "version {} should pass integrity check (hash: {})",
            v.version,
            v.content_hash
        );
    }
}

/// The whole point of `snapshot_version`: a destructive ingest must be undoable.
///
/// Until `insert_node_with_history` existed it had NO production caller — only
/// `restore_version` snapshotting before its own overwrite — so `kb_history` was
/// always empty and `kb_restore` could not undo the one thing that destroys
/// content: a re-ingest replacing store state from a `.org` file.
#[test]
fn an_overwriting_ingest_is_recoverable_from_history() {
    let store = CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();

    let original = Node::new("n1", "Title", NodeKind::Note, "the original body");
    store.insert_node(&original).unwrap();

    let clobbering = Node::new("n1", "Title", NodeKind::Note, "REPLACED BY INGEST");
    let snapshotted = store
        .insert_node_with_history(&clobbering, "replaced by org-directory ingest")
        .unwrap();
    assert!(
        snapshotted,
        "an overwrite of different content must snapshot"
    );

    // The clobber landed...
    assert_eq!(
        store.get_node("n1").unwrap().unwrap().body,
        "REPLACED BY INGEST"
    );

    // ...and is undoable, which is the property that matters.
    let history = store.node_history("n1", 100).unwrap();
    assert_eq!(history.len(), 1, "exactly one version recorded");
    store.restore_version("n1", history[0].version).unwrap();
    assert_eq!(
        store.get_node("n1").unwrap().unwrap().body,
        "the original body",
        "restoring the snapshot must recover the pre-ingest content"
    );
}

/// Bounded by construction: re-ingesting unchanged content must record nothing.
///
/// This is the case that actually dominates — a watcher tick, a daemon scheduler
/// pass, a startup ingest all re-read files that did not change. Appending a
/// version each time would grow `node_versions` without recording anything a
/// user could want back.
#[test]
fn re_ingesting_unchanged_content_records_no_version() {
    let store = CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();

    let node = Node::new("n1", "Title", NodeKind::Note, "stable body");
    store.insert_node(&node).unwrap();

    for _ in 0..25 {
        let snapshotted = store
            .insert_node_with_history(&node.clone(), "ingest")
            .unwrap();
        assert!(!snapshotted, "unchanged content must not snapshot");
    }
    assert!(
        store.node_history("n1", 100).unwrap().is_empty(),
        "25 unchanged re-ingests must leave history empty"
    );

    // A new node is not an overwrite either.
    let fresh = Node::new("n2", "Fresh", NodeKind::Note, "brand new");
    assert!(
        !store.insert_node_with_history(&fresh, "ingest").unwrap(),
        "creating a node destroys nothing, so it must not snapshot"
    );
}

/// #731 — version history must survive the projector's repair path.
///
/// The audit claimed `reconcile_kb`'s heal DESTROYS history. That is **false**,
/// and worth pinning rather than leaving as folklore: `delete_node` removes the
/// node row and its links and does not touch `node_versions`, and nothing else
/// in the store deletes versions either. So history outlives both a delete and a
/// re-projection, which is what makes it usable as a recovery surface at all.
#[test]
fn version_history_survives_node_deletion_and_reprojection() {
    let store = CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();

    store
        .insert_node(&Node::new("n1", "T", NodeKind::Note, "v1 body"))
        .unwrap();
    store
        .insert_node_with_history(&Node::new("n1", "T", NodeKind::Note, "v2 body"), "edit")
        .unwrap();
    assert_eq!(store.node_history("n1", 100).unwrap().len(), 1);

    // What the projector's heal does to an "extra" node.
    store.delete_node("n1").unwrap();
    assert!(store.get_node("n1").unwrap().is_none(), "node row is gone");
    assert_eq!(
        store.node_history("n1", 100).unwrap().len(),
        1,
        "deleting a node must NOT delete its version history"
    );

    // And what it does to a "differing" node: an unconditional re-put.
    store
        .insert_node(&Node::new("n1", "T", NodeKind::Note, "reprojected"))
        .unwrap();
    assert_eq!(
        store.node_history("n1", 100).unwrap().len(),
        1,
        "re-projection must not disturb history either"
    );
}

/// Overwriting SEEDED content records no version.
///
/// MAE seeds code-generated nodes and then ingests a corpus over them, so
/// without this the very first ingest on a fresh install would snapshot every
/// seeded node it touched — which is both noise and a direct contradiction of
/// the bound the test above pins. Seed content is regenerable from the binary
/// (ADR-104), so there is nothing a user could want restored.
///
/// Caught by CI rather than by the unit tests: `crates/mae/tests/` seeds a store
/// and then imports org fixtures over it, which is exactly this shape.
#[test]
fn overwriting_seeded_content_records_no_version() {
    let store = CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();

    let mut seeded = Node::new("concept:x", "Seeded", NodeKind::Concept, "generated body");
    seeded.source = Some(crate::NodeSource::Seed);
    store.insert_node(&seeded).unwrap();

    let from_corpus = Node::new(
        "concept:x",
        "Seeded",
        NodeKind::Concept,
        "richer prose from org",
    );
    assert!(
        !store
            .insert_node_with_history(&from_corpus, "corpus ingest")
            .unwrap(),
        "an ingest over SEEDED content must not snapshot"
    );
    assert!(
        store.node_history("concept:x", 100).unwrap().is_empty(),
        "no version recorded for a seed overwrite"
    );

    // But once the node is no longer seed-provenanced, it is user content again
    // and a further overwrite DOES snapshot — the skip is about provenance, not
    // about the id.
    let authored = Node::new("concept:x", "Seeded", NodeKind::Concept, "user edit");
    assert!(
        store
            .insert_node_with_history(&authored, "user edit")
            .unwrap(),
        "content that is no longer Seed-provenanced must snapshot again"
    );
}
