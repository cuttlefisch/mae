//! R7RS §6.6: Characters.

use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_fn("char=?", "Character equality", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].as_char()? == args[1].as_char()?))
    });

    vm.register_fn("char<?", "Character less than", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].as_char()? < args[1].as_char()?))
    });

    vm.register_fn(
        "char>?",
        "Character greater than",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].as_char()? > args[1].as_char()?)),
    );

    vm.register_fn(
        "char<=?",
        "Character less or equal",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].as_char()? <= args[1].as_char()?)),
    );

    vm.register_fn(
        "char>=?",
        "Character greater or equal",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].as_char()? >= args[1].as_char()?)),
    );

    // Case-insensitive
    vm.register_fn(
        "char-ci=?",
        "Case-insensitive char equality",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_char()?.to_lowercase().next().unwrap();
            let b = args[1].as_char()?.to_lowercase().next().unwrap();
            Ok(Value::Bool(a == b))
        },
    );

    // Classification
    vm.register_fn(
        "char-alphabetic?",
        "Is alphabetic?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(args[0].as_char()?.is_alphabetic())),
    );

    vm.register_fn("char-numeric?", "Is numeric?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(args[0].as_char()?.is_ascii_digit()))
    });

    vm.register_fn(
        "char-whitespace?",
        "Is whitespace?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(args[0].as_char()?.is_whitespace())),
    );

    vm.register_fn(
        "char-upper-case?",
        "Is uppercase?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(args[0].as_char()?.is_uppercase())),
    );

    vm.register_fn(
        "char-lower-case?",
        "Is lowercase?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(args[0].as_char()?.is_lowercase())),
    );

    // Conversion
    vm.register_fn(
        "char-upcase",
        "Uppercase character",
        Arity::Fixed(1),
        |args| {
            let c = args[0].as_char()?;
            Ok(Value::Char(c.to_uppercase().next().unwrap_or(c)))
        },
    );

    vm.register_fn(
        "char-downcase",
        "Lowercase character",
        Arity::Fixed(1),
        |args| {
            let c = args[0].as_char()?;
            Ok(Value::Char(c.to_lowercase().next().unwrap_or(c)))
        },
    );

    vm.register_fn(
        "char->integer",
        "Character to integer",
        Arity::Fixed(1),
        |args| Ok(Value::Int(args[0].as_char()? as i64)),
    );

    vm.register_fn(
        "integer->char",
        "Integer to character",
        Arity::Fixed(1),
        |args| {
            let n = args[0].as_int()? as u32;
            char::from_u32(n)
                .map(Value::Char)
                .ok_or_else(|| LispError::user("integer->char: invalid Unicode scalar", vec![]))
        },
    );

    vm.register_fn(
        "digit-value",
        "Numeric value of digit character",
        Arity::Fixed(1),
        |args| {
            let c = args[0].as_char()?;
            match c.to_digit(10) {
                Some(d) => Ok(Value::Int(d as i64)),
                None => Ok(Value::Bool(false)),
            }
        },
    );

    // String conversion
    vm.register_fn(
        "char->string",
        "Character to string",
        Arity::Fixed(1),
        |args| {
            let c = args[0].as_char()?;
            Ok(Value::String(Rc::from(c.to_string().as_str())))
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
    fn test_char_comparison() {
        assert_eq!(eval("(char=? #\\a #\\a)"), Value::Bool(true));
        assert_eq!(eval("(char<? #\\a #\\b)"), Value::Bool(true));
        assert_eq!(eval("(char>? #\\b #\\a)"), Value::Bool(true));
    }

    #[test]
    fn test_char_classification() {
        assert_eq!(eval("(char-alphabetic? #\\a)"), Value::Bool(true));
        assert_eq!(eval("(char-numeric? #\\5)"), Value::Bool(true));
        assert_eq!(eval("(char-whitespace? #\\space)"), Value::Bool(true));
        assert_eq!(eval("(char-upper-case? #\\A)"), Value::Bool(true));
        assert_eq!(eval("(char-lower-case? #\\a)"), Value::Bool(true));
    }

    #[test]
    fn test_char_conversion() {
        assert_eq!(eval("(char-upcase #\\a)"), Value::Char('A'));
        assert_eq!(eval("(char-downcase #\\A)"), Value::Char('a'));
        assert_eq!(eval("(char->integer #\\A)"), Value::Int(65));
        assert_eq!(eval("(integer->char 65)"), Value::Char('A'));
    }
}
