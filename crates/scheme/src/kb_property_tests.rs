//! `kb-node-properties` and `kb-set-node-property!`: the Scheme surface of
//! the one property write path (`Editor::kb_set_property_target`), checked
//! against the real `Editor` after `apply_to_editor`.

use super::*;
use mae_core::Editor;

/// The node's properties as the store the primitives read holds them.
fn props(editor: &Editor, id: &str) -> std::collections::HashMap<String, String> {
    let store = editor.kb.store.as_ref().expect("a KB store");
    store.get_node(id).unwrap().expect("the node").properties
}

fn setup() -> (SchemeRuntime, Editor) {
    let rt = SchemeRuntime::new().unwrap();
    // A real editor's KB has a store; the primitives read it (`lookup_node`).
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor
        .kb_create_node("note:props", "P", "Body.", mae_kb::NodeKind::Note)
        .unwrap();
    let mut rt = rt;
    rt.inject_editor_state(&editor);
    (rt, editor)
}

/// Set, read back sorted, overwrite and remove, each through the editor.
#[test]
fn a_property_is_set_read_back_overwritten_and_removed() {
    let (mut rt, mut editor) = setup();
    rt.eval(r#"(kb-set-node-property! "note:props" "Valid-Until" "2026-12-01")"#)
        .unwrap();
    rt.eval(r#"(kb-set-node-property! "note:props" "owner" "Ñandú 😀")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let p = props(&editor, "note:props");
    assert_eq!(p.get("valid-until").map(String::as_str), Some("2026-12-01"));
    assert_eq!(p.get("owner").map(String::as_str), Some("Ñandú 😀"));

    rt.inject_editor_state(&editor);
    assert_eq!(
        rt.eval(r#"(kb-node-properties "note:props")"#).unwrap(),
        r#"(("owner" . "Ñandú 😀") ("valid-until" . "2026-12-01"))"#,
        "sorted by key, lower-case"
    );

    rt.eval(r#"(kb-set-node-property! "note:props" "valid-until" "2027-01-01")"#)
        .unwrap();
    rt.eval(r#"(kb-set-node-property! "note:props" "owner" #f)"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let p = props(&editor, "note:props");
    assert_eq!(p.get("valid-until").map(String::as_str), Some("2027-01-01"));
    assert!(!p.contains_key("owner"), "#f removes");
    assert_eq!(
        rt.eval(r#"(kb-node-properties "note:absent")"#).unwrap(),
        "#f"
    );
}

/// A refused write is refused at the call, and nothing is queued or written.
#[test]
fn a_refused_write_signals_at_once_and_changes_nothing() {
    let (mut rt, mut editor) = setup();
    for (key, value) in [
        ("id", r#""x""#),
        ("a b", r#""x""#),
        ("a:b", r#""x""#),
        ("end", r#""x""#),
        ("tags+", r#""x""#),
        ("k", r#""""#),
        ("k", r#"" padded""#),
        ("k", "\"two\\nlines\""),
        ("role", r#""boss""#),
    ] {
        let expr = format!(r#"(kb-set-node-property! "note:props" "{key}" {value})"#);
        assert!(rt.eval(&expr).is_err(), "{expr} must be refused");
    }
    assert!(
        rt.eval(r#"(kb-set-node-property! "note:absent" "k" "v")"#)
            .is_err(),
        "an unknown node"
    );
    rt.apply_to_editor(&mut editor);
    assert!(
        props(&editor, "note:props").is_empty(),
        "nothing was written"
    );
}

/// `role` goes through the same rule as `kb-set-role`: one write path.
#[test]
fn role_through_the_generic_write_matches_kb_set_role() {
    let (mut rt, mut editor) = setup();
    rt.eval(r#"(kb-set-node-property! "note:props" "role" "hub")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        props(&editor, "note:props").get("role").map(String::as_str),
        Some("hub")
    );
}
