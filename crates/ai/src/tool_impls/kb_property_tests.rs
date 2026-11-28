//! `kb_set_property` and the `properties` `kb_get` returns: the MCP surface
//! of the one property write path, at parity with `kb-set-node-property!`.

use super::*;
use mae_core::Editor;

fn editor() -> Editor {
    let mut e = Editor::new();
    e.kb_create_node("note:mcp-props", "P", "Body.", mae_kb::NodeKind::Note)
        .unwrap();
    e
}

fn get_props(e: &Editor) -> serde_json::Value {
    let out = execute_kb_get(e, &serde_json::json!({"id": "note:mcp-props"})).unwrap();
    serde_json::from_str::<serde_json::Value>(&out).unwrap()["properties"].clone()
}

fn set(e: &mut Editor, args: serde_json::Value) -> Result<String, String> {
    let mut a = serde_json::json!({"id": "note:mcp-props"});
    a.as_object_mut()
        .unwrap()
        .extend(args.as_object().unwrap().clone());
    execute_kb_set_property(e, &a)
}

/// Set (key stored lower-case), read back through kb_get in key order,
/// overwrite, remove.
#[test]
fn kb_set_property_sets_reads_back_and_removes() {
    let mut e = editor();
    set(&mut e, serde_json::json!({"key": "Zeta", "value": "last"})).unwrap();
    set(
        &mut e,
        serde_json::json!({"key": "valid-until", "value": "2026-12-01"}),
    )
    .unwrap();
    let props = get_props(&e);
    assert_eq!(props["zeta"], "last");
    assert_eq!(props["valid-until"], "2026-12-01");
    let keys: Vec<&String> = props.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["valid-until", "zeta"], "in key order");

    set(&mut e, serde_json::json!({"key": "zeta", "value": "again"})).unwrap();
    set(&mut e, serde_json::json!({"key": "valid-until"})).unwrap();
    let props = get_props(&e);
    assert_eq!(props["zeta"], "again");
    assert!(props.get("valid-until").is_none(), "omitting value removes");
}

/// Each refusal reaches the caller and writes nothing.
#[test]
fn kb_set_property_refuses_and_writes_nothing() {
    let mut e = editor();
    for args in [
        serde_json::json!({"key": "ID", "value": "x"}),
        serde_json::json!({"key": "a b", "value": "x"}),
        serde_json::json!({"key": "end", "value": "x"}),
        serde_json::json!({"key": "k", "value": ""}),
        serde_json::json!({"key": "k", "value": "two\nlines"}),
        serde_json::json!({"key": "k", "value": " padded"}),
        serde_json::json!({"key": "k", "value": 7}),
        serde_json::json!({"key": "role", "value": "boss"}),
        serde_json::json!({"value": "no key"}),
    ] {
        assert!(set(&mut e, args.clone()).is_err(), "{args} must be refused");
    }
    let err = execute_kb_set_property(
        &mut e,
        &serde_json::json!({"id": "note:absent", "key": "k", "value": "v"}),
    )
    .unwrap_err();
    assert!(err.contains("No KB node"), "{err}");
    assert_eq!(get_props(&e), serde_json::json!({}), "nothing written");
}

/// `role` through the generic tool is `kb_set_role`: one rule, one path.
#[test]
fn role_through_kb_set_property_is_kb_set_role() {
    let mut e = editor();
    set(&mut e, serde_json::json!({"key": "role", "value": "hub"})).unwrap();
    assert_eq!(get_props(&e)["role"], "hub");
    let err = execute_kb_set_role(
        &mut e,
        &serde_json::json!({"id": "note:mcp-props", "role": "boss"}),
    )
    .unwrap_err();
    assert!(err.contains("Invalid role"), "{err}");
    assert_eq!(get_props(&e)["role"], "hub", "the refusal changed nothing");
}
