use std::collections::HashMap;

use proptest::prelude::*;

use super::{validate, PropertyRefusal};
use crate::export::node_to_org;
use crate::{Node, NodeKind};

fn node_with(props: &HashMap<String, String>) -> Node {
    let mut n = Node::new("note:p", "P", NodeKind::Note, "Body.\n");
    n.properties = props.clone();
    n
}

/// What each reader makes of the node exported with `props`.
fn read_back(
    props: &HashMap<String, String>,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let text = node_to_org(&node_with(props));
    let legacy = crate::org::parse_org(&text)
        .expect("the export parses")
        .properties;
    let ingested = crate::org_ingest::parse_nodes(&text)
        .into_iter()
        .find(|n| n.id == "note:p")
        .expect("ingest finds the node")
        .properties;
    (legacy, ingested)
}

/// A key the validator should accept or refuse: printable ASCII, sometimes
/// with the characters and spellings the rules are about.
fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        "[A-Za-z0-9_.-]{1,12}",
        "[ -~]{0,12}",
        Just("id".to_string()),
        Just("Kind".to_string()),
        Just("END".to_string()),
        Just("properties".to_string()),
        Just("tags+".to_string()),
        Just("ñandú".to_string()),
        Just("a:b".to_string()),
    ]
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        "[^\\n\\r]{0,24}",
        "\\PC{1,16}",
        Just(" padded".to_string()),
        Just("two\nlines".to_string()),
        Just("ok: with colon".to_string()),
        Just("Ñandú 😀 日本".to_string()),
        Just(String::new()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Every write the validator accepts comes back unchanged through BOTH
    /// readers, under the key it returned. This is what the rules are for.
    #[test]
    fn an_accepted_property_survives_the_round_trip(
        pairs in prop::collection::vec((key(), value()), 1..6)
    ) {
        let mut props = HashMap::new();
        for (k, v) in &pairs {
            if k == "role" { continue; } // its value set is checked below
            if let Ok(stored) = validate(k, Some(v)) {
                props.insert(stored, v.clone());
            }
        }
        let (legacy, ingested) = read_back(&props);
        prop_assert_eq!(&legacy, &props, "parse_org");
        prop_assert_eq!(&ingested, &props, "mae-org ingest");
    }
}

#[test]
fn each_refusal_names_its_reason() {
    let r = |k: &str, v: Option<&str>| validate(k, v).unwrap_err();
    assert_eq!(r("", Some("x")), PropertyRefusal::EmptyKey);
    assert_eq!(r("a b", Some("x")), PropertyRefusal::KeyCharacter(' '));
    assert_eq!(r("a:b", Some("x")), PropertyRefusal::KeyCharacter(':'));
    assert_eq!(r("ñ", Some("x")), PropertyRefusal::KeyCharacter('ñ'));
    for k in ["ID", "kind", "Aliases", "END", "PROPERTIES"] {
        assert!(
            matches!(r(k, Some("x")), PropertyRefusal::ReservedKey(_)),
            "{k}"
        );
    }
    assert_eq!(r("tags+", Some("x")), PropertyRefusal::KeyAccumulates);
    assert_eq!(r("k", Some("")), PropertyRefusal::EmptyValue);
    assert_eq!(r("k", Some("a\nb")), PropertyRefusal::MultilineValue);
    assert_eq!(r("k", Some("a\r")), PropertyRefusal::MultilineValue);
    assert_eq!(r("k", Some(" a")), PropertyRefusal::ValueBlankEdge);
    assert_eq!(r("k", Some("a\u{a0}")), PropertyRefusal::ValueBlankEdge);
    assert_eq!(
        r("ROLE", Some("boss")),
        PropertyRefusal::UnknownRole("boss".into())
    );
}

#[test]
fn an_accepted_key_is_stored_lower_case_and_a_removal_needs_no_value() {
    assert_eq!(
        validate("Valid-Until", Some("2026-12-01")).unwrap(),
        "valid-until"
    );
    assert_eq!(validate("Role", Some("hub")).unwrap(), "role");
    assert_eq!(validate("anything_ok", None).unwrap(), "anything_ok");
    // A removal still refuses what is not a property.
    assert!(validate("id", None).is_err());
}
