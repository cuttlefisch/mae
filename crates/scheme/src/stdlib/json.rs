//! `(mae json)` library — serde_json-backed JSON encode/decode.
//!
//! ## Mapping (deliberately unambiguous, matching common Scheme JSON
//! library convention, e.g. Chicken's medea):
//!
//! | JSON      | Scheme                                                     |
//! |-----------|-------------------------------------------------------------|
//! | object    | alist: proper list of `(key . value)` pairs, key always a string (so `assoc`/`assq` work directly on the result) |
//! | array     | vector (`#(...)`) — NOT a list, to disambiguate from object alists on encode |
//! | string    | string                                                     |
//! | number (integer, fits i64) | exact integer                                |
//! | number (else)              | inexact real (float)                         |
//! | true/false | `#t`/`#f`                                                  |
//! | null      | the symbol `null`                                          |
//!
//! Both `'()` (the empty list) and an empty alist look identical to
//! `json-encode` — both produce `[]`. There is deliberately no way to
//! produce `{}` from this design; use a single-entry-then-strip approach
//! or accept the asymmetry (documented here rather than special-cased
//! away).
//!
//! @stability: unstable (#521 follow-on)
//! @since: 0.14.74

use std::collections::HashMap;

use crate::library::{Library, LibraryName};
use crate::lisp_error::{Arity, LispError};
use crate::permission::tier;
use crate::value::Value;
use crate::vm::Vm;

const EXPORTS: &[&str] = &["json-encode", "json-decode"];

fn value_to_json(v: &Value) -> Result<serde_json::Value, LispError> {
    Ok(match v {
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .ok_or_else(|| LispError::user("json-encode: non-finite float", vec![]))?,
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::Symbol(s) if s.name() == "null" => serde_json::Value::Null,
        Value::Vector(items) => serde_json::Value::Array(
            items
                .borrow()
                .iter()
                .map(value_to_json)
                .collect::<Result<_, _>>()?,
        ),
        Value::Null => serde_json::Value::Array(vec![]), // empty list encodes as [] too
        Value::Pair(_) => {
            // Alist: every element must be a (key . value) pair with a string key.
            let items = v.to_list().ok_or_else(|| {
                LispError::user("json-encode: improper list is not valid JSON", vec![])
            })?;
            let mut map = serde_json::Map::new();
            for item in &items {
                let Value::Pair(cell) = item else {
                    return Err(LispError::user(
                        "json-encode: expected an alist of (key . value) pairs",
                        vec![],
                    ));
                };
                let Value::String(k) = &cell.0 else {
                    return Err(LispError::user(
                        "json-encode: alist keys must be strings",
                        vec![],
                    ));
                };
                map.insert(k.to_string(), value_to_json(&cell.1)?);
            }
            serde_json::Value::Object(map)
        }
        other => {
            return Err(LispError::type_error(
                "JSON-encodable value",
                format!("{other:?}"),
            ))
        }
    })
}

/// Convert a `serde_json::Value` into the Scheme shape `(json-decode)`
/// produces: objects become alists, arrays become vectors, null becomes the
/// symbol `null`.
///
/// `pub(crate)` so the LSP/DAP primitives (`runtime/lsp_dap.rs`) return MCP
/// tool payloads through the SAME conversion `(json-decode)` uses — one data
/// model across the Scheme surface, not a second hand-rolled mapping per
/// primitive (CLAUDE.md principle #8).
pub(crate) fn json_to_value(j: &serde_json::Value) -> Value {
    match j {
        serde_json::Value::Null => Value::symbol("null"),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
        serde_json::Value::String(s) => Value::string(s.clone()),
        serde_json::Value::Array(items) => Value::vector(items.iter().map(json_to_value).collect()),
        serde_json::Value::Object(map) => Value::list(
            map.iter()
                .map(|(k, v)| Value::cons(Value::string(k.clone()), json_to_value(v))),
        ),
    }
}

/// Register `json-encode`/`json-decode` as globals, plus the `(mae json)`
/// R7RS library facade wrapping them (mirrors `mae_async.rs`'s exact
/// facade-construction shape).
pub fn register(vm: &mut Vm) {
    vm.register_fn(
        "json-encode",
        "Encode a Scheme value as a JSON string (objects: alist of (key . value) string-keyed pairs; arrays: vectors)",
        Arity::Fixed(1), tier::PURE,
        |args: &[Value]| {
            let json = value_to_json(&args[0])?;
            serde_json::to_string(&json)
                .map(Value::string)
                .map_err(|e| LispError::user(format!("json-encode: {e}"), vec![]))
        },
    );

    vm.register_fn(
        "json-decode",
        "Decode a JSON string into a Scheme value (objects become alists, arrays become vectors, null becomes the symbol 'null)",
        Arity::Fixed(1), tier::PURE,
        |args: &[Value]| {
            let s = args[0].as_str()?;
            let parsed: serde_json::Value = serde_json::from_str(s).map_err(|e| {
                LispError::user(
                    format!("json-decode: invalid JSON: {e}"),
                    vec![s.to_string()],
                )
            })?;
            Ok(json_to_value(&parsed))
        },
    );

    let mut exports = HashMap::new();
    for name in EXPORTS {
        if let Some(val) = vm.globals.get(name) {
            exports.insert(name.to_string(), val.clone());
        }
    }
    vm.libraries.register(Library {
        name: LibraryName(vec!["mae".to_string(), "json".to_string()]),
        exports,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib;

    fn make_vm() -> Vm {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        register(&mut vm);
        vm
    }

    #[test]
    fn encode_scalars() {
        let mut vm = make_vm();
        assert_eq!(vm.eval(r#"(json-encode 42)"#).unwrap(), Value::string("42"));
        assert_eq!(
            vm.eval(r#"(json-encode "hi")"#).unwrap(),
            Value::string("\"hi\"")
        );
        assert_eq!(
            vm.eval(r#"(json-encode #t)"#).unwrap(),
            Value::string("true")
        );
        assert_eq!(
            vm.eval(r#"(json-encode 'null)"#).unwrap(),
            Value::string("null")
        );
    }

    #[test]
    fn encode_array_from_vector() {
        let mut vm = make_vm();
        assert_eq!(
            vm.eval(r#"(json-encode (vector 1 2 3))"#).unwrap(),
            Value::string("[1,2,3]")
        );
    }

    #[test]
    fn encode_object_from_alist() {
        let mut vm = make_vm();
        let result = vm
            .eval(r#"(json-encode (list (cons "a" 1) (cons "b" 2)))"#)
            .unwrap();
        // Object key order is not guaranteed (no serde_json preserve_order
        // feature anywhere in this workspace) -- decode-and-inspect
        // instead of a literal string comparison.
        let decoded: serde_json::Value = serde_json::from_str(result.as_str().unwrap()).unwrap();
        assert_eq!(decoded, serde_json::json!({"a": 1, "b": 2}));
    }

    #[test]
    fn encode_non_string_alist_key_errors() {
        let mut vm = make_vm();
        let err = vm.eval(r#"(json-encode (list (cons 1 "x")))"#).unwrap_err();
        assert!(err.message().contains("alist keys must be strings"));
    }

    #[test]
    fn decode_null_is_the_null_symbol() {
        let mut vm = make_vm();
        assert_eq!(
            vm.eval(r#"(json-decode "null")"#).unwrap(),
            Value::symbol("null")
        );
    }

    #[test]
    fn decode_array_becomes_a_vector() {
        let mut vm = make_vm();
        let result = vm.eval(r#"(json-decode "[1,2,3]")"#).unwrap();
        match result {
            Value::Vector(items) => {
                assert_eq!(
                    *items.borrow(),
                    vec![Value::Int(1), Value::Int(2), Value::Int(3)]
                );
            }
            other => panic!("expected a vector, got {other:?}"),
        }
    }

    #[test]
    fn decode_object_becomes_an_alist_lookup_works_via_assoc() {
        let mut vm = make_vm();
        let result = vm
            .eval(r#"(cdr (assoc "name" (json-decode "{\"name\":\"mae\"}")))"#)
            .unwrap();
        assert_eq!(result, Value::string("mae"));
    }

    #[test]
    fn decode_then_reencode_roundtrips_structurally() {
        let mut vm = make_vm();
        let original = r#"{"name":"mae","tags":["editor","scheme"],"stars":42}"#;
        let reencoded = vm
            .eval(&format!(
                r#"(json-encode (json-decode "{}"))"#,
                original.replace('"', "\\\"")
            ))
            .unwrap();
        let a: serde_json::Value = serde_json::from_str(original).unwrap();
        let b: serde_json::Value = serde_json::from_str(reencoded.as_str().unwrap()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn decode_invalid_json_errors_clearly() {
        let mut vm = make_vm();
        let err = vm.eval(r#"(json-decode "not json")"#).unwrap_err();
        assert!(err.message().contains("invalid JSON"));
    }

    #[test]
    fn library_importable() {
        let mut vm = make_vm();
        let result = vm.eval("(import (mae json)) (json-decode \"1\")");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Int(1));
    }
}
