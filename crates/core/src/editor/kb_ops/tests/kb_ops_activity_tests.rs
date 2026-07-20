//! Regression tests for KB activity tracking (#316): a self-inflicted write
//! to a node's `:PROPERTIES:` drawer must not make an open buffer for that
//! same file look externally modified.

use super::*;

fn insert_test_instance(editor: &mut Editor, node: mae_kb::Node) {
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node);
    editor.kb.instances.insert("test-instance".to_string(), kb);
}

/// #316: `kb_update_property_in_file` writes to disk (bumping the real
/// mtime), but before the fix nothing told an open `Buffer` for that same
/// path that the change was self-inflicted — its independent freshness
/// tracking (`file_mtime`/`content_hash`) would then detect the change on
/// its own and the next focus-regain would fire a spurious "changed on
/// disk, reload?" prompt during active editing.
#[test]
fn kb_update_property_in_file_resyncs_an_open_buffers_freshness_state() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("note.org");
    std::fs::write(
        &path,
        ":PROPERTIES:\n:ID: test-node\n:END:\n#+title: Note\n\nBody.\n",
    )
    .unwrap();

    let mut editor = Editor::new();
    let mut node = mae_kb::Node::new("test-node", "Note", mae_kb::NodeKind::Note, "Body.");
    node.source_file = Some(path.clone());
    insert_test_instance(&mut editor, node);

    // Open the same file in a buffer, exactly like a user actively editing it.
    let buf = crate::buffer::Buffer::from_file(&path).unwrap();
    editor.buffers.push(buf);
    let buf_idx = editor.buffers.len() - 1;

    assert!(!editor.buffers[buf_idx].check_disk_changed());
    assert!(!editor.buffers[buf_idx].check_disk_changed_by_hash());

    // Simulate the self-write activity tracking performs (e.g. via
    // kb_record_access/kb_record_modification).
    editor.kb_update_property_in_file(&path, "test-node", "last-accessed", "2026-07-20");

    // A real external change on disk occurred (the file's bytes did
    // change), but the buffer must recognize this as already-accounted-for,
    // not something requiring a reload prompt.
    assert!(
        !editor.buffers[buf_idx].check_disk_changed(),
        "self-inflicted KB write must not look like an external mtime change"
    );
    assert!(
        !editor.buffers[buf_idx].check_disk_changed_by_hash(),
        "self-inflicted KB write must not look like an external content change"
    );
}

/// Without the buffer lookup, a genuinely external change to a DIFFERENT
/// open buffer's file must still be detected normally — the fix must not
/// accidentally suppress real external-change detection.
#[test]
fn resync_after_external_write_does_not_mask_unrelated_files() {
    let dir = TempDir::new().unwrap();
    let tracked_path = dir.path().join("tracked.org");
    let other_path = dir.path().join("other.org");
    std::fs::write(
        &tracked_path,
        ":PROPERTIES:\n:ID: tracked-node\n:END:\n#+title: Tracked\n\nBody.\n",
    )
    .unwrap();
    std::fs::write(&other_path, "unrelated content\n").unwrap();

    let mut editor = Editor::new();
    let mut node = mae_kb::Node::new("tracked-node", "Tracked", mae_kb::NodeKind::Note, "Body.");
    node.source_file = Some(tracked_path.clone());
    insert_test_instance(&mut editor, node);

    let other_buf = crate::buffer::Buffer::from_file(&other_path).unwrap();
    editor.buffers.push(other_buf);
    let other_idx = editor.buffers.len() - 1;

    // A real external edit to the unrelated file, independent of any KB write.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&other_path, "changed externally\n").unwrap();

    editor.kb_update_property_in_file(&tracked_path, "tracked-node", "last-accessed", "2026-07-20");

    assert!(
        editor.buffers[other_idx].check_disk_changed_by_hash(),
        "an unrelated file's real external change must still be detected"
    );
}
