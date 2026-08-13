//! Provenance guards on MAE's own bundled content.
//!
//! `kb.primary` is a mixture: the user's notes AND the ~1,200-node bundled
//! manual. Every guard that keeps the two apart keys on `NodeSource::Seed`, so
//! these tests cover both the guard and the thing that makes the stamp
//! trustworthy in the first place.

use super::*;

/// Phase D3b guard: the snapshot writes the user's *own* notes back to the
/// local store — it must never copy MAE's bundled manual in with them.
///
/// `kb.primary` carries ~1,200 built-in nodes alongside the user's content, so
/// an unfiltered snapshot dumped MAE's whole manual into `primary.cozo` on every
/// shutdown and every collab disconnect.
///
/// The oracle is deliberately two-sided: proving the seed nodes are absent is
/// worthless if the guard also dropped the user data the snapshot exists to save.
#[test]
fn snapshot_skips_builtin_content_but_still_persists_user_notes() {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));

    // Real built-ins, several of them, spanning the id namespaces the manual
    // actually uses — not one hand-picked node that might be special.
    let builtins: Vec<String> = editor
        .kb
        .primary
        .list_ids(None)
        .into_iter()
        .filter(|id| {
            editor.kb.primary.get(id).and_then(|n| n.source) == Some(mae_kb::NodeSource::Seed)
        })
        .collect();
    assert!(
        builtins.len() > 100,
        "expected the seeded manual in the mirror, found {} nodes",
        builtins.len()
    );

    // User content that MUST survive: varied kinds, and one that deliberately
    // wears a built-in-looking id prefix to prove the discriminator is the
    // provenance stamp and not the id.
    let user_nodes = [
        ("note:journal", "Journal", mae_kb::NodeKind::Note),
        ("daily:2026-08-13", "Today", mae_kb::NodeKind::Note),
        ("concept:my-own-concept", "Mine", mae_kb::NodeKind::Concept),
        ("cmd:my-macro", "My macro", mae_kb::NodeKind::Note),
    ];
    for (id, title, kind) in user_nodes {
        editor.kb.primary.insert(
            mae_kb::Node::new(id, title, kind, "user body")
                .with_source(mae_kb::NodeSource::Manual, 0),
        );
    }

    editor.kb_snapshot_primary_to_store();

    let store = editor.kb.store.as_ref().unwrap();
    for (id, _, _) in user_nodes {
        assert!(
            store.get_node(id).unwrap().is_some(),
            "user node {id} must be persisted by the snapshot"
        );
    }
    let leaked: Vec<&String> = builtins
        .iter()
        .filter(|id| store.get_node(id).unwrap().is_some())
        .collect();
    assert!(
        leaked.is_empty(),
        "{} built-in nodes leaked into the user's store (e.g. {:?})",
        leaked.len(),
        &leaked[..leaked.len().min(5)]
    );
}

/// The guard above trusts the `Seed` stamp, and that trust is only warranted
/// because the manual ingest restores what it overwrites.
///
/// `ingest_org_dir` REPLACES a node, so ingesting `assets/manual/*.org` over the
/// code-generated nodes strips their provenance. This pins the whole chain:
/// stamped → ingest strips it → `stamp_source_for` restores it → the built-in
/// guards fire again and the snapshot stays clean. Without the restore step,
/// `kb_update_node`/`kb_delete_node` accept writes to MAE's own help.
#[test]
fn manual_ingest_strips_provenance_and_the_rescan_restores_every_guard() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Stand in for the bundled corpus: hand-written prose for ids that already
    // exist as seeded nodes.
    for (file, id) in [("buffer.org", "concept:buffer"), ("kb.org", "concept:kb")] {
        std::fs::write(
            tmp.path().join(file),
            format!(":PROPERTIES:\n:ID:       {id}\n:END:\n#+title: Prose\n\nRicher hand-written prose.\n"),
        )
        .unwrap();
    }

    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));

    assert_eq!(
        editor.kb.primary.get("concept:buffer").unwrap().source,
        Some(mae_kb::NodeSource::Seed),
        "precondition: seeded nodes start stamped"
    );

    let report = editor.kb.primary.ingest_org_dir(tmp.path());
    assert_eq!(report.indexed, 2, "both corpus nodes ingested");
    assert_eq!(
        editor.kb.primary.get("concept:buffer").unwrap().source,
        None,
        "the ingest is what destroys provenance — if this ever stops being true, \
         the restore below is redundant rather than load-bearing"
    );
    // Provenance gone ⇒ MAE's own help is writable and deletable.
    assert!(
        editor
            .kb_update_node("concept:buffer", None, Some("vandalized"), None)
            .is_ok(),
        "documents the unguarded state this restore closes"
    );

    // What bootstrap does immediately after the ingest.
    editor
        .kb
        .primary
        .stamp_source_for(&report.ingested_ids, mae_kb::NodeSource::Seed, 1);

    for id in ["concept:buffer", "concept:kb"] {
        assert_eq!(
            editor.kb.primary.get(id).unwrap().source,
            Some(mae_kb::NodeSource::Seed),
            "{id}: provenance restored after the corpus ingest"
        );
        let err = editor
            .kb_update_node(id, None, Some("vandalized"), None)
            .expect_err("built-in help must reject edits once re-stamped");
        assert!(err.contains("seed node"), "unexpected error: {err}");
        let err = editor
            .kb_delete_node(id)
            .expect_err("built-in help must reject deletion once re-stamped");
        assert!(err.contains("seed node"), "unexpected error: {err}");
    }

    // ...and the restored stamp is what keeps the snapshot out of user storage.
    //
    // Checked on `concept:kb` only: the unguarded-state edit above already wrote
    // `concept:buffer` through to the store, which is itself the damage this
    // restore prevents — but it means that id can no longer distinguish "the
    // snapshot skipped it" from "the earlier write put it there".
    editor.kb_snapshot_primary_to_store();
    assert!(
        editor
            .kb
            .store
            .as_ref()
            .unwrap()
            .get_node("concept:kb")
            .unwrap()
            .is_none(),
        "a re-stamped corpus node must not reach the user's store"
    );
}
