//! R7RS §6.7: Strings.

use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_fn(
        "make-string",
        "Create string of k copies of char",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            let c = if args.len() > 1 {
                args[1].as_char()?
            } else {
                '\0'
            };
            let s: String = std::iter::repeat_n(c, k).collect();
            Ok(Value::String(Rc::from(s.as_str())))
        },
    );

    vm.register_fn(
        "string",
        "Create string from chars",
        Arity::Variadic(0),
        |args| {
            let mut s = String::with_capacity(args.len());
            for a in args {
                s.push(a.as_char()?);
            }
            Ok(Value::String(Rc::from(s.as_str())))
        },
    );

    vm.register_fn(
        "string-length",
        "Length of string",
        Arity::Fixed(1),
        |args| Ok(Value::Int(args[0].as_str()?.len() as i64)),
    );

    vm.register_fn(
        "string-ref",
        "Character at index",
        Arity::Fixed(2),
        |args| {
            let s = args[0].as_str()?;
            let k = args[1].as_int()? as usize;
            s.chars()
                .nth(k)
                .map(Value::Char)
                .ok_or_else(|| LispError::user("string-ref: index out of range", vec![]))
        },
    );

    vm.register_fn(
        "substring",
        "Extract substring",
        Arity::Variadic(2),
        |args| {
            let s = args[0].as_str()?;
            let start = args[1].as_int()? as usize;
            let end = if args.len() > 2 {
                args[2].as_int()? as usize
            } else {
                s.len()
            };
            if start > end || end > s.len() {
                return Err(LispError::user("substring: index out of range", vec![]));
            }
            // Handle UTF-8 properly via char indices
            let chars: Vec<char> = s.chars().collect();
            if start > chars.len() || end > chars.len() {
                return Err(LispError::user("substring: index out of range", vec![]));
            }
            let sub: String = chars[start..end].iter().collect();
            Ok(Value::String(Rc::from(sub.as_str())))
        },
    );

    vm.register_fn(
        "string-append",
        "Concatenate strings",
        Arity::Variadic(0),
        |args| {
            let mut result = String::new();
            for a in args {
                result.push_str(a.as_str()?);
            }
            Ok(Value::String(Rc::from(result.as_str())))
        },
    );

    // Comparison
    vm.register_fn("string=?", "String equality", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].as_str()? == args[1].as_str()?))
    });

    vm.register_fn("string<?", "String less than", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].as_str()? < args[1].as_str()?))
    });

    vm.register_fn("string>?", "String greater than", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].as_str()? > args[1].as_str()?))
    });

    vm.register_fn(
        "string<=?",
        "String less or equal",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].as_str()? <= args[1].as_str()?)),
    );

    vm.register_fn(
        "string>=?",
        "String greater or equal",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].as_str()? >= args[1].as_str()?)),
    );

    // Conversion
    vm.register_fn(
        "string->list",
        "Convert string to list of chars",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            let chars: Vec<Value> = s.chars().map(Value::Char).collect();
            Ok(Value::list(chars))
        },
    );

    vm.register_fn(
        "list->string",
        "Convert list of chars to string",
        Arity::Fixed(1),
        |args| {
            let v = args[0].to_vec()?;
            let mut s = String::with_capacity(v.len());
            for val in &v {
                s.push(val.as_char()?);
            }
            Ok(Value::String(Rc::from(s.as_str())))
        },
    );

    vm.register_fn("string-copy", "Copy a string", Arity::Fixed(1), |args| {
        let s = args[0].as_str()?;
        Ok(Value::String(Rc::from(s)))
    });

    vm.register_fn(
        "string-contains",
        "Does string contain substring?",
        Arity::Fixed(2),
        |args| {
            let haystack = args[0].as_str()?;
            let needle = args[1].as_str()?;
            Ok(Value::Bool(haystack.contains(needle)))
        },
    );

    vm.register_fn(
        "string-upcase",
        "Uppercase string",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::String(Rc::from(s.to_uppercase().as_str())))
        },
    );

    vm.register_fn(
        "string-downcase",
        "Lowercase string",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::String(Rc::from(s.to_lowercase().as_str())))
        },
    );

    vm.register_fn(
        "string-trim",
        "Trim whitespace from both ends",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::String(Rc::from(s.trim())))
        },
    );

    vm.register_fn(
        "string-split",
        "Split string by delimiter",
        Arity::Fixed(2),
        |args| {
            let s = args[0].as_str()?;
            let delim = args[1].as_str()?;
            let parts: Vec<Value> = s.split(delim).map(|p| Value::String(Rc::from(p))).collect();
            Ok(Value::list(parts))
        },
    );

    vm.register_fn(
        "string-join",
        "Join list of strings with separator",
        Arity::Fixed(2),
        |args| {
            let v = args[0].to_vec()?;
            let sep = args[1].as_str()?;
            let parts: Result<Vec<&str>, _> = v.iter().map(|v| v.as_str()).collect();
            let result = parts?.join(sep);
            Ok(Value::String(Rc::from(result.as_str())))
        },
    );

    // R7RS §6.7 string-copy!
    // Since our strings are immutable Rc<str>, string-copy! and string-set!
    // and string-fill! signal immutability errors. This is R7RS-compliant
    // since strings MAY be immutable (R7RS §6.7 allows it).

    vm.register_fn(
        "string-set!",
        "Set character (immutable strings — error)",
        Arity::Fixed(3),
        |_args| Err(LispError::immutable("string (string-set!)")),
    );

    vm.register_fn(
        "string-copy!",
        "Copy into string (immutable strings — error)",
        Arity::Variadic(3),
        |_args| Err(LispError::immutable("string (string-copy!)")),
    );

    vm.register_fn(
        "string-fill!",
        "Fill string (immutable strings — error)",
        Arity::Variadic(2),
        |_args| Err(LispError::immutable("string (string-fill!)")),
    );

    vm.register_fn(
        "string-foldcase",
        "Unicode case-fold",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            // Case folding: lowercase is a reasonable approximation for ASCII
            Ok(Value::String(Rc::from(s.to_lowercase().as_str())))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(code: &str) -> Value {
        let mut vm = Vm::new();
        crate::stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap()
    }

    #[test]
    fn test_make_string() {
        assert_eq!(eval("(make-string 3 #\\a)"), Value::String(Rc::from("aaa")));
    }

    #[test]
    fn test_string_ops() {
        assert_eq!(eval("(string-length \"hello\")"), Value::Int(5));
        assert_eq!(eval("(string-ref \"hello\" 1)"), Value::Char('e'));
        assert_eq!(
            eval("(substring \"hello\" 1 3)"),
            Value::String(Rc::from("el"))
        );
    }

    #[test]
    fn test_string_append() {
        assert_eq!(
            eval("(string-append \"hello\" \" \" \"world\")"),
            Value::String(Rc::from("hello world"))
        );
    }

    #[test]
    fn test_string_comparison() {
        assert_eq!(eval("(string=? \"abc\" \"abc\")"), Value::Bool(true));
        assert_eq!(eval("(string<? \"abc\" \"abd\")"), Value::Bool(true));
    }

    #[test]
    fn test_string_list_conversion() {
        assert_eq!(eval("(car (string->list \"abc\"))"), Value::Char('a'));
        assert_eq!(
            eval("(list->string '(#\\a #\\b #\\c))"),
            Value::String(Rc::from("abc"))
        );
    }

    #[test]
    fn test_string_case() {
        assert_eq!(
            eval("(string-upcase \"hello\")"),
            Value::String(Rc::from("HELLO"))
        );
        assert_eq!(
            eval("(string-downcase \"HELLO\")"),
            Value::String(Rc::from("hello"))
        );
    }

    #[test]
    fn test_string_contains() {
        assert_eq!(
            eval("(string-contains \"hello world\" \"world\")"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_string_split_join() {
        assert_eq!(
            eval("(car (string-split \"a,b,c\" \",\"))"),
            Value::String(Rc::from("a"))
        );
        assert_eq!(
            eval("(string-join '(\"a\" \"b\" \"c\") \",\")"),
            Value::String(Rc::from("a,b,c"))
        );
    }
}
