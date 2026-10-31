//! The facade's JSON reader against serde_json: the same values for every
//! document serde_json writes, and the same verdict (accept or refuse) on
//! JSON-shaped noise. Differences are by design and excluded explicitly:
//! a lone surrogate escape (U+FFFD here, as `TextEncoder` encodes one; an
//! error in serde_json) and nesting past MAX_DEPTH (refused here first).

use mae_org_wasm::json::{parse, Json, MAX_DEPTH};
use proptest::prelude::*;
use serde_json::Value;

fn same(a: &Json, b: &Value) -> bool {
    match (a, b) {
        (Json::Null, Value::Null) => true,
        (Json::Bool(x), Value::Bool(y)) => x == y,
        (Json::Num(x), Value::Number(y)) => y.as_f64().is_some_and(|y| y == *x),
        (Json::Str(x), Value::String(y)) => x == y,
        (Json::Arr(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        (Json::Obj(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => false,
    }
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        any::<String>().prop_map(Value::String),
    ];
    leaf.prop_recursive(6, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(any::<String>(), inner, 0..8)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

fn noise() -> impl Strategy<Value = String> {
    let atoms = prop::sample::select(vec![
        "{", "}", "[", "]", "\"", ",", ":", "0", "1", "-", ".", "e", "E", "+", "true", "false",
        "null", " ", "\\", "\\n", "\\u00e9", "\"a\"", "01", "1.", ".5", "\t", "\u{1}", "é",
    ]);
    prop::collection::vec(atoms, 0..16).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn reads_every_document_serde_json_writes(v in value()) {
        for text in [serde_json::to_string(&v).unwrap(), serde_json::to_string_pretty(&v).unwrap()] {
            let ours = parse(&text);
            prop_assert!(ours.as_ref().is_ok_and(|o| same(o, &v)), "{text}: {ours:?}");
        }
    }

    #[test]
    fn accepts_and_refuses_as_serde_json_does(text in noise()) {
        let theirs = serde_json::from_str::<Value>(&text).is_ok();
        prop_assert_eq!(parse(&text).is_ok(), theirs, "{:?}", text);
    }
}

#[test]
fn the_intended_differences_are_pinned() {
    // A lone surrogate is U+FFFD; a pair is its character.
    assert_eq!(parse(r#""\ud800x""#), Ok(Json::Str("\u{fffd}x".into())));
    assert_eq!(parse(r#""😀""#), Ok(Json::Str("😀".into())));
    // Nesting past the limit is refused, so no request can exhaust the stack.
    let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
    assert!(parse(&deep).is_err());
    let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
    assert!(parse(&ok).is_ok());
    let hostile = "[".repeat(1_000_000);
    assert!(parse(&hostile).is_err());
    // A repeated key keeps its last value in its first place.
    assert_eq!(
        parse(r#"{"a":1,"b":2,"a":3}"#),
        Ok(Json::Obj(vec![
            ("a".into(), Json::Num(3.0)),
            ("b".into(), Json::Num(2.0))
        ]))
    );
}
