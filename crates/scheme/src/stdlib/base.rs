//! R7RS §6.1-6.5, §6.10-6.11: Core primitives.
//!
//! Equivalence predicates, arithmetic, booleans, pairs/lists, symbols,
//! control flow, and exceptions.

use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    register_equivalence(vm);
    register_arithmetic(vm);
    register_booleans(vm);
    register_pairs_lists(vm);
    register_symbols(vm);
    register_control(vm);
    register_exceptions(vm);
    register_type_predicates(vm);
}

// -- §6.1 Equivalence predicates --

fn register_equivalence(vm: &mut Vm) {
    vm.register_fn("eq?", "Identity equality", Arity::Fixed(2), |args| {
        Ok(Value::Bool(args[0].is_eq(&args[1])))
    });

    vm.register_fn(
        "eqv?",
        "Equivalent values (same as eq? for atoms)",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].is_eqv(&args[1]))),
    );

    vm.register_fn(
        "equal?",
        "Recursive structural equality",
        Arity::Fixed(2),
        |args| Ok(Value::Bool(args[0].is_equal(&args[1]))),
    );
}

// -- §6.2 Numbers --

fn register_arithmetic(vm: &mut Vm) {
    vm.register_fn("+", "Add numbers", Arity::Variadic(0), |args| {
        let mut int_sum: i64 = 0;
        let mut is_float = false;
        let mut float_sum: f64 = 0.0;
        for a in args {
            match a {
                Value::Int(n) => {
                    if is_float {
                        float_sum += *n as f64;
                    } else {
                        int_sum = int_sum.wrapping_add(*n);
                    }
                }
                Value::Float(f) => {
                    if !is_float {
                        float_sum = int_sum as f64;
                        is_float = true;
                    }
                    float_sum += f;
                }
                _ => return Err(LispError::type_error("number", format!("{a}"))),
            }
        }
        if is_float {
            Ok(Value::Float(float_sum))
        } else {
            Ok(Value::Int(int_sum))
        }
    });

    vm.register_fn("-", "Subtract numbers", Arity::Variadic(1), |args| {
        if args.len() == 1 {
            return match &args[0] {
                Value::Int(n) => Ok(Value::Int(-n)),
                Value::Float(f) => Ok(Value::Float(-f)),
                _ => Err(LispError::type_error("number", format!("{}", args[0]))),
            };
        }
        let mut result = require_f64(&args[0])?;
        let first_is_int = matches!(args[0], Value::Int(_));
        let mut all_int = first_is_int;
        for a in &args[1..] {
            match a {
                Value::Int(n) => result -= *n as f64,
                Value::Float(f) => {
                    all_int = false;
                    result -= f;
                }
                _ => return Err(LispError::type_error("number", format!("{a}"))),
            }
        }
        if all_int && result.fract() == 0.0 {
            Ok(Value::Int(result as i64))
        } else {
            Ok(Value::Float(result))
        }
    });

    vm.register_fn("*", "Multiply numbers", Arity::Variadic(0), |args| {
        let mut int_prod: i64 = 1;
        let mut is_float = false;
        let mut float_prod: f64 = 1.0;
        for a in args {
            match a {
                Value::Int(n) => {
                    if is_float {
                        float_prod *= *n as f64;
                    } else {
                        int_prod = int_prod.wrapping_mul(*n);
                    }
                }
                Value::Float(f) => {
                    if !is_float {
                        float_prod = int_prod as f64;
                        is_float = true;
                    }
                    float_prod *= f;
                }
                _ => return Err(LispError::type_error("number", format!("{a}"))),
            }
        }
        if is_float {
            Ok(Value::Float(float_prod))
        } else {
            Ok(Value::Int(int_prod))
        }
    });

    vm.register_fn("/", "Divide numbers", Arity::Variadic(1), |args| {
        if args.len() == 1 {
            let d = require_f64(&args[0])?;
            if d == 0.0 {
                return Err(LispError::division_by_zero());
            }
            return Ok(Value::Float(1.0 / d));
        }
        let mut result = require_f64(&args[0])?;
        for a in &args[1..] {
            let d = require_f64(a)?;
            if d == 0.0 {
                return Err(LispError::division_by_zero());
            }
            result /= d;
        }
        if result.fract() == 0.0 && result.abs() < i64::MAX as f64 {
            Ok(Value::Int(result as i64))
        } else {
            Ok(Value::Float(result))
        }
    });

    // Comparison operators
    vm.register_fn("=", "Numeric equality", Arity::Variadic(2), |args| {
        Ok(Value::Bool(numeric_compare(args, |a, b| a == b)?))
    });
    vm.register_fn("<", "Less than", Arity::Variadic(2), |args| {
        Ok(Value::Bool(numeric_compare(args, |a, b| a < b)?))
    });
    vm.register_fn(">", "Greater than", Arity::Variadic(2), |args| {
        Ok(Value::Bool(numeric_compare(args, |a, b| a > b)?))
    });
    vm.register_fn("<=", "Less or equal", Arity::Variadic(2), |args| {
        Ok(Value::Bool(numeric_compare(args, |a, b| a <= b)?))
    });
    vm.register_fn(">=", "Greater or equal", Arity::Variadic(2), |args| {
        Ok(Value::Bool(numeric_compare(args, |a, b| a >= b)?))
    });

    // Integer arithmetic
    vm.register_fn("quotient", "Integer division", Arity::Fixed(2), |args| {
        let a = args[0].as_int()?;
        let b = args[1].as_int()?;
        if b == 0 {
            return Err(LispError::division_by_zero());
        }
        Ok(Value::Int(a / b))
    });

    vm.register_fn("remainder", "Integer remainder", Arity::Fixed(2), |args| {
        let a = args[0].as_int()?;
        let b = args[1].as_int()?;
        if b == 0 {
            return Err(LispError::division_by_zero());
        }
        Ok(Value::Int(a % b))
    });

    vm.register_fn("modulo", "Integer modulo", Arity::Fixed(2), |args| {
        let a = args[0].as_int()?;
        let b = args[1].as_int()?;
        if b == 0 {
            return Err(LispError::division_by_zero());
        }
        Ok(Value::Int(((a % b) + b) % b))
    });

    vm.register_fn(
        "abs",
        "Absolute value",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(n.abs())),
            Value::Float(f) => Ok(Value::Float(f.abs())),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn("min", "Minimum of numbers", Arity::Variadic(1), |args| {
        let mut result = args[0].clone();
        for a in &args[1..] {
            if numeric_lt(a, &result)? {
                result = a.clone();
            }
        }
        Ok(result)
    });

    vm.register_fn("max", "Maximum of numbers", Arity::Variadic(1), |args| {
        let mut result = args[0].clone();
        for a in &args[1..] {
            if numeric_lt(&result, a)? {
                result = a.clone();
            }
        }
        Ok(result)
    });

    vm.register_fn(
        "floor",
        "Floor to integer",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(f.floor() as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "ceiling",
        "Ceiling to integer",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(f.ceil() as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "round",
        "Round to nearest integer",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(f.round() as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "truncate",
        "Truncate toward zero",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(f.trunc() as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "exact->inexact",
        "Convert to inexact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Float(*n as f64)),
            Value::Float(f) => Ok(Value::Float(*f)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "inexact->exact",
        "Convert to exact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(*f as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "number->string",
        "Convert number to string",
        Arity::Variadic(1),
        |args| {
            let radix = if args.len() > 1 {
                args[1].as_int()? as u32
            } else {
                10
            };
            match &args[0] {
                Value::Int(n) => {
                    let s = match radix {
                        2 => format!("{n:b}"),
                        8 => format!("{n:o}"),
                        10 => format!("{n}"),
                        16 => format!("{n:x}"),
                        _ => {
                            return Err(LispError::user(
                                "number->string: unsupported radix",
                                vec![],
                            ))
                        }
                    };
                    Ok(Value::String(Rc::from(s.as_str())))
                }
                Value::Float(f) => Ok(Value::String(Rc::from(format!("{f}").as_str()))),
                _ => Err(LispError::type_error("number", format!("{}", args[0]))),
            }
        },
    );

    vm.register_fn(
        "string->number",
        "Parse string to number",
        Arity::Variadic(1),
        |args| {
            let s = args[0].as_str()?;
            let radix = if args.len() > 1 {
                args[1].as_int()? as u32
            } else {
                10
            };
            if radix == 10 {
                if let Ok(n) = s.parse::<i64>() {
                    return Ok(Value::Int(n));
                }
                if let Ok(f) = s.parse::<f64>() {
                    return Ok(Value::Float(f));
                }
            } else if let Ok(n) = i64::from_str_radix(s, radix) {
                return Ok(Value::Int(n));
            }
            Ok(Value::Bool(false)) // R7RS returns #f on failure
        },
    );

    // Numeric predicates
    vm.register_fn("zero?", "Is zero?", Arity::Fixed(1), |args| {
        match &args[0] {
            Value::Int(n) => Ok(Value::Bool(*n == 0)),
            Value::Float(f) => Ok(Value::Bool(*f == 0.0)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        }
    });

    vm.register_fn(
        "positive?",
        "Is positive?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Bool(*n > 0)),
            Value::Float(f) => Ok(Value::Bool(*f > 0.0)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "negative?",
        "Is negative?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Bool(*n < 0)),
            Value::Float(f) => Ok(Value::Bool(*f < 0.0)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn("odd?", "Is odd integer?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(args[0].as_int()? % 2 != 0))
    });

    vm.register_fn("even?", "Is even integer?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(args[0].as_int()? % 2 == 0))
    });

    vm.register_fn(
        "exact?",
        "Is exact number?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(_) => Ok(Value::Bool(true)),
            Value::Float(_) => Ok(Value::Bool(false)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "inexact?",
        "Is inexact number?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(_) => Ok(Value::Bool(false)),
            Value::Float(_) => Ok(Value::Bool(true)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "integer?",
        "Is integer?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(_) => Ok(Value::Bool(true)),
            Value::Float(f) => Ok(Value::Bool(f.fract() == 0.0)),
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "exact",
        "Convert to exact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => Ok(Value::Int(*f as i64)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "inexact",
        "Convert to inexact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Float(*n as f64)),
            Value::Float(f) => Ok(Value::Float(*f)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );
}

// -- §6.3 Booleans --

fn register_booleans(vm: &mut Vm) {
    vm.register_fn("not", "Boolean not", Arity::Fixed(1), |args| {
        Ok(Value::Bool(args[0].is_false()))
    });

    vm.register_fn(
        "boolean=?",
        "Boolean equality",
        Arity::Variadic(2),
        |args| {
            let first = args[0].is_true();
            Ok(Value::Bool(args[1..].iter().all(|a| a.is_true() == first)))
        },
    );
}

// -- §6.4 Pairs and lists --

fn register_pairs_lists(vm: &mut Vm) {
    vm.register_fn("cons", "Construct pair", Arity::Fixed(2), |args| {
        Ok(Value::cons(args[0].clone(), args[1].clone()))
    });

    vm.register_fn("car", "First of pair", Arity::Fixed(1), |args| {
        args[0].car()
    });

    vm.register_fn("cdr", "Rest of pair", Arity::Fixed(1), |args| args[0].cdr());

    vm.register_fn("set-car!", "Set car of pair", Arity::Fixed(2), |args| {
        if matches!(&args[0], Value::Pair(_)) {
            Err(LispError::immutable("pair (set-car!)"))
        } else {
            Err(LispError::type_error("pair", format!("{}", args[0])))
        }
    });

    vm.register_fn("set-cdr!", "Set cdr of pair", Arity::Fixed(2), |args| {
        if matches!(&args[0], Value::Pair(_)) {
            Err(LispError::immutable("pair (set-cdr!)"))
        } else {
            Err(LispError::type_error("pair", format!("{}", args[0])))
        }
    });

    vm.register_fn("list", "Construct list", Arity::Variadic(0), |args| {
        Ok(Value::list(args.to_vec()))
    });

    vm.register_fn("length", "Length of list", Arity::Fixed(1), |args| {
        let mut len = 0i64;
        let mut current = args[0].clone();
        loop {
            match current {
                Value::Null => return Ok(Value::Int(len)),
                Value::Pair(p) => {
                    len += 1;
                    current = p.1.clone();
                }
                _ => return Err(LispError::type_error("proper list", format!("{}", args[0]))),
            }
        }
    });

    vm.register_fn("append", "Append lists", Arity::Variadic(0), |args| {
        if args.is_empty() {
            return Ok(Value::Null);
        }
        if args.len() == 1 {
            return Ok(args[0].clone());
        }
        let mut elems = Vec::new();
        for a in &args[..args.len() - 1] {
            let mut cur = a.clone();
            loop {
                match cur {
                    Value::Null => break,
                    Value::Pair(p) => {
                        elems.push(p.0.clone());
                        cur = p.1.clone();
                    }
                    _ => return Err(LispError::type_error("list", format!("{a}"))),
                }
            }
        }
        let mut result = args.last().unwrap().clone();
        for elem in elems.into_iter().rev() {
            result = Value::cons(elem, result);
        }
        Ok(result)
    });

    vm.register_fn("reverse", "Reverse a list", Arity::Fixed(1), |args| {
        let v = args[0]
            .to_vec()
            .map_err(|_| LispError::type_error("list", format!("{}", args[0])))?;
        let reversed: Vec<Value> = v.into_iter().rev().collect();
        Ok(Value::list(reversed))
    });

    vm.register_fn(
        "list-tail",
        "Return sublist after k elements",
        Arity::Fixed(2),
        |args| {
            let k = args[1].as_int()? as usize;
            let mut cur = args[0].clone();
            for _ in 0..k {
                cur = match cur {
                    Value::Pair(p) => p.1.clone(),
                    _ => return Err(LispError::user("list-tail: index out of range", vec![])),
                };
            }
            Ok(cur)
        },
    );

    vm.register_fn("list-ref", "Return k-th element", Arity::Fixed(2), |args| {
        let k = args[1].as_int()? as usize;
        let mut cur = args[0].clone();
        for _ in 0..k {
            cur = match cur {
                Value::Pair(p) => p.1.clone(),
                _ => return Err(LispError::user("list-ref: index out of range", vec![])),
            };
        }
        cur.car()
    });

    vm.register_fn("list?", "Is a proper list?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(args[0].is_list()))
    });

    // caar..cddr
    vm.register_fn("caar", "car of car", Arity::Fixed(1), |args| {
        args[0].car()?.car()
    });
    vm.register_fn("cadr", "car of cdr", Arity::Fixed(1), |args| {
        args[0].cdr()?.car()
    });
    vm.register_fn("cdar", "cdr of car", Arity::Fixed(1), |args| {
        args[0].car()?.cdr()
    });
    vm.register_fn("cddr", "cdr of cdr", Arity::Fixed(1), |args| {
        args[0].cdr()?.cdr()
    });

    // Association lists
    vm.register_fn(
        "assoc",
        "Find in alist by equal?",
        Arity::Fixed(2),
        |args| {
            let key = &args[0];
            let mut cur = args[1].clone();
            loop {
                match cur {
                    Value::Null => return Ok(Value::Bool(false)),
                    Value::Pair(p) => {
                        if let Value::Pair(entry) = &p.0 {
                            if entry.0.is_equal(key) {
                                return Ok(p.0.clone());
                            }
                        }
                        cur = p.1.clone();
                    }
                    _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
                }
            }
        },
    );

    vm.register_fn("assv", "Find in alist by eqv?", Arity::Fixed(2), |args| {
        let key = &args[0];
        let mut cur = args[1].clone();
        loop {
            match cur {
                Value::Null => return Ok(Value::Bool(false)),
                Value::Pair(p) => {
                    if let Value::Pair(entry) = &p.0 {
                        if entry.0.is_eqv(key) {
                            return Ok(p.0.clone());
                        }
                    }
                    cur = p.1.clone();
                }
                _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
            }
        }
    });

    vm.register_fn("assq", "Find in alist by eq?", Arity::Fixed(2), |args| {
        let key = &args[0];
        let mut cur = args[1].clone();
        loop {
            match cur {
                Value::Null => return Ok(Value::Bool(false)),
                Value::Pair(p) => {
                    if let Value::Pair(entry) = &p.0 {
                        if entry.0.is_eq(key) {
                            return Ok(p.0.clone());
                        }
                    }
                    cur = p.1.clone();
                }
                _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
            }
        }
    });

    // Member
    vm.register_fn(
        "member",
        "Find in list by equal?",
        Arity::Fixed(2),
        |args| {
            let key = &args[0];
            let mut cur = args[1].clone();
            loop {
                match cur {
                    Value::Null => return Ok(Value::Bool(false)),
                    Value::Pair(ref p) => {
                        if p.0.is_equal(key) {
                            return Ok(cur);
                        }
                        cur = p.1.clone();
                    }
                    _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
                }
            }
        },
    );

    vm.register_fn("memv", "Find in list by eqv?", Arity::Fixed(2), |args| {
        let key = &args[0];
        let mut cur = args[1].clone();
        loop {
            match cur {
                Value::Null => return Ok(Value::Bool(false)),
                Value::Pair(ref p) => {
                    if p.0.is_eqv(key) {
                        return Ok(cur);
                    }
                    cur = p.1.clone();
                }
                _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
            }
        }
    });

    vm.register_fn("memq", "Find in list by eq?", Arity::Fixed(2), |args| {
        let key = &args[0];
        let mut cur = args[1].clone();
        loop {
            match cur {
                Value::Null => return Ok(Value::Bool(false)),
                Value::Pair(ref p) => {
                    if p.0.is_eq(key) {
                        return Ok(cur);
                    }
                    cur = p.1.clone();
                }
                _ => return Err(LispError::type_error("list", format!("{}", args[1]))),
            }
        }
    });

    // map and for-each need VM access to call closures — register as stubs.
    // They'll be implemented as Scheme once the module system lands,
    // or as VM-integrated builtins later.
}

// -- §6.5 Symbols --

fn register_symbols(vm: &mut Vm) {
    vm.register_fn(
        "symbol->string",
        "Convert symbol to string",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Symbol(s) => Ok(Value::String(Rc::from(s.name()))),
            _ => Err(LispError::type_error("symbol", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "string->symbol",
        "Convert string to symbol",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::symbol(s))
        },
    );
}

// -- §6.10 Control --

fn register_control(vm: &mut Vm) {
    // `apply`, `call/cc` are compiled as special forms.
    // Register placeholders for dynamic lookups.

    vm.register_fn(
        "apply",
        "Apply procedure to list of arguments",
        Arity::Variadic(2),
        |_args| {
            Err(LispError::internal(
                "apply must be compiled as a special form (use Op::Apply)",
            ))
        },
    );

    vm.register_fn(
        "call-with-current-continuation",
        "Capture current continuation",
        Arity::Fixed(1),
        |_args| {
            Err(LispError::internal(
                "call/cc must be compiled as a special form",
            ))
        },
    );

    vm.register_fn(
        "call/cc",
        "Capture current continuation (alias)",
        Arity::Fixed(1),
        |_args| {
            Err(LispError::internal(
                "call/cc must be compiled as a special form",
            ))
        },
    );

    vm.register_fn(
        "values",
        "Return multiple values",
        Arity::Variadic(0),
        |args| {
            if args.len() == 1 {
                Ok(args[0].clone())
            } else {
                Ok(Value::list(args.to_vec()))
            }
        },
    );
}

// -- §6.11 Exceptions --

fn register_exceptions(vm: &mut Vm) {
    vm.register_fn("error", "Raise an error", Arity::Variadic(1), |args| {
        let msg = match &args[0] {
            Value::String(s) => s.to_string(),
            other => format!("{other}"),
        };
        let irritants: Vec<String> = args[1..].iter().map(|v| format!("{v}")).collect();
        Err(LispError::user(&msg, irritants))
    });

    vm.register_fn(
        "error-object?",
        "Is error object?",
        Arity::Fixed(1),
        |_args| {
            // Error objects don't exist as values in our implementation —
            // they're Rust LispError. Always false for values.
            Ok(Value::Bool(false))
        },
    );

    vm.register_fn(
        "error-object-message",
        "Get error message",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::String(s) => Ok(Value::String(s.clone())),
            _ => Err(LispError::type_error(
                "error-object",
                format!("{}", args[0]),
            )),
        },
    );

    vm.register_fn("raise", "Raise exception value", Arity::Fixed(1), |args| {
        Err(LispError::user(format!("{}", args[0]), vec![]))
    });
}

// -- Type predicates --

fn register_type_predicates(vm: &mut Vm) {
    vm.register_fn("number?", "Is number?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(
            args[0],
            Value::Int(_) | Value::Float(_)
        )))
    });

    vm.register_fn("string?", "Is string?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::String(_))))
    });

    vm.register_fn("symbol?", "Is symbol?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Symbol(_))))
    });

    vm.register_fn("char?", "Is character?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Char(_))))
    });

    vm.register_fn("procedure?", "Is procedure?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(
            args[0],
            Value::Closure(_) | Value::Foreign(_) | Value::Continuation(_)
        )))
    });

    vm.register_fn("boolean?", "Is boolean?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Bool(_))))
    });

    vm.register_fn("pair?", "Is pair?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Pair(_))))
    });

    vm.register_fn("null?", "Is null?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Null)))
    });

    vm.register_fn("vector?", "Is vector?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Vector(_))))
    });

    vm.register_fn("bytevector?", "Is bytevector?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Bytevector(_))))
    });

    vm.register_fn("port?", "Is port?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Port(_))))
    });

    vm.register_fn("eof-object?", "Is EOF?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Eof)))
    });

    vm.register_fn("void?", "Is void?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(args[0], Value::Void)))
    });

    vm.register_fn("void", "Return void", Arity::Fixed(0), |_args| {
        Ok(Value::Void)
    });
}

// -- Helpers --

fn require_f64(v: &Value) -> Result<f64, LispError> {
    v.to_f64()
        .ok_or_else(|| LispError::type_error("number", format!("{v}")))
}

fn numeric_compare(args: &[Value], pred: fn(f64, f64) -> bool) -> Result<bool, LispError> {
    for i in 0..args.len() - 1 {
        let a = require_f64(&args[i])?;
        let b = require_f64(&args[i + 1])?;
        if !pred(a, b) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn numeric_lt(a: &Value, b: &Value) -> Result<bool, LispError> {
    Ok(require_f64(a)? < require_f64(b)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(code: &str) -> Value {
        let mut vm = Vm::new();
        crate::stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap()
    }

    fn eval_err(code: &str) -> LispError {
        let mut vm = Vm::new();
        crate::stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap_err()
    }

    // -- Arithmetic --

    #[test]
    fn test_add() {
        assert_eq!(eval("(+ 1 2 3)"), Value::Int(6));
        assert_eq!(eval("(+)"), Value::Int(0));
        assert_eq!(eval("(+ 1 2.0)"), Value::Float(3.0));
    }

    #[test]
    fn test_subtract() {
        assert_eq!(eval("(- 10 3)"), Value::Int(7));
        assert_eq!(eval("(- 5)"), Value::Int(-5));
        assert_eq!(eval("(- 10 3 2)"), Value::Int(5));
    }

    #[test]
    fn test_multiply() {
        assert_eq!(eval("(* 2 3 4)"), Value::Int(24));
        assert_eq!(eval("(*)"), Value::Int(1));
    }

    #[test]
    fn test_divide() {
        assert_eq!(eval("(/ 10 2)"), Value::Int(5));
        assert_eq!(eval("(/ 10 3)"), Value::Float(10.0 / 3.0));
        let _ = eval_err("(/ 1 0)");
    }

    #[test]
    fn test_integer_arithmetic() {
        assert_eq!(eval("(quotient 10 3)"), Value::Int(3));
        assert_eq!(eval("(remainder 10 3)"), Value::Int(1));
        assert_eq!(eval("(modulo -10 3)"), Value::Int(2));
    }

    #[test]
    fn test_comparison() {
        assert_eq!(eval("(= 1 1 1)"), Value::Bool(true));
        assert_eq!(eval("(< 1 2 3)"), Value::Bool(true));
        assert_eq!(eval("(> 3 2 1)"), Value::Bool(true));
        assert_eq!(eval("(<= 1 1 2)"), Value::Bool(true));
        assert_eq!(eval("(>= 2 1 1)"), Value::Bool(true));
    }

    #[test]
    fn test_min_max() {
        assert_eq!(eval("(min 3 1 2)"), Value::Int(1));
        assert_eq!(eval("(max 3 1 2)"), Value::Int(3));
    }

    #[test]
    fn test_numeric_predicates() {
        assert_eq!(eval("(zero? 0)"), Value::Bool(true));
        assert_eq!(eval("(positive? 5)"), Value::Bool(true));
        assert_eq!(eval("(negative? -1)"), Value::Bool(true));
        assert_eq!(eval("(odd? 3)"), Value::Bool(true));
        assert_eq!(eval("(even? 4)"), Value::Bool(true));
        assert_eq!(eval("(exact? 42)"), Value::Bool(true));
        assert_eq!(eval("(inexact? 1.5)"), Value::Bool(true));
    }

    #[test]
    fn test_number_conversion() {
        assert_eq!(eval("(number->string 42)"), Value::String(Rc::from("42")));
        assert_eq!(
            eval("(number->string 255 16)"),
            Value::String(Rc::from("ff"))
        );
        assert_eq!(eval("(string->number \"42\")"), Value::Int(42));
        assert_eq!(eval("(string->number \"nope\")"), Value::Bool(false));
    }

    #[test]
    fn test_rounding() {
        assert_eq!(eval("(floor 2.7)"), Value::Int(2));
        assert_eq!(eval("(ceiling 2.3)"), Value::Int(3));
        assert_eq!(eval("(round 2.5)"), Value::Int(3));
        assert_eq!(eval("(truncate -2.7)"), Value::Int(-2));
    }

    // -- Equivalence --

    #[test]
    fn test_eq() {
        assert_eq!(eval("(eq? 'a 'a)"), Value::Bool(true));
        assert_eq!(eval("(eq? 1 1)"), Value::Bool(true));
    }

    #[test]
    fn test_equal() {
        assert_eq!(eval("(equal? '(1 2) '(1 2))"), Value::Bool(true));
        assert_eq!(eval("(equal? \"abc\" \"abc\")"), Value::Bool(true));
    }

    // -- Booleans --

    #[test]
    fn test_not() {
        assert_eq!(eval("(not #f)"), Value::Bool(true));
        assert_eq!(eval("(not #t)"), Value::Bool(false));
        assert_eq!(eval("(not 0)"), Value::Bool(false));
    }

    // -- Lists --

    #[test]
    fn test_cons_car_cdr() {
        assert_eq!(eval("(car (cons 1 2))"), Value::Int(1));
        assert_eq!(eval("(cdr (cons 1 2))"), Value::Int(2));
    }

    #[test]
    fn test_list_ops() {
        assert_eq!(eval("(length '(1 2 3))"), Value::Int(3));
        assert_eq!(eval("(length '())"), Value::Int(0));
    }

    #[test]
    fn test_append() {
        assert_eq!(eval("(length (append '(1 2) '(3 4)))"), Value::Int(4));
        assert_eq!(eval("(car (append '(1) '(2)))"), Value::Int(1));
    }

    #[test]
    fn test_reverse() {
        assert_eq!(eval("(car (reverse '(1 2 3)))"), Value::Int(3));
    }

    #[test]
    fn test_list_ref_tail() {
        assert_eq!(eval("(list-ref '(a b c) 1)"), Value::symbol("b"));
        assert_eq!(eval("(car (list-tail '(a b c) 2))"), Value::symbol("c"));
    }

    #[test]
    fn test_list_predicate() {
        assert_eq!(eval("(list? '(1 2 3))"), Value::Bool(true));
        assert_eq!(eval("(list? '())"), Value::Bool(true));
        assert_eq!(eval("(list? (cons 1 2))"), Value::Bool(false));
    }

    #[test]
    fn test_assoc() {
        assert_eq!(
            eval("(car (assoc 'b '((a 1) (b 2) (c 3))))"),
            Value::symbol("b")
        );
        assert_eq!(eval("(assoc 'z '((a 1) (b 2)))"), Value::Bool(false));
    }

    #[test]
    fn test_member() {
        assert_eq!(eval("(car (member 2 '(1 2 3)))"), Value::Int(2));
        assert_eq!(eval("(member 5 '(1 2 3))"), Value::Bool(false));
    }

    // -- Symbols --

    #[test]
    fn test_symbol_conversion() {
        assert_eq!(
            eval("(symbol->string 'hello)"),
            Value::String(Rc::from("hello"))
        );
        assert_eq!(eval("(string->symbol \"world\")"), Value::symbol("world"));
    }

    // -- Type predicates --

    #[test]
    fn test_predicates() {
        assert_eq!(eval("(number? 42)"), Value::Bool(true));
        assert_eq!(eval("(number? \"hi\")"), Value::Bool(false));
        assert_eq!(eval("(string? \"hi\")"), Value::Bool(true));
        assert_eq!(eval("(symbol? 'x)"), Value::Bool(true));
        assert_eq!(eval("(boolean? #t)"), Value::Bool(true));
        assert_eq!(eval("(pair? '(1))"), Value::Bool(true));
        assert_eq!(eval("(null? '())"), Value::Bool(true));
        assert_eq!(eval("(procedure? car)"), Value::Bool(true));
        assert_eq!(eval("(integer? 42)"), Value::Bool(true));
        assert_eq!(eval("(integer? 1.5)"), Value::Bool(false));
    }

    // -- Exceptions --

    #[test]
    fn test_error() {
        let e = eval_err("(error \"boom\" 1 2)");
        assert!(e.to_string().contains("boom"));
    }
}
