//! The body-format write guard (`kb_ops::literal_guard`).
//!
//! The oracle throughout is the node's **CRDT document bytes** and the outbound
//! queue, not a status string: a guard that printed a refusal and wrote anyway
//! would pass a message-only check. Each refusal is paired with the same call on
//! a KB that does not carry the marker, so a guard that refused everything —
//! or a fixture in which nothing could ever be written — fails too.

use super::*;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};

const UUID: &str = "uuid-lit";
const NAME: &str = "litkb";
const KB_ID: &str = "litkb-collab-7f3a";
const NODE: &str = "lit:n";
/// An org-roam id link with a description — the form MAE's writer rewrites.
const LITERAL_BODY: &str =
    "See [[id:lit:other][the other note]] for context.\n\nSecond paragraph.\n";
/// A `kind` outside `NodeKind`'s 14 values — MAE's writer folds it to `note`.
const LITERAL_KIND: &str = "decision";

fn instance() -> mae_kb::federation::KbInstance {
    let mut inst = mae_kb::federation::KbInstance::joined(
        UUID.into(),
        KB_ID,
        std::path::PathBuf::new(),
        "2026-11-09T00:00:00Z".into(),
    );
    inst.name = NAME.into();
    inst
}

/// The node document as a literal-org seeder writes it: the raw `kind` string
/// and properties with their source case, including an empty one.
fn literal_doc() -> KbNodeDoc {
    let mut doc = KbNodeDoc::new_with_client_id(
        NODE,
        "Literal note",
        LITERAL_BODY,
        &["alpha".to_string()],
        0x5eed,
    );
    let _ = doc.set_kind(Some(LITERAL_KIND));
    let props: std::collections::HashMap<String, String> = [
        ("Owner".to_string(), "ops".to_string()),
        ("Reviewed".to_string(), String::new()),
    ]
    .into_iter()
    .collect();
    let _ = doc.set_properties(&props);
    doc
}

/// A joined KB holding one seeder-written node, with a collection replica that
/// declares `format` (`None` = no key, i.e. every KB before the marker).
fn editor_with_kb(format: Option<&str>) -> (Editor, TempDir) {
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    editor.collab.kb_sync_mode = "on_save".into();
    editor.kb.registry.instances.push(instance());

    let doc = literal_doc();
    let node =
        mae_kb::Node::from_crdt_doc(&doc, mae_kb::NodeKind::Note, mae_kb::NodeSource::Federation);
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node);
    editor.kb.instances.insert(UUID.into(), kb);

    let mut coll = KbCollectionDoc::new(NAME, "operator");
    coll.add_node(NODE, "Literal note");
    if let Some(f) = format {
        let _ = coll.set_body_format(f);
    }
    editor
        .collab
        .kb_collection_state
        .insert(KB_ID.into(), coll.encode_state());
    (editor, dirs)
}

fn doc_bytes(editor: &Editor) -> Option<Vec<u8>> {
    editor.kb.instances[UUID]
        .get(NODE)
        .and_then(|n| n.crdt_doc.clone())
}

fn doc(editor: &Editor) -> KbNodeDoc {
    KbNodeDoc::from_bytes(&doc_bytes(editor).expect("node has a CRDT doc")).unwrap()
}

fn queued(editor: &Editor) -> usize {
    editor.collab.pending_kb_updates.len() + editor.collab.pending_kb_manifest.len()
}

/// Open the node as org source and save it unchanged — the human edit surface.
fn save_unchanged(editor: &mut Editor) {
    editor.kb_edit_node(NODE).unwrap();
    let idx = editor.active_buffer_idx();
    assert!(editor.kb_save_node_buffer(idx));
}

fn assert_refused(editor: &Editor, before: &[u8], what: &str, err: &str) {
    assert!(
        err.contains(NAME)
            && err.contains(BODY_FORMAT_ORG_LITERAL_V1)
            && err.contains("ADR-112 O4"),
        "{what}: the refusal must name the KB, the format and what lifts it, got: {err}"
    );
    assert_eq!(
        doc_bytes(editor).as_deref(),
        Some(before),
        "{what}: the node's CRDT document must be byte-unchanged"
    );
    assert_eq!(queued(editor), 0, "{what}: nothing may be queued for peers");
}

/// **Why the guard exists.** With no marker, MAE's own save — open the node,
/// change nothing, `:w` — rewrites the seeder's document and queues the
/// rewrite for every peer. If this ever stops failing (MAE learns to write the
/// format losslessly, ADR-112 O4), the guard can be retired.
#[test]
fn without_the_marker_an_unchanged_save_rewrites_a_literal_node() {
    let (mut editor, _d) = editor_with_kb(None);
    let before = doc_bytes(&editor).unwrap();
    assert_eq!(doc(&editor).kind().as_deref(), Some(LITERAL_KIND));

    save_unchanged(&mut editor);

    let after = doc(&editor);
    assert_ne!(
        doc_bytes(&editor).unwrap(),
        before,
        "the save wrote the doc"
    );
    assert_eq!(
        after.kind().as_deref(),
        Some("note"),
        "MAE folds an unknown kind to `note`"
    );
    // Pinned to what MAE actually writes (measured), not merely "differs": an
    // oracle of "changed somehow" would also pass on unrelated churn.
    assert_eq!(
        after.body(),
        "See [[lit:other|the other note]] for context.\n\nSecond paragraph.\n",
        "MAE rewrites the org-roam id link to its own link syntax"
    );
    let expected_props: std::collections::HashMap<String, String> =
        [("owner".to_string(), "ops".to_string())]
            .into_iter()
            .collect();
    assert_eq!(
        after.properties(),
        expected_props,
        "MAE lowercases property keys and drops the empty one"
    );
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "and the rewrite is queued for every peer"
    );
}

/// The same save on the same node, with the marker: refused, and the document
/// is byte-for-byte what the seeder wrote.
#[test]
fn with_the_marker_the_save_is_refused_and_the_doc_is_byte_unchanged() {
    let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
    let before = doc_bytes(&editor).unwrap();

    save_unchanged(&mut editor);

    assert_refused(
        &editor,
        &before,
        "node-buffer save",
        &editor.status_msg.clone(),
    );
    assert_eq!(doc(&editor).kind().as_deref(), Some(LITERAL_KIND));
    assert_eq!(doc(&editor).body(), LITERAL_BODY);
}

type Mutator = (&'static str, fn(&mut Editor) -> Result<(), String>);

/// Every editor-level path that writes a node of an existing KB. Agent tools,
/// ex-commands and Scheme primitives all lower onto these.
const MUTATORS: &[Mutator] = &[
    ("kb_update_node (title)", |e| {
        e.kb_update_node(NODE, Some("Retitled"), None, None)
    }),
    ("kb_update_node (body)", |e| {
        e.kb_update_node(NODE, None, Some("plain body"), None)
    }),
    ("kb_update_node (tags)", |e| {
        e.kb_update_node(NODE, None, None, Some(vec!["beta".into()]))
    }),
    ("kb_update_node_target (named KB)", |e| {
        e.kb_update_node_target(Some(NAME), NODE, Some("Retitled"), None, None)
    }),
    ("kb_set_role", |e| e.kb_set_role(NODE, "atom").map(|_| ())),
    ("kb_delete_node", |e| e.kb_delete_node(NODE)),
    ("kb_create_node_target (into the KB)", |e| {
        e.kb_create_node_target(Some(NAME), "lit:new", "New", "b", mae_kb::NodeKind::Note)
    }),
    ("kb_promote_node (out of the KB)", |e| {
        e.kb_promote_node(NODE).map(|_| ())
    }),
];

/// Each path is refused on a literal KB with nothing written or queued…
#[test]
fn every_mutator_is_refused_on_a_literal_kb() {
    for (what, run) in MUTATORS {
        let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
        let before = doc_bytes(&editor).unwrap();
        let err = run(&mut editor).expect_err(what);
        assert_refused(&editor, &before, what, &err);
        assert!(
            !editor.kb.instances[UUID].contains("lit:new"),
            "{what}: nothing created"
        );
        assert!(
            !editor.kb.primary.contains(NODE),
            "{what}: nothing promoted"
        );
    }
}

/// …and every one of them still works, and still syncs, on the same KB without
/// the marker — so the refusals above are the guard's, not the fixture's.
#[test]
fn every_mutator_still_works_on_a_kb_without_the_marker() {
    for (what, run) in MUTATORS {
        let (mut editor, _d) = editor_with_kb(None);
        let before = doc_bytes(&editor).unwrap();
        run(&mut editor).unwrap_or_else(|e| panic!("{what}: {e}"));
        let changed = doc_bytes(&editor).as_deref() != Some(before.as_slice())
            || !editor.kb.instances[UUID].contains(NODE)
            || editor.kb.instances[UUID].contains("lit:new");
        assert!(changed, "{what}: the write must land");
        assert!(queued(&editor) > 0, "{what}: and be queued for peers");
    }
}

/// A format this build does not recognise is refused too: MAE writes only the
/// format it owns, and a newer one is no safer than `org-literal-v1`.
#[test]
fn an_unrecognised_body_format_is_refused() {
    let (mut editor, _d) = editor_with_kb(Some("org-literal-v9"));
    let before = doc_bytes(&editor).unwrap();
    let err = editor
        .kb_update_node(NODE, Some("Retitled"), None, None)
        .unwrap_err();
    assert!(
        err.contains("org-literal-v9") && err.contains(NAME),
        "{err}"
    );
    assert_eq!(doc_bytes(&editor).unwrap(), before);
}

/// A replica that is present but undecodable is evidence of a declaration this
/// editor could not read: fail closed rather than let a read error through.
#[test]
fn an_undecodable_replica_fails_closed() {
    let (mut editor, _d) = editor_with_kb(None);
    editor
        .collab
        .kb_collection_state
        .insert(KB_ID.into(), vec![0xff, 0x00, 0x13]);
    let before = doc_bytes(&editor).unwrap();
    let err = editor
        .kb_update_node(NODE, Some("Retitled"), None, None)
        .unwrap_err();
    assert!(err.contains("could not read its collection"), "{err}");
    assert_eq!(doc_bytes(&editor).unwrap(), before);
}

/// An EMPTY replica (a join answered without collection state) and no replica
/// at all are both "unknown", which is fail-open: ordinary shared KBs keep
/// working offline after a restart. See `kb_refuse_foreign_body_format`.
#[test]
fn an_absent_or_empty_replica_with_nothing_recorded_stays_writable() {
    for replica in [None, Some(Vec::new())] {
        let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
        editor.collab.kb_collection_state.remove(KB_ID);
        if let Some(r) = replica.clone() {
            editor.collab.kb_collection_state.insert(KB_ID.into(), r);
        }
        editor
            .kb_update_node(NODE, Some("Retitled"), None, None)
            .unwrap_or_else(|e| panic!("replica {replica:?}: {e}"));
    }
}

/// A KB that was never shared or joined has no collection at all, so it can
/// carry no marker — and a replica filed under some OTHER KB's id must not
/// leak onto it.
#[test]
fn a_never_shared_kb_is_unaffected_by_another_kbs_marker() {
    let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
    editor
        .kb_create_node_target(
            Some("local"),
            "note:mine",
            "Mine",
            "body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    editor
        .kb_update_node("note:mine", Some("Mine, edited"), None, None)
        .unwrap();
    assert_eq!(
        editor.kb.primary.get("note:mine").unwrap().title,
        "Mine, edited"
    );
}

/// The restart case. The in-memory replica is gone (as after an editor
/// restart, before any reconnect), but the format was recorded in the KB's own
/// store when the replica was learned — and that record still refuses. This is
/// the window in which an offline edit would be queued and later shipped.
#[test]
fn the_recorded_format_refuses_after_the_replica_is_gone() {
    let (mut editor, _d) = editor_with_kb(None);
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    editor
        .kb
        .instance_stores
        .insert(UUID.into(), std::sync::Arc::new(store));

    let mut coll = KbCollectionDoc::new(NAME, "operator");
    let _ = coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    editor.kb_note_collection_state(KB_ID, coll.encode_state());
    assert_eq!(
        editor.kb.instance_stores[UUID]
            .collection_body_format()
            .unwrap()
            .as_deref(),
        Some(BODY_FORMAT_ORG_LITERAL_V1),
        "learning the replica records the format in the KB's store"
    );

    // "Restart": the replica is not in memory any more.
    editor.collab.kb_collection_state.clear();
    let before = doc_bytes(&editor).unwrap();
    let err = editor
        .kb_update_node(NODE, Some("Offline edit"), None, None)
        .unwrap_err();
    assert_refused(&editor, &before, "offline edit after restart", &err);

    // A later replica without the marker clears the record: the KB is writable.
    editor.kb_note_collection_state(KB_ID, KbCollectionDoc::new(NAME, "operator").encode_state());
    editor.collab.kb_collection_state.clear();
    editor
        .kb_update_node(NODE, Some("Format lifted"), None, None)
        .unwrap();
}

/// A live `kbc:` broadcast that ADDS the marker to a KB already joined takes
/// effect on the very next write, with no reconnect.
#[test]
fn a_marker_arriving_by_delta_takes_effect_immediately() {
    let (mut editor, _d) = editor_with_kb(None);
    editor
        .kb_update_node(NODE, Some("before the marker"), None, None)
        .unwrap();

    let base = editor.collab.kb_collection_state[KB_ID].clone();
    let mut coll = KbCollectionDoc::from_bytes(&base).unwrap();
    let delta = coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    let mut replica = KbCollectionDoc::from_bytes(&base).unwrap();
    replica.apply_update(&delta).unwrap();
    editor.kb_note_collection_state(KB_ID, replica.encode_state());

    assert!(editor
        .kb_update_node(NODE, Some("after the marker"), None, None)
        .unwrap_err()
        .contains(BODY_FORMAT_ORG_LITERAL_V1));
}

/// Re-joining a literal KB must not re-send local-ahead ops — ops this editor
/// authored on it before it knew (a build without the guard). The daemon's
/// copy is the seeder's; the local one carries MAE's rewrite.
#[test]
fn a_rejoin_does_not_resend_local_ahead_ops_to_a_literal_kb() {
    for (format, expect_resend) in [(None, true), (Some(BODY_FORMAT_ORG_LITERAL_V1), false)] {
        let (mut editor, _d) = editor_with_kb(format);
        // The daemon's view: the seeder's document.
        let daemon = literal_doc();
        let daemon_sv = daemon.state_vector();
        // Locally, a pre-guard MAE edit on top of it.
        let mut local =
            KbNodeDoc::from_bytes_with_client_id(&daemon.encode_state(), 0xabc).unwrap();
        let _ = local.set_kind(Some("note"));
        let mut node = mae_kb::Node::from_crdt_doc(
            &local,
            mae_kb::NodeKind::Note,
            mae_kb::NodeSource::Federation,
        );
        node.crdt_doc = Some(local.encode_state());
        editor.kb.instances.get_mut(UUID).unwrap().insert(node);

        editor.kb_register_joined_instance(
            KB_ID,
            vec![crate::editor::JoinedNode {
                id: NODE.into(),
                bytes: daemon.encode_diff(&local.state_vector()).unwrap(),
                daemon_sv: Some(daemon_sv),
            }],
        );
        assert_eq!(
            !editor.collab.pending_kb_updates.is_empty(),
            expect_resend,
            "format {format:?}: local-ahead re-send"
        );
    }
}

/// A KB whose share marker is off (`shared = false`) but which still carries a
/// collab id keeps its protection: the guard keys on the collab id, not on the
/// broadcast gate, because a later re-share or re-join would push whatever MAE
/// wrote locally (ADR-022 local-ahead).
#[test]
fn an_unshared_literal_kb_is_still_refused() {
    let (mut editor, _d) = editor_with_kb(Some(BODY_FORMAT_ORG_LITERAL_V1));
    editor.kb.registry.find_mut(UUID).unwrap().shared = false;
    let before = doc_bytes(&editor).unwrap();
    let err = editor
        .kb_update_node(NODE, Some("Retitled"), None, None)
        .unwrap_err();
    assert!(err.contains(BODY_FORMAT_ORG_LITERAL_V1), "{err}");
    assert_eq!(doc_bytes(&editor).unwrap(), before);
}

/// Re-import rewrites an instance's nodes from MAE's own ingest. A joined KB has
/// no org directory, so `kb_reimport` already refuses it; this pins the guard
/// for a literal KB that has been given one.
#[test]
fn reimport_is_refused_on_a_literal_kb_with_an_org_dir() {
    for (format, refused) in [(Some(BODY_FORMAT_ORG_LITERAL_V1), true), (None, false)] {
        let (mut editor, d) = editor_with_kb(format);
        let org = d.path().join("org");
        std::fs::create_dir_all(&org).unwrap();
        {
            let inst = editor.kb.registry.find_mut(UUID).unwrap();
            inst.org_dir = org;
            inst.db_path = d.path().join("lit-store");
        }
        let before = doc_bytes(&editor).unwrap();
        let result = editor.kb_reimport(NAME, None);
        assert_eq!(
            result.is_none(),
            refused,
            "format {format:?}: {}",
            editor.status_msg
        );
        if refused {
            assert!(
                editor.status_msg.contains(BODY_FORMAT_ORG_LITERAL_V1),
                "{}",
                editor.status_msg
            );
            assert_eq!(doc_bytes(&editor).unwrap(), before);
        }
    }
}
