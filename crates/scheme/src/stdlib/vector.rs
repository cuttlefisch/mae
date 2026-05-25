//! R7RS §6.8-6.9: Vectors and bytevectors.

use std::cell::RefCell;
use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    register_vectors(vm);
    register_bytevectors(vm);
}

fn register_vectors(vm: &mut Vm) {
    vm.register_fn(
        "make-vector",
        "Create vector of k elements",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            let fill = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Undefined
            };
            Ok(Value::Vector(Rc::new(RefCell::new(vec![fill; k]))))
        },
    );

    vm.register_fn(
        "vector",
        "Create vector from arguments",
        Arity::Variadic(0),
        |args| Ok(Value::vector(args.to_vec())),
    );

    vm.register_fn(
        "vector-length",
        "Length of vector",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Vector(v) => Ok(Value::Int(v.borrow().len() as i64)),
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "vector-ref",
        "Element at index",
        Arity::Fixed(2),
        |args| match &args[0] {
            Value::Vector(v) => {
                let k = args[1].as_int()? as usize;
                let vec = v.borrow();
                vec.get(k)
                    .cloned()
                    .ok_or_else(|| LispError::user("vector-ref: index out of range", vec![]))
            }
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "vector-set!",
        "Set element at index",
        Arity::Fixed(3),
        |args| match &args[0] {
            Value::Vector(v) => {
                let k = args[1].as_int()? as usize;
                let mut vec = v.borrow_mut();
                if k >= vec.len() {
                    return Err(LispError::user("vector-set!: index out of range", vec![]));
                }
                vec[k] = args[2].clone();
                Ok(Value::Void)
            }
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "vector->list",
        "Convert vector to list",
        Arity::Variadic(1),
        |args| match &args[0] {
            Value::Vector(v) => {
                let vec = v.borrow();
                let start = if args.len() > 1 {
                    args[1].as_int()? as usize
                } else {
                    0
                };
                let end = if args.len() > 2 {
                    args[2].as_int()? as usize
                } else {
                    vec.len()
                };
                Ok(Value::list(vec[start..end].to_vec()))
            }
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "list->vector",
        "Convert list to vector",
        Arity::Fixed(1),
        |args| {
            let v = args[0].to_vec()?;
            Ok(Value::vector(v))
        },
    );

    vm.register_fn(
        "vector-fill!",
        "Fill vector with value",
        Arity::Fixed(2),
        |args| match &args[0] {
            Value::Vector(v) => {
                let fill = args[1].clone();
                let mut vec = v.borrow_mut();
                for elem in vec.iter_mut() {
                    *elem = fill.clone();
                }
                Ok(Value::Void)
            }
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "vector-copy",
        "Copy vector",
        Arity::Variadic(1),
        |args| match &args[0] {
            Value::Vector(v) => {
                let vec = v.borrow();
                let start = if args.len() > 1 {
                    args[1].as_int()? as usize
                } else {
                    0
                };
                let end = if args.len() > 2 {
                    args[2].as_int()? as usize
                } else {
                    vec.len()
                };
                Ok(Value::vector(vec[start..end].to_vec()))
            }
            _ => Err(LispError::type_error("vector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "vector-append",
        "Concatenate vectors",
        Arity::Variadic(0),
        |args| {
            let mut result = Vec::new();
            for a in args {
                match a {
                    Value::Vector(v) => result.extend(v.borrow().iter().cloned()),
                    _ => return Err(LispError::type_error("vector", format!("{a}"))),
                }
            }
            Ok(Value::vector(result))
        },
    );

    // vector-copy!: mutate target vector in-place
    vm.register_fn(
        "vector-copy!",
        "Copy elements into vector",
        Arity::Variadic(3),
        |args| {
            let to = match &args[0] {
                Value::Vector(v) => v.clone(),
                _ => return Err(LispError::type_error("vector", format!("{}", args[0]))),
            };
            let at = args[1].as_int()? as usize;
            let from = match &args[2] {
                Value::Vector(v) => v.borrow().clone(),
                _ => return Err(LispError::type_error("vector", format!("{}", args[2]))),
            };
            let start = if args.len() > 3 {
                args[3].as_int()? as usize
            } else {
                0
            };
            let end = if args.len() > 4 {
                args[4].as_int()? as usize
            } else {
                from.len()
            };
            let mut to_vec = to.borrow_mut();
            for (i, j) in (start..end).enumerate() {
                if at + i < to_vec.len() && j < from.len() {
                    to_vec[at + i] = from[j].clone();
                }
            }
            Ok(Value::Void)
        },
    );

    // vector->string and string->vector
    vm.register_fn(
        "vector->string",
        "Convert char vector to string",
        Arity::Variadic(1),
        |args| {
            let v = match &args[0] {
                Value::Vector(v) => v.borrow().clone(),
                _ => return Err(LispError::type_error("vector", format!("{}", args[0]))),
            };
            let start = if args.len() > 1 {
                args[1].as_int()? as usize
            } else {
                0
            };
            let end = if args.len() > 2 {
                args[2].as_int()? as usize
            } else {
                v.len()
            };
            let s: Result<String, _> = v[start..end].iter().map(|c| c.as_char()).collect();
            Ok(Value::String(Rc::from(s?.as_str())))
        },
    );

    vm.register_fn(
        "string->vector",
        "Convert string to char vector",
        Arity::Variadic(1),
        |args| {
            let s = args[0].as_str()?;
            let start = if args.len() > 1 {
                args[1].as_int()? as usize
            } else {
                0
            };
            let chars: Vec<char> = s.chars().collect();
            let end = if args.len() > 2 {
                args[2].as_int()? as usize
            } else {
                chars.len()
            };
            let result: Vec<Value> = chars[start..end].iter().map(|c| Value::Char(*c)).collect();
            Ok(Value::vector(result))
        },
    );
}

fn register_bytevectors(vm: &mut Vm) {
    vm.register_fn(
        "make-bytevector",
        "Create bytevector of k bytes",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            let fill = if args.len() > 1 {
                args[1].as_int()? as u8
            } else {
                0
            };
            Ok(Value::Bytevector(Rc::new(RefCell::new(vec![fill; k]))))
        },
    );

    vm.register_fn(
        "bytevector",
        "Create bytevector from bytes",
        Arity::Variadic(0),
        |args| {
            let mut bytes = Vec::with_capacity(args.len());
            for a in args {
                bytes.push(a.as_int()? as u8);
            }
            Ok(Value::bytevector(bytes))
        },
    );

    vm.register_fn(
        "bytevector-length",
        "Length of bytevector",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => Ok(Value::Int(bv.borrow().len() as i64)),
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "bytevector-u8-ref",
        "Byte at index",
        Arity::Fixed(2),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let k = args[1].as_int()? as usize;
                let vec = bv.borrow();
                vec.get(k)
                    .map(|b| Value::Int(*b as i64))
                    .ok_or_else(|| LispError::user("bytevector-u8-ref: index out of range", vec![]))
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "bytevector-u8-set!",
        "Set byte at index",
        Arity::Fixed(3),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let k = args[1].as_int()? as usize;
                let byte = args[2].as_int()? as u8;
                let mut vec = bv.borrow_mut();
                if k >= vec.len() {
                    return Err(LispError::user(
                        "bytevector-u8-set!: index out of range",
                        vec![],
                    ));
                }
                vec[k] = byte;
                Ok(Value::Void)
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "bytevector-copy",
        "Copy bytevector",
        Arity::Variadic(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let vec = bv.borrow();
                let start = if args.len() > 1 {
                    args[1].as_int()? as usize
                } else {
                    0
                };
                let end = if args.len() > 2 {
                    args[2].as_int()? as usize
                } else {
                    vec.len()
                };
                Ok(Value::bytevector(vec[start..end].to_vec()))
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "bytevector-append",
        "Concatenate bytevectors",
        Arity::Variadic(0),
        |args| {
            let mut result = Vec::new();
            for a in args {
                match a {
                    Value::Bytevector(bv) => result.extend(bv.borrow().iter()),
                    _ => return Err(LispError::type_error("bytevector", format!("{a}"))),
                }
            }
            Ok(Value::bytevector(result))
        },
    );

    vm.register_fn(
        "utf8->string",
        "Decode bytevector as UTF-8",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let bytes = bv.borrow();
                let s = std::str::from_utf8(&bytes)
                    .map_err(|_| LispError::user("utf8->string: invalid UTF-8", vec![]))?;
                Ok(Value::String(Rc::from(s)))
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "string->utf8",
        "Encode string as UTF-8 bytevector",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::bytevector(s.as_bytes().to_vec()))
        },
    );

    // bytevector-copy!
    vm.register_fn(
        "bytevector-copy!",
        "Copy bytes into bytevector",
        Arity::Variadic(3),
        |args| {
            let to = match &args[0] {
                Value::Bytevector(bv) => bv.clone(),
                _ => return Err(LispError::type_error("bytevector", format!("{}", args[0]))),
            };
            let at = args[1].as_int()? as usize;
            let from = match &args[2] {
                Value::Bytevector(bv) => bv.borrow().clone(),
                _ => return Err(LispError::type_error("bytevector", format!("{}", args[2]))),
            };
            let start = if args.len() > 3 {
                args[3].as_int()? as usize
            } else {
                0
            };
            let end = if args.len() > 4 {
                args[4].as_int()? as usize
            } else {
                from.len()
            };
            let mut to_vec = to.borrow_mut();
            for (i, j) in (start..end).enumerate() {
                if at + i < to_vec.len() && j < from.len() {
                    to_vec[at + i] = from[j];
                }
            }
            Ok(Value::Void)
        },
    );

    // bytevector->list and list->bytevector
    vm.register_fn(
        "bytevector->list",
        "Convert bytevector to list of integers",
        Arity::Variadic(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let vec = bv.borrow();
                let start = if args.len() > 1 {
                    args[1].as_int()? as usize
                } else {
                    0
                };
                let end = if args.len() > 2 {
                    args[2].as_int()? as usize
                } else {
                    vec.len()
                };
                let items: Vec<Value> = vec[start..end]
                    .iter()
                    .map(|b| Value::Int(*b as i64))
                    .collect();
                Ok(Value::list(items))
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "list->bytevector",
        "Convert list of integers to bytevector",
        Arity::Fixed(1),
        |args| {
            let elems = args[0].to_vec()?;
            let mut bytes = Vec::with_capacity(elems.len());
            for e in &elems {
                bytes.push(e.as_int()? as u8);
            }
            Ok(Value::bytevector(bytes))
        },
    );

    // vector? and bytevector? predicates
    vm.register_fn("vector?", "Is vector?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Vector(_))))
    });

    vm.register_fn("bytevector?", "Is bytevector?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Bytevector(_))))
    });
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
    fn test_vector_ops() {
        assert_eq!(eval("(vector-length (vector 1 2 3))"), Value::Int(3));
        assert_eq!(eval("(vector-ref (vector 10 20 30) 1)"), Value::Int(20));
    }

    #[test]
    fn test_vector_mutation() {
        assert_eq!(
            eval("(let ((v (vector 1 2 3))) (vector-set! v 1 99) (vector-ref v 1))"),
            Value::Int(99)
        );
    }

    #[test]
    fn test_vector_conversion() {
        assert_eq!(eval("(car (vector->list (vector 1 2 3)))"), Value::Int(1));
        assert_eq!(
            eval("(vector-ref (list->vector '(10 20 30)) 2)"),
            Value::Int(30)
        );
    }

    #[test]
    fn test_make_vector() {
        assert_eq!(eval("(vector-ref (make-vector 3 0) 2)"), Value::Int(0));
    }

    #[test]
    fn test_bytevector_ops() {
        assert_eq!(
            eval("(bytevector-length (bytevector 1 2 3))"),
            Value::Int(3)
        );
        assert_eq!(
            eval("(bytevector-u8-ref (bytevector 10 20 30) 1)"),
            Value::Int(20)
        );
    }

    #[test]
    fn test_bytevector_mutation() {
        assert_eq!(
            eval("(let ((bv (bytevector 1 2 3))) (bytevector-u8-set! bv 0 99) (bytevector-u8-ref bv 0))"),
            Value::Int(99)
        );
    }

    #[test]
    fn test_utf8_conversion() {
        assert_eq!(
            eval("(utf8->string (string->utf8 \"hello\"))"),
            Value::String(Rc::from("hello"))
        );
    }
}
