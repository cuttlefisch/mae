//! What a re-import tells its caller. A client deciding whether its own edit
//! landed reads this result; before these fields it could not tell an
//! incremental run that re-parsed the changed file from one that did nothing
//! (both said `nodes_imported: 0`), nor whether this editor watches the KB.

use super::*;

fn json(result: &KbImportResult) -> serde_json::Value {
    serde_json::from_str(&result.to_json()).expect("to_json emits valid JSON")
}

/// An edit to one known file: incremental re-parses exactly that file, says
/// so, and counts its node as rewritten — distinguishable from a no-op run.
#[test]
fn an_incremental_reimport_reports_which_files_it_reparsed() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let dir = create_test_org_dir();
    editor.kb_register("TestNotes", dir.path()).unwrap();

    let idle = editor
        .kb_reimport("TestNotes", Some(mae_kb::IngestMode::Incremental))
        .expect("reimport");
    let idle = json(&idle);
    assert_eq!(
        idle["mode"], "incremental",
        "same spelling as the `mode` argument"
    );
    assert_eq!(idle["files_reparsed"], 0, "nothing changed: {idle}");
    assert_eq!(idle["nodes_rewritten"], 0, "{idle}");

    std::fs::write(
        dir.path().join("note1.org"),
        ":PROPERTIES:\n:ID: test-note-1\n:END:\n#+title: Note One\n\nEdited body.\n",
    )
    .unwrap();
    let changed = editor
        .kb_reimport("TestNotes", Some(mae_kb::IngestMode::Incremental))
        .expect("reimport");
    let changed = json(&changed);
    assert_eq!(changed["files_reparsed"], 1, "{changed}");
    assert_eq!(changed["nodes_rewritten"], 1, "{changed}");
    assert_eq!(
        changed["nodes_imported"], 0,
        "the node is not new: {changed}"
    );
    assert!(
        changed["files_seen"].as_u64().unwrap() >= 3,
        "every .org file in the dir is seen: {changed}"
    );
}

/// The editor that registered a KB watches it (when watching is on), and the
/// result says which; a client uses this to decide whether to re-import.
#[test]
fn the_result_says_whether_this_editor_watches_the_kb() {
    for enabled in [true, false] {
        let mut editor = Editor::new();
        let _dirs = with_test_dirs(&mut editor);
        editor.kb.watcher_enabled = enabled;
        let dir = create_test_org_dir();
        editor.kb_register("TestNotes", dir.path()).unwrap();
        let r = editor.kb_reimport("TestNotes", None).expect("reimport");
        assert_eq!(json(&r)["watched"], enabled, "watcher_enabled={enabled}");
    }
}
