use super::*;
use sha2::Digest;

fn real_mtime(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- #303 follow-up: detect_reimport_stale_files ---
//
// Tests deliberately record a STORED mtime that's artificially older than
// the file's real on-disk mtime (rather than sleeping + touching the file),
// so drift detection is deterministic and CI-fast — no filesystem mtime
// resolution/timing dependency.

#[test]
fn detect_reimport_stale_files_flags_edited_file_content_changed() {
    let (_tmp, store) = make_store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.org");
    std::fs::write(&path, "current content").unwrap();
    let path_str = path.to_string_lossy().to_string();
    let real_mtime = real_mtime(&path);

    store
        .record_source_file(
            &path_str,
            "stale-hash-from-before-the-edit",
            real_mtime - 100,
            &["user:note".to_string()],
        )
        .unwrap();

    let stale = store.detect_reimport_stale_files().unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].file_path, path);
    assert_eq!(stale[0].node_ids, vec!["user:note".to_string()]);
    assert_eq!(stale[0].stored_mtime, real_mtime - 100);
    assert_eq!(stale[0].current_mtime, real_mtime);
    assert!(
        stale[0].content_changed,
        "content genuinely differs from the stored hash"
    );
}

#[test]
fn detect_reimport_stale_files_touch_without_edit_is_not_content_changed() {
    let (_tmp, store) = make_store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.org");
    let content = "unchanged content";
    std::fs::write(&path, content).unwrap();
    let path_str = path.to_string_lossy().to_string();
    let real_mtime = real_mtime(&path);
    let real_hash = hex::encode(sha2::Sha256::digest(content.as_bytes()));

    // mtime drifted (as if the file was `touch`ed) but content is byte-identical.
    store
        .record_source_file(&path_str, &real_hash, real_mtime - 100, &[])
        .unwrap();

    let stale = store.detect_reimport_stale_files().unwrap();
    assert_eq!(stale.len(), 1, "mtime drift alone is still flagged");
    assert!(
        !stale[0].content_changed,
        "content is byte-identical, only mtime drifted"
    );
}

#[test]
fn detect_reimport_stale_files_excludes_files_with_matching_mtime() {
    let (_tmp, store) = make_store();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.org");
    std::fs::write(&path, "content").unwrap();
    let path_str = path.to_string_lossy().to_string();
    let real_mtime = real_mtime(&path);

    store
        .record_source_file(&path_str, "irrelevant-hash", real_mtime, &[])
        .unwrap();

    let stale = store.detect_reimport_stale_files().unwrap();
    assert!(
        stale.is_empty(),
        "a file whose mtime still matches what was recorded must not be flagged"
    );
}

#[test]
fn detect_reimport_stale_files_skips_deleted_files() {
    // A deleted file is federation.rs's `Full`-reimport-mode concern (id
    // retraction), not this signal's — it must be skipped, not flagged.
    let (_tmp, store) = make_store();
    store
        .record_source_file(
            "/nonexistent/path/gone.org",
            "hash",
            1,
            &["user:gone".to_string()],
        )
        .unwrap();

    let stale = store.detect_reimport_stale_files().unwrap();
    assert!(stale.is_empty());
}

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
