//! ADR-109 D6: the editor's own cap on a node's body (`kb_max_node_bytes`).
//!
//! The daemon refuses an update past its (larger) limit, but by then the
//! author's copy already holds the edit and would re-send it on every
//! reconnect. So an over-cap edit has to be stopped in the editor, before it
//! becomes a CRDT op — and stopped without losing what was there.

use super::*;

fn editor_with_cap(cap: &str) -> (Editor, TempDir) {
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    editor.set_option("kb_max_node_bytes", cap).unwrap();
    (editor, dirs)
}

fn body_of(editor: &Editor, id: &str) -> Option<usize> {
    editor.kb.primary.get(id).map(|n| n.body.len())
}

#[test]
fn an_edit_growing_a_node_past_the_cap_is_refused_and_changes_nothing() {
    let (mut editor, _dirs) = editor_with_cap("100");
    editor
        .kb_create_node("size:a", "A", &"a".repeat(50), mae_kb::NodeKind::Note)
        .unwrap();

    let err = editor
        .kb_update_node("size:a", None, Some(&"b".repeat(101)), None)
        .unwrap_err();
    assert!(err.contains("kb-max-node-bytes"), "{err}");
    assert_eq!(
        body_of(&editor, "size:a"),
        Some(50),
        "the refused edit was applied"
    );

    // Up to the cap is fine; bytes, not characters: 26 four-byte chars = 104.
    editor
        .kb_update_node("size:a", None, Some(&"c".repeat(100)), None)
        .unwrap();
    assert!(editor
        .kb_update_node("size:a", None, Some(&"\u{1F600}".repeat(26)), None)
        .is_err());
}

#[test]
fn a_node_already_past_the_cap_may_shrink_but_not_grow() {
    let (mut editor, _dirs) = editor_with_cap("0");
    editor
        .kb_create_node("size:b", "B", &"a".repeat(200), mae_kb::NodeKind::Note)
        .unwrap();
    editor.set_option("kb_max_node_bytes", "100").unwrap();

    editor
        .kb_update_node("size:b", None, Some(&"a".repeat(150)), None)
        .unwrap();
    assert_eq!(body_of(&editor, "size:b"), Some(150));
    // Unrelated fields of an oversized node can still change (same body size).
    editor
        .kb_update_node("size:b", Some("Renamed"), None, None)
        .unwrap();
    assert!(editor
        .kb_update_node("size:b", None, Some(&"a".repeat(151)), None)
        .is_err());
    assert_eq!(body_of(&editor, "size:b"), Some(150));
}

#[test]
fn creating_a_node_past_the_cap_is_refused() {
    let (mut editor, _dirs) = editor_with_cap("100");
    let err = editor
        .kb_create_node("size:c", "C", &"a".repeat(101), mae_kb::NodeKind::Note)
        .unwrap_err();
    assert!(err.contains("kb-max-node-bytes"), "{err}");
    assert_eq!(body_of(&editor, "size:c"), None, "the refused node exists");
}

#[test]
fn zero_means_unlimited_and_the_option_rejects_non_numbers() {
    let (mut editor, _dirs) = editor_with_cap("0");
    editor
        .kb_create_node(
            "size:d",
            "D",
            &"a".repeat(2_000_000),
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    assert!(editor.set_option("kb_max_node_bytes", "lots").is_err());
    assert_eq!(editor.get_option("kb_max_node_bytes").unwrap().0, "0");
}

/// The default is 1 MiB: about three times the largest node measured across
/// real KBs (320 KB), so no real note is refused, while a node stays cheap
/// enough to parse and render on every open (~200 ms in the browser build at
/// 1 MiB, against ~2 s at the daemon's 10 MB backstop).
#[test]
fn the_default_cap_admits_real_large_notes_and_stops_runaway_ones() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    assert_eq!(editor.get_option("kb_max_node_bytes").unwrap().0, "1048576");
    editor
        .kb_create_node("size:e", "E", &"a".repeat(400_000), mae_kb::NodeKind::Note)
        .unwrap();
    assert!(editor
        .kb_create_node(
            "size:f",
            "F",
            &"a".repeat(1_048_577),
            mae_kb::NodeKind::Note
        )
        .is_err());
}
