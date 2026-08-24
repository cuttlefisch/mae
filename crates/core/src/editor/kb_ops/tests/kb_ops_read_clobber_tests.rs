//! #729: **reading** a KB node reverts it to its `.org` file.
//!
//! Activity tracking writes a `:last-accessed:` property into the node's source
//! file and then reimports that file over the store. The reimport re-parses the
//! file as truth, so anything that exists only in the store — an MCP
//! `kb_update`, a `(kb-update-node)`, any edit that never round-tripped to disk
//! — is silently undone the next time *anyone opens the node*.
//!
//! Default-on: gated only on `kb.activity_tracking`, which defaults to `true`.
//!
//! These tests are written to FAIL against the current implementation. They are
//! the reproduction the fix is measured against, not a description of intended
//! behaviour.

use super::*;
use mae_kb::federation::{KbInstance, KbInstanceKind};

const UUID: &str = "uuid-read-clobber";

/// A node carrying an explicit `source_file`, since `kb_node_source_path`
/// resolves only through `kb.instances` and reads that field directly.
fn node_from_file(id: &str, body: &str, path: &std::path::Path) -> mae_kb::Node {
    let mut n = mae_kb::Node::new(id, id, mae_kb::NodeKind::Note, body);
    n.source_file = Some(path.to_path_buf());
    n
}

/// Register `dir` as a real KB instance so `kb_reimport_file` will claim paths
/// under it. `org_dir` is canonicalized because the reimport path canonicalizes
/// before its `starts_with` containment check, and a TempDir is frequently
/// reached through a symlink (`/tmp` on macOS being the standard case).
fn register(editor: &mut Editor, dir: &std::path::Path) {
    let org_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    editor.kb.registry.instances.push(KbInstance {
        uuid: UUID.into(),
        name: "read-clobber".into(),
        org_dir,
        db_path: dir.join("kb.db"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: None,
        kind: KbInstanceKind::default(),
        priority: 0,
        remote_hub: None,
    });
}

/// The headline defect: a store-only edit does not survive being read.
///
/// This is the exact shape of an MCP `kb_update` — the AI peer's only way to
/// edit a node — followed by the human opening that node in the KB viewer.
#[test]
fn reading_a_node_does_not_revert_a_store_only_edit() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("note.org");
    std::fs::write(
        &path,
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nOriginal body from disk.\n",
    )
    .unwrap();

    let mut editor = Editor::new();
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node_from_file("note-a", "Original body from disk.", &path));
    editor.kb.instances.insert(UUID.into(), kb);
    register(&mut editor, dir.path());

    // The store-only edit. Nothing writes the file — that is the whole point.
    editor
        .kb
        .instances
        .get_mut(UUID)
        .unwrap()
        .insert(node_from_file(
            "note-a",
            "EDITED IN THE STORE, never written to disk.",
            &path,
        ));

    // Read it. Not an edit, not a save — a read.
    editor.kb_record_access("note-a");

    let body = editor
        .kb
        .instances
        .get(UUID)
        .and_then(|kb| kb.get("note-a"))
        .map(|n| n.body.clone())
        .unwrap_or_default();

    assert_eq!(
        body, "EDITED IN THE STORE, never written to disk.",
        "reading a node must not revert it to its .org file — the store-only \
         edit was silently replaced by the on-disk body"
    );
}

/// The sharper case: reading one node **destroys a different one**.
///
/// `kb_reimport_file` retracts every id the file no longer produces
/// (`kb_ops/search.rs`), and retraction is a `remove` plus a durable delete. A
/// node created in the store and attributed to that file — which is what
/// capture and `kb_create` produce before any export exists — is therefore
/// deleted outright by an unrelated read.
#[test]
fn reading_a_node_does_not_delete_a_sibling_that_exists_only_in_the_store() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("note.org");
    std::fs::write(
        &path,
        ":PROPERTIES:\n:ID: note-a\n:END:\n#+title: note-a\n\nOn disk.\n",
    )
    .unwrap();

    let mut editor = Editor::new();
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node_from_file("note-a", "On disk.", &path));
    // Never written to the file, but attributed to it.
    kb.insert(node_from_file("note-b", "Store-only sibling.", &path));
    editor.kb.instances.insert(UUID.into(), kb);
    register(&mut editor, dir.path());

    editor.kb_record_access("note-a");

    assert!(
        editor
            .kb
            .instances
            .get(UUID)
            .map(|kb| kb.get("note-b").is_some())
            .unwrap_or(false),
        "reading note-a must not retract note-b — the reimport treated the file \
         as the complete set of nodes and deleted the store-only sibling"
    );
}

/// The property, rather than two hand-picked cases: for a corpus of nodes,
/// reading **any** of them must leave **every** node's body untouched.
///
/// Written as a sweep because the defect is a property of the read path, not of
/// a particular node — a fix that special-cases one entry point or one node
/// shape would pass the two tests above and still fail this.
#[test]
fn reading_any_node_leaves_every_body_untouched() {
    let dir = TempDir::new().unwrap();
    let ids = ["n1", "n2", "n3", "n4"];
    for id in ids {
        std::fs::write(
            dir.path().join(format!("{id}.org")),
            format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n\ndisk body {id}\n"),
        )
        .unwrap();
    }

    let mut editor = Editor::new();
    let mut kb = mae_kb::KnowledgeBase::new();
    for id in ids {
        let p = dir.path().join(format!("{id}.org"));
        // Every node carries a store-only body, distinct from its file.
        kb.insert(node_from_file(id, &format!("STORE body {id}"), &p));
    }
    editor.kb.instances.insert(UUID.into(), kb);
    register(&mut editor, dir.path());

    let expected: Vec<String> = ids.iter().map(|id| format!("STORE body {id}")).collect();

    for read_id in ids {
        editor.kb_record_access(read_id);

        let actual: Vec<String> = ids
            .iter()
            .map(|id| {
                editor
                    .kb
                    .instances
                    .get(UUID)
                    .and_then(|kb| kb.get(id))
                    .map(|n| n.body.clone())
                    .unwrap_or_else(|| "<MISSING>".into())
            })
            .collect();

        assert_eq!(
            actual, expected,
            "after reading '{read_id}', at least one node's store-only body was \
             reverted to (or deleted from) its .org file"
        );
    }
}
