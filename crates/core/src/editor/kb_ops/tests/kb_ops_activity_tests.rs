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

/// The unfiled node-scoping bug found while investigating #316:
/// `kb_record_modification` used to hash the WHOLE file after its first
/// `:END:` and misattribute the result to whichever node
/// `kb_find_node_by_path` happened to return first — so editing one
/// sibling node's body silently rewrote a DIFFERENT sibling's
/// `:hash:`/`:last-modified:`. This file has two list-item nodes sharing
/// one `source_file`, mirroring the #332 minimal repro shape.
#[test]
fn kb_record_modification_only_updates_the_node_whose_body_actually_changed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("steps.org");
    let make_content = |step1: &str, step2: &str| {
        format!(
            ":PROPERTIES:\n:ID: file-id\n:END:\n#+title: Repro\n\n* Steps\n\n1. {step1}\n   :PROPERTIES:\n   :ID: step-1\n   :END:\n2. {step2}\n   :PROPERTIES:\n   :ID: step-2\n   :END:\n"
        )
    };
    let initial = make_content("First step.", "Second step.");
    std::fs::write(&path, &initial).unwrap();

    let mut editor = Editor::new();
    let mut kb = mae_kb::KnowledgeBase::new();
    for (id, title, body) in [
        ("file-id", "Repro", ""),
        ("step-1", "First step.", "First step."),
        ("step-2", "Second step.", "Second step."),
    ] {
        let mut node = mae_kb::Node::new(id, title, mae_kb::NodeKind::Note, body);
        node.source_file = Some(path.clone());
        // Seed each node's :hash: to what it would be for the initial content
        // (mirroring what a real ingest would compute), so the first
        // recorded modification only sees whichever node's body actually
        // moved.
        let parsed = mae_kb::org::parse_org_multi(&initial);
        let parsed_node = parsed.iter().find(|n| n.id == id).unwrap();
        node.properties.insert(
            "hash".to_string(),
            mae_kb::activity::body_hash(&parsed_node.body),
        );
        kb.insert(node);
    }
    editor.kb.instances.insert("test-instance".to_string(), kb);

    // Only step-2's text changes.
    let changed = make_content("First step.", "Second step, edited.");
    std::fs::write(&path, &changed).unwrap();

    editor.kb_record_modification(&path);

    let step1_modified = editor
        .kb_get_node_mut("step-1")
        .and_then(|n| n.properties.get("last-modified").cloned());
    let step2_modified = editor
        .kb_get_node_mut("step-2")
        .and_then(|n| n.properties.get("last-modified").cloned());
    assert!(
        step1_modified.is_none(),
        "step-1's body didn't change — its :last-modified: must not be touched"
    );
    assert!(
        step2_modified.is_some(),
        "step-2's body changed — its :last-modified: must be stamped"
    );

    // The on-disk drawers reflect the same: only step-2 gained the property.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    let step1_drawer_start = on_disk.find(":ID: step-1").unwrap();
    let step1_drawer_end =
        on_disk[step1_drawer_start..].find(":END:").unwrap() + step1_drawer_start;
    assert!(!on_disk[step1_drawer_start..step1_drawer_end].contains("last-modified"));
    let step2_drawer_start = on_disk.find(":ID: step-2").unwrap();
    let step2_drawer_end =
        on_disk[step2_drawer_start..].find(":END:").unwrap() + step2_drawer_start;
    assert!(on_disk[step2_drawer_start..step2_drawer_end].contains("last-modified"));
}
