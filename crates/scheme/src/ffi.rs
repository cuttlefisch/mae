//! FFI helpers for mae-scheme foreign function argument extraction.
//!
//! Provides type-checked extraction from `&[Value]` slices, converting
//! mae-scheme Values to Rust types with proper error messages.
//!
//! @stability: unstable (Phase 13e)
//! @since: 0.12.0

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;

/// Extract a string argument at the given index.
pub fn arg_string(args: &[Value], idx: usize, fn_name: &str) -> Result<String, LispError> {
    match args.get(idx) {
        Some(Value::String(s)) => Ok(s.to_string()),
        Some(Value::Symbol(s)) => Ok(s.name().to_string()),
        Some(other) => Err(LispError::type_error(
            "string",
            format!("{} got {:?}", fn_name, other),
        )),
        None => Err(LispError::arity(
            fn_name,
            Arity::Variadic(idx + 1),
            args.len(),
        )),
    }
}

/// Extract an integer argument at the given index.
pub fn arg_int(args: &[Value], idx: usize, fn_name: &str) -> Result<i64, LispError> {
    match args.get(idx) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Float(f)) => Ok(*f as i64),
        Some(other) => Err(LispError::type_error(
            "integer",
            format!("{} got {:?}", fn_name, other),
        )),
        None => Err(LispError::arity(
            fn_name,
            Arity::Variadic(idx + 1),
            args.len(),
        )),
    }
}

/// Extract a float argument at the given index.
pub fn arg_float(args: &[Value], idx: usize, fn_name: &str) -> Result<f64, LispError> {
    match args.get(idx) {
        Some(Value::Float(f)) => Ok(*f),
        Some(Value::Int(n)) => Ok(*n as f64),
        Some(other) => Err(LispError::type_error(
            "number",
            format!("{} got {:?}", fn_name, other),
        )),
        None => Err(LispError::arity(
            fn_name,
            Arity::Variadic(idx + 1),
            args.len(),
        )),
    }
}

/// Extract a boolean argument at the given index.
pub fn arg_bool(args: &[Value], idx: usize, fn_name: &str) -> Result<bool, LispError> {
    match args.get(idx) {
        Some(Value::Bool(b)) => Ok(*b),
        Some(other) => Err(LispError::type_error(
            "boolean",
            format!("{} got {:?}", fn_name, other),
        )),
        None => Err(LispError::arity(
            fn_name,
            Arity::Variadic(idx + 1),
            args.len(),
        )),
    }
}

/// Extract an optional string argument at the given index.
/// Returns `None` if the argument is missing, `#f`, or `void`.
pub fn arg_opt_string(args: &[Value], idx: usize, _fn_name: &str) -> Option<String> {
    match args.get(idx) {
        Some(Value::String(s)) => Some(s.to_string()),
        Some(Value::Bool(false)) | Some(Value::Void) | None => None,
        Some(Value::Symbol(s)) => Some(s.name().to_string()),
        _ => None,
    }
}

/// Extract an optional boolean argument (default: false).
pub fn arg_opt_bool(args: &[Value], idx: usize) -> bool {
    match args.get(idx) {
        Some(v) => v.is_true(),
        None => false,
    }
}

/// Convert a Scheme list of strings to a Vec<String>.
pub fn list_to_strings(val: &Value) -> Vec<String> {
    let mut result = Vec::new();
    let mut cur = val.clone();
    loop {
        match cur {
            Value::Null => break,
            Value::Pair(p) => {
                if let Value::String(s) = &p.0 {
                    result.push(s.to_string());
                } else if let Value::Symbol(s) = &p.0 {
                    result.push(s.name().to_string());
                }
                cur = p.1.clone();
            }
            _ => break,
        }
    }
    result
}

/// Convert a mae-scheme Value to a display string (for eval results).
pub fn value_to_display(val: &Value) -> String {
    match val {
        Value::Void => String::new(),
        Value::Bool(b) => if *b { "#t" } else { "#f" }.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => format!("{}", n),
        Value::String(s) => s.to_string(),
        Value::Char(c) => format!("#\\{}", c),
        Value::Null => "()".to_string(),
        Value::Symbol(s) => s.name().to_string(),
        Value::Pair(_) => format!("{}", val),
        Value::Vector(_) => format!("{}", val),
        Value::Eof => "#<eof>".to_string(),
        Value::Undefined => "#<undefined>".to_string(),
        _ => format!("{}", val),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arg_string() {
        let args = vec![Value::string("hello"), Value::Int(42)];
        assert_eq!(arg_string(&args, 0, "test").unwrap(), "hello");
        assert!(arg_string(&args, 1, "test").is_err()); // Int, not string
        assert!(arg_string(&args, 2, "test").is_err()); // Missing
    }

    #[test]
    fn test_arg_int() {
        let args = vec![Value::Int(42), Value::Float(3.5)];
        assert_eq!(arg_int(&args, 0, "test").unwrap(), 42);
        assert_eq!(arg_int(&args, 1, "test").unwrap(), 3); // Float truncated
    }

    #[test]
    fn test_arg_opt_string() {
        let args = vec![Value::string("hi"), Value::Bool(false)];
        assert_eq!(arg_opt_string(&args, 0, "test"), Some("hi".to_string()));
        assert_eq!(arg_opt_string(&args, 1, "test"), None);
        assert_eq!(arg_opt_string(&args, 5, "test"), None);
    }

    #[test]
    fn test_list_to_strings() {
        let list = Value::list(vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]);
        assert_eq!(list_to_strings(&list), vec!["a", "b", "c"]);
        assert_eq!(list_to_strings(&Value::Null), Vec::<String>::new());
    }

    #[test]
    fn test_value_to_display() {
        assert_eq!(value_to_display(&Value::Void), "");
        assert_eq!(value_to_display(&Value::Bool(true)), "#t");
        assert_eq!(value_to_display(&Value::Int(42)), "42");
        assert_eq!(value_to_display(&Value::string("hi")), "hi");
    }
}
