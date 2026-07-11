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
