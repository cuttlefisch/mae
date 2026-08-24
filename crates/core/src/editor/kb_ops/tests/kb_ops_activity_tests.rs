//! KB activity tracking.
//!
//! Activity timestamps are **per-replica local state** (`kb.activity`), not node
//! content. They used to be written into each node's `:PROPERTIES:` drawer *and
//! its `.org` file*, with the file then reimported over the store — which made a
//! plain read destructive (#729, see `kb_ops_read_clobber_tests`).
//!
//! Historical note, because the coverage here changed shape rather than
//! disappearing: two tests used to guard #316 — a self-inflicted property write
//! bumped a file's mtime, and an open buffer for that same path would then fire
//! a spurious "changed on disk, reload?" prompt mid-edit. Those tests exercised
//! `kb_update_property_in_file`, which no longer exists: activity writes touch
//! no file at all, so #316's trigger is gone from this path *by construction*,
//! which is what the first test below pins.
//!
//! #316's underlying hazard is NOT gone from the codebase — `kb_ops/daily.rs`
//! still writes `.org` files under a `write_guard` and never calls
//! `resync_after_external_write`. That gap is tracked separately; it is not
//! reachable from activity tracking any more.

use super::*;

fn insert_test_instance(editor: &mut Editor, node: mae_kb::Node) {
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(node);
    editor.kb.instances.insert("test-instance".to_string(), kb);
}

/// The #729 invariant, stated positively: recording activity must leave the
/// node's source file **byte-identical**.
///
/// This is deliberately an assertion about the file's bytes rather than about
/// which function was called, so it holds against any future re-implementation
/// of activity tracking — including one that reintroduces a "just update the
/// drawer" shortcut.
#[test]
fn recording_activity_never_touches_the_source_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("note.org");
    let original = ":PROPERTIES:\n:ID: test-node\n:END:\n#+title: Note\n\nBody.\n";
    std::fs::write(&path, original).unwrap();

    let mut editor = Editor::new();
    let mut node = mae_kb::Node::new("test-node", "Note", mae_kb::NodeKind::Note, "Body.");
    node.source_file = Some(path.clone());
    insert_test_instance(&mut editor, node);

    editor.kb_record_access("test-node");
    editor.kb_record_link("test-node");
    editor.kb_record_modification(&path);

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "activity tracking must not write the source file — that write, plus the \
         reimport that followed it, is what made reading a node destructive (#729)"
    );

    // And the node itself is untouched: activity is not node content.
    let node = editor.kb_get_node_mut("test-node").unwrap();
    for key in ["last-accessed", "last-linked", "last-modified", "hash"] {
        assert!(
            !node.properties.contains_key(key),
            "'{key}' must live in the per-replica activity table, not in the node"
        );
    }
}

/// Activity still has to *work* — the point of moving it, not abandoning it.
/// Recorded timestamps must raise the node's activity score, which is what
/// `KbSort::Activity` orders on.
#[test]
fn recorded_activity_still_scores() {
    let mut editor = Editor::new();
    insert_test_instance(
        &mut editor,
        mae_kb::Node::new("scored", "Scored", mae_kb::NodeKind::Note, "Body."),
    );
    let weights = mae_kb::activity::ActivityWeights::default();
    let today = crate::editor::kb_ops::today_ymd();

    let before = editor.kb_activity_score_for_id("scored", &weights, today);
    editor.kb_record_access("scored");
    let after = editor.kb_activity_score_for_id("scored", &weights, today);

    assert!(
        after > before,
        "recording an access must raise the activity score ({before} -> {after})"
    );
}

/// A corpus ingested before #729 carries years of `:last-accessed:` values in
/// its `.org` files. Those must keep counting, or the change silently resets
/// every user's activity ranking.
#[test]
fn historical_properties_from_disk_still_score() {
    let mut editor = Editor::new();
    let mut node = mae_kb::Node::new("legacy", "Legacy", mae_kb::NodeKind::Note, "Body.");
    node.properties.insert("last-accessed".into(), {
        let (y, m, d) = crate::editor::kb_ops::today_ymd();
        mae_kb::activity::format_date(y, m, d)
    });
    insert_test_instance(&mut editor, node);

    let weights = mae_kb::activity::ActivityWeights::default();
    let score =
        editor.kb_activity_score_for_id("legacy", &weights, crate::editor::kb_ops::today_ymd());

    assert!(
        score > 0.0,
        "a pre-existing on-disk :last-accessed: must still contribute — the local \
         table is an overlay, not a replacement"
    );
}

/// The unfiled node-scoping bug found while investigating #316:
/// `kb_record_modification` used to hash the WHOLE file after its first `:END:`
/// and misattribute the result to whichever node `kb_find_node_by_path` returned
/// first — so editing one sibling's body silently stamped a DIFFERENT sibling.
/// Two list-item nodes share one `source_file` here, mirroring #332's shape.
///
/// Now asserted against the local activity table rather than the drawer, since
/// that is where the stamp lives.
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
    let parsed = mae_kb::org::parse_org_multi(&initial);
    for (id, title, body) in [
        ("file-id", "Repro", ""),
        ("step-1", "First step.", "First step."),
        ("step-2", "Second step.", "Second step."),
    ] {
        let mut node = mae_kb::Node::new(id, title, mae_kb::NodeKind::Note, body);
        node.source_file = Some(path.clone());
        kb.insert(node);
        // Seed each node's baseline hash the way a real ingest would, so the
        // first recorded modification sees only whichever body actually moved.
        let parsed_node = parsed.iter().find(|n| n.id == id).unwrap();
        editor.kb.activity.entry(id.to_string()).or_default().hash =
            Some(mae_kb::activity::body_hash(&parsed_node.body));
    }
    editor.kb.instances.insert("test-instance".to_string(), kb);

    // Only step-2's text changes.
    std::fs::write(&path, make_content("First step.", "Second step, edited.")).unwrap();
    editor.kb_record_modification(&path);

    assert!(
        editor
            .kb
            .activity
            .get("step-1")
            .and_then(|a| a.modified.as_ref())
            .is_none(),
        "step-1's body didn't change — it must not be stamped modified"
    );
    assert!(
        editor
            .kb
            .activity
            .get("step-2")
            .and_then(|a| a.modified.as_ref())
            .is_some(),
        "step-2's body changed — it must be stamped modified"
    );
}

/// Activity must survive a restart. Moving it out of the `.org` files removed
/// the thing that used to persist it, so the round-trip is the replacement
/// guarantee and needs its own guard — otherwise the fix for #729 would quietly
/// reset every user's activity ranking on every launch.
#[test]
fn the_activity_table_round_trips_through_disk() {
    let mut editor = Editor::new();
    let _tmp = with_test_dirs(&mut editor);

    editor.kb_record_access("a");
    editor.kb_record_link("b");
    editor.kb.activity.entry("c".into()).or_default().hash = Some("deadbeef".into());
    editor.kb.activity_dirty = true;

    let before = editor.kb.activity.clone();
    editor.kb_save_activity();
    assert!(
        !editor.kb.activity_dirty,
        "a successful save must clear the dirty flag, or every shutdown rewrites"
    );

    // A fresh editor pointed at the same data dir.
    let mut reopened = Editor::new();
    reopened.data_dir_override = editor.data_dir_override.clone();
    reopened.kb_load_activity();

    assert_eq!(
        reopened.kb.activity, before,
        "the activity table must survive a restart intact"
    );
}

/// A clean table must not rewrite the file. Activity is touched on every node
/// read, so a save-on-every-shutdown regardless of change would be pure write
/// amplification against the user's data dir.
#[test]
fn saving_a_clean_activity_table_writes_nothing() {
    let mut editor = Editor::new();
    let tmp = with_test_dirs(&mut editor);
    let path = tmp.path().join("data").join("kb-activity.json");

    editor.kb_record_access("a");
    editor.kb_save_activity();
    let first = std::fs::metadata(&path).unwrap().modified().unwrap();

    // Nothing recorded since — the save must be a no-op.
    editor.kb_save_activity();
    let second = std::fs::metadata(&path).unwrap().modified().unwrap();

    assert_eq!(first, second, "a clean table must not be rewritten");
}

/// A corrupt table must degrade to "no activity signal", never to a failed
/// start. Ranking is cosmetic; refusing to open the editor over it would not be.
#[test]
fn a_corrupt_activity_table_degrades_instead_of_failing() {
    let mut editor = Editor::new();
    let tmp = with_test_dirs(&mut editor);
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("kb-activity.json"), "{ this is not json").unwrap();

    editor.kb_load_activity();

    assert!(
        editor.kb.activity.is_empty(),
        "a corrupt table must load as empty rather than panicking or half-loading"
    );
}
