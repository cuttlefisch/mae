//! R7RS §6.1-6.5, §6.10-6.12: Core primitives.
//!
//! Equivalence predicates, arithmetic, booleans, pairs/lists, symbols,
//! control flow, exceptions, and eval.
//!
//! ## mae-scheme spec stances
//!
//! Where R7RS leaves behavior implementation-defined, mae-scheme makes the
//! following choices. Each is documented at the point of implementation and
//! here for reference.
//!
//! ### Numeric tower (§6.2)
//! - **Exact integers**: `i64` fixnums. No bignum promotion (planned).
//! - **Inexact reals**: `f64` IEEE 754 double precision.
//! - **Complex numbers**: Not supported. `(scheme complex)` library is absent.
//!   `complex?` returns `#t` for all numbers (R7RS §6.2.1: "all numbers are
//!   complex" in implementations without a separate complex type).
//! - **Exact/inexact coercion**: `(exact->inexact 5)` → `5.0`,
//!   `(inexact->exact 5.0)` → `5`. Truncation for non-integer inexacts.
//! - **Division**: `(/ 6 3)` → `2` (exact integer when divisible).
//!   `(/ 1 3)` → `0.333...` (inexact when not). R7RS permits this.
//!
//! ### Pairs and lists (§6.4)
//! - **Immutable pairs**: `set-car!` and `set-cdr!` are provided but pairs
//!   are `Rc<(Value, Value)>`. Mutation creates new pairs. `list-set!` errors.
//!
//! ### Multiple values (§6.10)
//! - **Values representation**: `(values x)` returns `x` directly.
//!   `(values x y z)` returns a list `(x y z)`. This is a pragmatic choice —
//!   true multi-value return would require VM-level support for a separate
//!   values type. `call-with-values` and `receive` work correctly with
//!   this representation via compiler-level desugaring.
//!
//! ### Eval (§6.12)
//! - **`eval`** is a compiler special form that emits an `Op::Eval` opcode.
//!   The VM converts the datum to string, re-parses, and evaluates it.
//!   This is correct for quoted data `(eval '(+ 1 2))` which is the
//!   standard use case. The environment argument is accepted but ignored —
//!   all eval happens in the interaction environment.
//!
//! ### Tail calls (§3.5)
//! - **Proper tail calls**: Guaranteed via `TAIL_CALL` opcode. Includes
//!   tail position in `if`, `cond`, `case`, `and`, `or`, `when`, `unless`,
//!   `let`, `let*`, `letrec`, `begin`, `do`, `guard`, and named `let`.
//!
//! ### Continuations (§6.10)
//! - **Full call/cc**: Captures entire VM state (stack + frames). One-shot
//!   and multi-shot invocation supported. `dynamic-wind` is implemented
//!   in Scheme (bootstrap) using `guard` for exception safety.

use std::cell::RefCell;
use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

/// Check if a value is an error object (tagged vector starting with 'error-object).
fn is_error_object(v: &Value) -> bool {
    get_error_object_fields(v).is_some()
}

/// Extract error object fields if value is a tagged error vector.
fn get_error_object_fields(v: &Value) -> Option<Vec<Value>> {
    if let Value::Vector(rc) = v {
        let fields = rc.borrow();
        if fields.len() == 4 {
            if let Value::Symbol(s) = &fields[0] {
                if s.name() == "error-object" {
                    return Some(fields.to_vec());
                }
            }
        }
    }
    None
}

pub fn register(vm: &mut Vm) {
    register_equivalence(vm);
    register_arithmetic(vm);
    register_booleans(vm);
    register_pairs_lists(vm);
    register_symbols(vm);
    register_control(vm);
    register_exceptions(vm);
    register_type_predicates(vm);
    register_list_ops(vm);
    register_extra_numeric(vm);
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
        let mut has_inexact = matches!(&args[0], Value::Float(_));
        for a in &args[1..] {
            if matches!(a, Value::Float(_)) {
                has_inexact = true;
            }
            if numeric_lt(a, &result)? {
                result = a.clone();
            }
        }
        // R7RS §6.2.6: if any argument is inexact, result is inexact
        if has_inexact {
            if let Value::Int(n) = result {
                return Ok(Value::Float(n as f64));
            }
        }
        Ok(result)
    });

    vm.register_fn("max", "Maximum of numbers", Arity::Variadic(1), |args| {
        let mut result = args[0].clone();
        let mut has_inexact = matches!(&args[0], Value::Float(_));
        for a in &args[1..] {
            if matches!(a, Value::Float(_)) {
                has_inexact = true;
            }
            if numeric_lt(&result, a)? {
                result = a.clone();
            }
        }
        // R7RS §6.2.6: if any argument is inexact, result is inexact
        if has_inexact {
            if let Value::Int(n) = result {
                return Ok(Value::Float(n as f64));
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
        "Round to nearest integer (banker's rounding: half to even)",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(*n)),
            Value::Float(f) => {
                // R7RS requires banker's rounding (round half to even)
                let rounded = {
                    let v = *f;
                    let floor = v.floor();
                    let frac = v - floor;
                    if (frac - 0.5).abs() < f64::EPSILON {
                        // Exactly halfway — round to even
                        let fl = floor as i64;
                        if fl % 2 == 0 {
                            fl
                        } else {
                            fl + 1
                        }
                    } else {
                        v.round() as i64
                    }
                };
                Ok(Value::Int(rounded))
            }
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
                    let (sign, abs_n) = if *n < 0 {
                        ("-", n.unsigned_abs())
                    } else {
                        ("", *n as u64)
                    };
                    let s = match radix {
                        2 => format!("{sign}{abs_n:b}"),
                        8 => format!("{sign}{abs_n:o}"),
                        10 => format!("{n}"),
                        16 => format!("{sign}{abs_n:x}"),
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

    vm.register_fn(
        "infinite?",
        "Is infinite?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Float(f) => Ok(Value::Bool(f.is_infinite())),
            Value::Int(_) => Ok(Value::Bool(false)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn("nan?", "Is NaN?", Arity::Fixed(1), |args| match &args[0] {
        Value::Float(f) => Ok(Value::Bool(f.is_nan())),
        Value::Int(_) => Ok(Value::Bool(false)),
        _ => Err(LispError::type_error("number", format!("{}", args[0]))),
    });
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

    // mae-scheme: pairs are immutable (Rc-based). list-set! is registered
    // with a helpful error message rather than being absent.
    vm.register_fn(
        "list-set!",
        "Set element of list. Error: mae-scheme pairs are immutable. Build new lists with cons/append.",
        Arity::Fixed(3),
        |_args| Err(LispError::user(
            "list-set!: mae-scheme pairs are immutable. Use (append (list-head lst k) (cons new-val (list-tail lst (+ k 1)))) to construct a modified list.",
            vec![],
        )),
    );

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
    // assoc is defined in Scheme bootstrap (supports optional comparator)

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

    // member is defined in Scheme bootstrap (supports optional comparator)

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

    // make-list, list-set!, list-copy
    vm.register_fn(
        "make-list",
        "Create list of k elements",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            let fill = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Undefined
            };
            Ok(Value::list(vec![fill; k]))
        },
    );

    vm.register_fn(
        "list-copy",
        "Shallow copy a list",
        Arity::Fixed(1),
        |args| {
            let elems = args[0]
                .to_vec()
                .map_err(|_| LispError::type_error("list", format!("{}", args[0])))?;
            Ok(Value::list(elems))
        },
    );

    vm.register_fn("symbol=?", "Compare symbols", Arity::Variadic(2), |args| {
        for arg in args {
            if !matches!(arg, Value::Symbol(_)) {
                return Err(LispError::type_error("symbol", format!("{arg}")));
            }
        }
        for i in 0..args.len() - 1 {
            if !args[i].is_eq(&args[i + 1]) {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    });
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

    // call-with-values: since our `values` returns a list for multiple values,
    // we apply the consumer to the list elements.
    vm.register_fn(
        "call-with-values",
        "Call consumer with values from producer",
        Arity::Fixed(2),
        |_args| {
            Err(LispError::internal(
                "call-with-values requires VM-level implementation",
            ))
        },
    );

    // R7RS §6.12 eval and environment specifiers
    vm.register_fn(
        "eval",
        "Evaluate expression in environment (stub)",
        Arity::Variadic(1),
        |_args| {
            Err(LispError::user(
                "eval: not yet implemented (requires VM access at runtime)",
                vec![],
            ))
        },
    );

    vm.register_fn(
        "interaction-environment",
        "Return the interaction environment",
        Arity::Fixed(0),
        |_args| Ok(Value::symbol("interaction")),
    );

    vm.register_fn(
        "scheme-report-environment",
        "Return the R7RS environment",
        Arity::Fixed(1),
        |_args| Ok(Value::symbol("r7rs")),
    );
}

// -- §6.10 Higher-order list operations --

fn register_list_ops(vm: &mut Vm) {
    // Higher-order list ops and R7RS features implemented as Scheme code.
    // This follows the Chibi-Scheme pattern (init-7.scm).
    let bootstrap = r#"
        ;; R7RS §6.10 map — single-list only; multi-list via internal helpers
        (define (map1 f lst)
          (if (null? lst)
              '()
              (cons (f (car lst)) (map1 f (cdr lst)))))

        ;; Check if any list in the list-of-lists is null
        (define (any-null? lsts)
          (if (null? lsts) #f
              (if (null? (car lsts)) #t
                  (any-null? (cdr lsts)))))

        (define (map f . lsts)
          (if (null? (cdr lsts))
              (map1 f (car lsts))
              ;; Multi-list: stop at shortest list
              (if (any-null? lsts)
                  '()
                  (cons (apply f (map1 car lsts))
                        (apply map f (map1 cdr lsts))))))

        ;; R7RS §6.10 for-each — single and multi-list
        (define (for-each1 f lst)
          (if (null? lst)
              (void)
              (begin (f (car lst)) (for-each1 f (cdr lst)))))

        (define (for-each f . lsts)
          (if (null? (cdr lsts))
              (for-each1 f (car lsts))
              (if (any-null? lsts)
                  (void)
                  (begin
                    (apply f (map1 car lsts))
                    (apply for-each f (map1 cdr lsts))))))

        (define (filter pred lst)
          (cond ((null? lst) '())
                ((pred (car lst)) (cons (car lst) (filter pred (cdr lst))))
                (else (filter pred (cdr lst)))))

        (define (fold-left f init lst)
          (if (null? lst)
              init
              (fold-left f (f init (car lst)) (cdr lst))))

        (define (fold-right f init lst)
          (if (null? lst)
              init
              (f (car lst) (fold-right f init (cdr lst)))))

        (define (call-with-values producer consumer)
          (let ((vals (producer)))
            (if (list? vals)
                (apply consumer vals)
                (consumer vals))))

        ;; R7RS §6.4 Extended cXXXr accessors
        (define (caaar x) (car (car (car x))))
        (define (caadr x) (car (car (cdr x))))
        (define (cadar x) (car (cdr (car x))))
        (define (caddr x) (car (cdr (cdr x))))
        (define (cdaar x) (cdr (car (car x))))
        (define (cdadr x) (cdr (car (cdr x))))
        (define (cddar x) (cdr (cdr (car x))))
        (define (cdddr x) (cdr (cdr (cdr x))))

        ;; R7RS §4.2.6 make-parameter — parameter objects as closures.
        ;; A parameter is a closure wrapping a mutable cell.
        ;; (param) → current value, (param v) → set value, returns void.
        (define (make-parameter init . args)
          (let ((value init)
                (converter (if (null? args) (lambda (x) x) (car args))))
            (lambda rest
              (if (null? rest)
                  value
                  (begin (set! value (converter (car rest))) (void))))))

        ;; R7RS §6.10 dynamic-wind — implemented as a compiler special form.
        ;; The compiler emits PushWinder/PopWinder opcodes that interact with
        ;; the VM's wind stack for proper call/cc semantics.

        ;; R7RS §6.7 string-for-each and string-map (multi-string)
        (define (string-for-each f . strs)
          (apply for-each f (map string->list strs)))

        (define (string-map f . strs)
          (list->string (apply map f (map string->list strs))))

        ;; R7RS §6.8 vector-for-each and vector-map (multi-vector)
        (define (vector-for-each f . vecs)
          (apply for-each f (map vector->list vecs)))

        (define (vector-map f . vecs)
          (list->vector (apply map f (map vector->list vecs))))

        ;; R7RS §6.13 call-with-port and file convenience functions
        (define (call-with-port port proc)
          (let ((result (proc port)))
            (close-port port)
            result))

        (define (call-with-input-file filename proc)
          (call-with-port (open-input-file filename) proc))

        (define (call-with-output-file filename proc)
          (call-with-port (open-output-file filename) proc))

        ;; R7RS §6.13.1 with-input-from-file / with-output-to-file
        ;; Uses dynamic-wind to ensure port is properly restored.
        (define (with-input-from-file filename thunk)
          (let ((port (open-input-file filename))
                (old-port (%current-input-port)))
            (dynamic-wind
              (lambda () (%set-current-input-port! port))
              (lambda () (let ((result (thunk)))
                           (close-input-port port)
                           result))
              (lambda () (%set-current-input-port! old-port)))))

        (define (with-output-to-file filename thunk)
          (let ((port (open-output-file filename))
                (old-port (%current-output-port)))
            (dynamic-wind
              (lambda () (%set-current-output-port! port))
              (lambda () (let ((result (thunk)))
                           (close-output-port port)
                           result))
              (lambda () (%set-current-output-port! old-port)))))

        ;; R7RS §4.2.5 Promises (delay/force)
        ;; Uses a mutable vector #(promise done? value/thunk)
        ;; Internal constructor
        (define (%make-promise-internal done? value)
          (vector 'promise done? value))

        ;; R7RS make-promise: wraps value in already-forced promise
        (define (make-promise obj)
          (if (and (vector? obj)
                   (> (vector-length obj) 0)
                   (eq? (vector-ref obj 0) 'promise))
              obj
              (%make-promise-internal #t obj)))

        (define (promise? obj)
          (and (vector? obj)
               (> (vector-length obj) 0)
               (eq? (vector-ref obj 0) 'promise)))

        (define-syntax delay
          (syntax-rules ()
            ((delay expr)
             (%make-promise-internal #f (lambda () expr)))))

        (define-syntax delay-force
          (syntax-rules ()
            ((delay-force expr)
             (%make-promise-internal #f (lambda () expr)))))

        ;; R7RS §4.2.5: force must iteratively force promises returned by
        ;; delay-force, enabling iterative lazy algorithms without stack growth.
        (define (force promise)
          (if (not (promise? promise))
              promise
              (if (vector-ref promise 1)
                  (vector-ref promise 2)
                  (let ((val ((vector-ref promise 2))))
                    (if (promise? val)
                        ;; delay-force case: the thunk returned another promise.
                        ;; Transfer its contents into this promise and force again.
                        (begin
                          (vector-set! promise 1 (vector-ref val 1))
                          (vector-set! promise 2 (vector-ref val 2))
                          (vector-set! val 1 #t)
                          (vector-set! val 2 promise)
                          (force promise))
                        ;; Normal delay case: memoize and return.
                        (begin
                          (vector-set! promise 1 #t)
                          (vector-set! promise 2 val)
                          val))))))

        ;; R7RS §6.4: member with optional comparator
        (define (member obj lst . rest)
          (let ((compare (if (null? rest) equal? (car rest))))
            (let loop ((l lst))
              (cond
                ((null? l) #f)
                ((compare obj (car l)) l)
                (else (loop (cdr l)))))))

        ;; R7RS §6.4: assoc with optional comparator
        (define (assoc obj alist . rest)
          (let ((compare (if (null? rest) equal? (car rest))))
            (let loop ((l alist))
              (cond
                ((null? l) #f)
                ((compare obj (caar l)) (car l))
                (else (loop (cdr l)))))))

        ;; R7RS §5.5 define-record-type
        ;; Implemented as a Rust-side function (registered below) because:
        ;; 1. syntax-rules can't do arithmetic on field indices
        ;; 2. define-macro doesn't support rest args (dotted pairs)
        ;; The Rust implementation is registered in register_record_type().
    "#;
    vm.eval(bootstrap)
        .unwrap_or_else(|e| panic!("failed to bootstrap list ops: {e}"));
}

// -- §6.2 Additional numeric operations --

fn register_extra_numeric(vm: &mut Vm) {
    vm.register_fn(
        "gcd",
        "Greatest common divisor",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Ok(Value::Int(0));
            }
            let mut result = args[0].as_int()?.unsigned_abs();
            for arg in &args[1..] {
                let b = arg.as_int()?.unsigned_abs();
                result = gcd_u64(result, b);
            }
            Ok(Value::Int(result as i64))
        },
    );

    vm.register_fn("lcm", "Least common multiple", Arity::Variadic(0), |args| {
        if args.is_empty() {
            return Ok(Value::Int(1));
        }
        let mut result = args[0].as_int()?.unsigned_abs();
        for arg in &args[1..] {
            let b = arg.as_int()?.unsigned_abs();
            if result == 0 || b == 0 {
                result = 0;
            } else {
                result = result / gcd_u64(result, b) * b;
            }
        }
        Ok(Value::Int(result as i64))
    });

    vm.register_fn("expt", "Raise to power", Arity::Fixed(2), |args| {
        let base = require_f64(&args[0])?;
        let exp = require_f64(&args[1])?;
        let result = base.powf(exp);
        if args[0].is_exact() && args[1].is_exact() && exp >= 0.0 && exp == exp.floor() {
            Ok(Value::Int(result as i64))
        } else {
            Ok(Value::Float(result))
        }
    });

    vm.register_fn("sqrt", "Square root", Arity::Fixed(1), |args| {
        let n = require_f64(&args[0])?;
        let result = n.sqrt();
        if args[0].is_exact() && result == result.floor() && result >= 0.0 {
            Ok(Value::Int(result as i64))
        } else {
            Ok(Value::Float(result))
        }
    });

    // R7RS numeric predicates
    vm.register_fn("complex?", "Is complex number?", Arity::Fixed(1), |args| {
        // All numbers are complex in R7RS (no separate complex type)
        Ok(Value::Bool(matches!(
            args[0],
            Value::Int(_) | Value::Float(_)
        )))
    });

    vm.register_fn("real?", "Is real number?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(matches!(
            args[0],
            Value::Int(_) | Value::Float(_)
        )))
    });

    vm.register_fn(
        "rational?",
        "Is rational number?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(_) => Ok(Value::Bool(true)),
            Value::Float(f) => Ok(Value::Bool(f.is_finite())),
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "exact-integer?",
        "Is exact integer?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(matches!(args[0], Value::Int(_)))),
    );

    vm.register_fn(
        "square",
        "Square a number",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Int(n.wrapping_mul(*n))),
            Value::Float(f) => Ok(Value::Float(f * f)),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "exact-integer-sqrt",
        "Integer square root",
        Arity::Fixed(1),
        |args| {
            let n = args[0].as_int()?;
            if n < 0 {
                return Err(LispError::user("exact-integer-sqrt: negative", vec![]));
            }
            let s = (n as f64).sqrt() as i64;
            let r = n - s * s;
            // Return values as a pair (s . r)
            Ok(Value::list(vec![Value::Int(s), Value::Int(r)]))
        },
    );

    // R7RS floor-quotient: floor(a/b) — rounds toward negative infinity
    vm.register_fn(
        "floor-quotient",
        "Floor division quotient",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            // floor division: round quotient toward negative infinity
            let q = a / b;
            let r = a % b;
            // Adjust if remainder has opposite sign to divisor
            if r != 0 && (r ^ b) < 0 {
                Ok(Value::Int(q - 1))
            } else {
                Ok(Value::Int(q))
            }
        },
    );

    // R7RS floor-remainder: a - floor-quotient(a,b) * b
    vm.register_fn(
        "floor-remainder",
        "Floor division remainder",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            let r = a % b;
            // Adjust if remainder has opposite sign to divisor
            if r != 0 && (r ^ b) < 0 {
                Ok(Value::Int(r + b))
            } else {
                Ok(Value::Int(r))
            }
        },
    );

    vm.register_fn(
        "truncate-quotient",
        "Truncated division quotient",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            Ok(Value::Int(a / b))
        },
    );

    vm.register_fn(
        "truncate-remainder",
        "Truncated division remainder",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            Ok(Value::Int(a % b))
        },
    );

    // R7RS §6.2.6 floor/ — returns two values (quotient, remainder)
    // R7RS says these return "two values" via the values mechanism.
    // Since our `values` for multiple returns is represented as a list,
    // we return a list which call-with-values/receive can destructure.
    vm.register_fn(
        "floor/",
        "Floor division returning two values: quotient and remainder",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            let q = a / b;
            let r = a % b;
            let (q, r) = if r != 0 && (r ^ b) < 0 {
                (q - 1, r + b)
            } else {
                (q, r)
            };
            Ok(Value::list(vec![Value::Int(q), Value::Int(r)]))
        },
    );

    // R7RS §6.2.6 truncate/ — returns two values (quotient, remainder)
    vm.register_fn(
        "truncate/",
        "Truncated division returning two values: quotient and remainder",
        Arity::Fixed(2),
        |args| {
            let a = args[0].as_int()?;
            let b = args[1].as_int()?;
            if b == 0 {
                return Err(LispError::user("division by zero", vec![]));
            }
            Ok(Value::list(vec![Value::Int(a / b), Value::Int(a % b)]))
        },
    );

    // R7RS §6.2.6 rationalize — approximate x within diff
    // For exact integers, returns x if diff >= 0
    // For inexact, finds simplest rational within tolerance
    vm.register_fn(
        "rationalize",
        "Simplest rational within tolerance",
        Arity::Fixed(2),
        |args| {
            let x = args[0].as_float()?;
            let diff = args[1].as_float()?;
            if diff.is_infinite() || diff.is_nan() {
                return Ok(Value::Float(0.0));
            }
            if x.is_infinite() || x.is_nan() {
                return Ok(Value::Float(x));
            }
            // Simple implementation: round to nearest integer if within tolerance
            let lo = x - diff.abs();
            let hi = x + diff.abs();
            // Find simplest rational p/q in [lo, hi] using Stern-Brocot
            // Simplified: check if an integer is in range first
            let lo_ceil = lo.ceil() as i64;
            let hi_floor = hi.floor() as i64;
            if lo_ceil <= hi_floor {
                // An integer is in range — that's the simplest
                if args[0].is_exact() && args[1].is_exact() {
                    return Ok(Value::Int(lo_ceil));
                }
                return Ok(Value::Float(lo_ceil as f64));
            }
            // Otherwise return x rounded to reasonable precision
            if args[0].is_exact() && args[1].is_exact() {
                Ok(Value::Int(x.round() as i64))
            } else {
                Ok(Value::Float(x))
            }
        },
    );
}

/// Register `(scheme inexact)` library functions.
pub fn register_inexact(vm: &mut Vm) {
    vm.register_fn("sin", "Sine", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.sin()))
    });
    vm.register_fn("cos", "Cosine", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.cos()))
    });
    vm.register_fn("tan", "Tangent", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.tan()))
    });
    vm.register_fn("asin", "Arc sine", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.asin()))
    });
    vm.register_fn("acos", "Arc cosine", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.acos()))
    });
    vm.register_fn(
        "atan",
        "Arc tangent (1 or 2 args)",
        Arity::Variadic(1),
        |args| {
            if args.len() == 1 {
                Ok(Value::Float(args[0].as_float()?.atan()))
            } else {
                Ok(Value::Float(args[0].as_float()?.atan2(args[1].as_float()?)))
            }
        },
    );
    vm.register_fn("exp", "Exponential (e^x)", Arity::Fixed(1), |args| {
        Ok(Value::Float(args[0].as_float()?.exp()))
    });
    vm.register_fn(
        "log",
        "Natural logarithm (1 arg) or log base (2 args)",
        Arity::Variadic(1),
        |args| {
            if args.len() == 1 {
                Ok(Value::Float(args[0].as_float()?.ln()))
            } else {
                let x = args[0].as_float()?;
                let base = args[1].as_float()?;
                Ok(Value::Float(x.ln() / base.ln()))
            }
        },
    );
    vm.register_fn("finite?", "Is number finite?", Arity::Fixed(1), |args| {
        Ok(Value::Bool(
            args[0].as_float().map_or(true, |f| f.is_finite()),
        ))
    });
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

// -- §6.11 Exceptions --

fn register_exceptions(vm: &mut Vm) {
    // error: Creates a tagged error object vector and raises it.
    // The error object is #(error-object message type irritants-list)
    vm.register_fn("error", "Raise an error", Arity::Variadic(1), |args| {
        let msg = match &args[0] {
            Value::String(s) => Value::String(s.clone()),
            other => Value::string(format!("{other}")),
        };
        let irritants = Value::list(args[1..].to_vec());
        // Build error object as tagged vector: #(error-object msg "error" irritants)
        let err_obj = Value::Vector(Rc::new(RefCell::new(vec![
            Value::symbol("error-object"),
            msg.clone(),
            Value::string("error"),
            irritants,
        ])));
        // Store display form in LispError for Rust-side reporting
        let display_msg = match &args[0] {
            Value::String(s) => s.to_string(),
            other => format!("{other}"),
        };
        let irritant_strs: Vec<String> = args[1..].iter().map(|v| format!("{v}")).collect();
        let mut err = LispError::user(display_msg, irritant_strs);
        // Stash the error object value so handle_exception can use it
        err.error_value = Some(Box::new(err_obj));
        Err(err)
    });

    vm.register_fn(
        "error-object?",
        "Is error object?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(is_error_object(&args[0]))),
    );

    vm.register_fn(
        "error-object-message",
        "Get error message",
        Arity::Fixed(1),
        |args| {
            if let Some(fields) = get_error_object_fields(&args[0]) {
                Ok(fields[1].clone()) // message field
            } else {
                // Fallback: treat string as error message
                match &args[0] {
                    Value::String(s) => Ok(Value::String(s.clone())),
                    _ => Err(LispError::type_error(
                        "error-object",
                        format!("{}", args[0]),
                    )),
                }
            }
        },
    );

    vm.register_fn("raise", "Raise exception value", Arity::Fixed(1), |args| {
        let mut err = LispError::user(format!("{}", args[0]), vec![]);
        err.error_value = Some(Box::new(args[0].clone()));
        Err(err)
    });

    vm.register_fn(
        "raise-continuable",
        "Raise continuable exception",
        Arity::Fixed(1),
        |args| {
            let mut err = LispError::user(format!("{}", args[0]), vec![]);
            err.error_value = Some(Box::new(args[0].clone()));
            Err(err)
        },
    );

    vm.register_fn(
        "error-object-irritants",
        "Get error irritants",
        Arity::Fixed(1),
        |args| {
            if let Some(fields) = get_error_object_fields(&args[0]) {
                Ok(fields[3].clone()) // irritants field
            } else {
                Ok(Value::Null)
            }
        },
    );

    vm.register_fn(
        "error-object-type",
        "Get error type",
        Arity::Fixed(1),
        |args| {
            if let Some(fields) = get_error_object_fields(&args[0]) {
                Ok(fields[2].clone()) // type field
            } else {
                Ok(Value::string("error"))
            }
        },
    );

    vm.register_fn("file-error?", "Is file error?", Arity::Fixed(1), |args| {
        if let Some(fields) = get_error_object_fields(&args[0]) {
            if let Value::String(s) = &fields[2] {
                return Ok(Value::Bool(s.as_ref() == "file-error"));
            }
        }
        Ok(Value::Bool(false))
    });

    vm.register_fn("read-error?", "Is read error?", Arity::Fixed(1), |args| {
        if let Some(fields) = get_error_object_fields(&args[0]) {
            if let Value::String(s) = &fields[2] {
                return Ok(Value::Bool(s.as_ref() == "read-error"));
            }
        }
        Ok(Value::Bool(false))
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
        assert_eq!(eval("(round 2.5)"), Value::Int(2)); // banker's rounding (R7RS)
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

    // -- Dynamic-wind --

    #[test]
    fn test_dynamic_wind_basic() {
        // Simple test: before/thunk/after all execute
        let val = eval(
            "(let ((x '()))
               (dynamic-wind
                 (lambda () (set! x (cons 'in x)))
                 (lambda () (set! x (cons 'body x)))
                 (lambda () (set! x (cons 'out x))))
               x)",
        );
        let list = val.to_list().unwrap();
        assert_eq!(list.len(), 3, "dynamic-wind result: {val}");
    }

    #[test]
    fn test_dynamic_wind_debug() {
        // Even simpler: just test closure mutation with set!
        let val = eval(
            "(let ((x '()))
               (let ((f (lambda () (set! x (cons 'a x)))))
                 (f)
                 (f)
                 x))",
        );
        let list = val.to_list().unwrap();
        assert_eq!(list.len(), 2, "closure mutation: {val}");
    }

    // -- Make-parameter --

    #[test]
    fn test_make_parameter() {
        assert_eq!(eval("(let ((p (make-parameter 10))) (p))"), Value::Int(10));
        assert_eq!(
            eval("(let ((p (make-parameter 10))) (p 20) (p))"),
            Value::Int(20)
        );
    }

    // -- Quasiquote --

    #[test]
    fn test_quasiquote_basic() {
        // Basic unquote
        assert_eq!(eval("`42"), Value::Int(42));
        assert_eq!(eval("(let ((x 10)) `,x)"), Value::Int(10));
    }

    #[test]
    fn test_quasiquote_list() {
        // Simple list — no unquotes
        assert_eq!(eval("(equal? `(a) '(a))"), Value::Bool(true));
        assert_eq!(eval("(equal? `(a b) '(a b))"), Value::Bool(true));
        // With unquote variable
        assert_eq!(eval("(equal? (let ((x 1)) `(,x)) '(1))"), Value::Bool(true),);
        assert_eq!(
            eval("(equal? (let ((x 1)) `(a ,x c)) '(a 1 c))"),
            Value::Bool(true),
        );
    }

    #[test]
    fn test_quasiquote_splicing() {
        // Splicing
        assert_eq!(
            eval("(equal? (let ((xs '(1 2 3))) `(a ,@xs b)) '(a 1 2 3 b))"),
            Value::Bool(true)
        );
    }

    // -- Case --

    #[test]
    fn test_case() {
        assert_eq!(
            eval("(case (+ 1 1) ((1) 'one) ((2) 'two) ((3) 'three))"),
            Value::symbol("two")
        );
        assert_eq!(
            eval("(case 5 ((1 2 3) 'small) (else 'big))"),
            Value::symbol("big")
        );
    }

    // -- Case-lambda --

    #[test]
    fn test_case_lambda() {
        assert_eq!(
            eval("(let ((f (case-lambda ((x) x) ((x y) (+ x y))))) (f 5))"),
            Value::Int(5)
        );
        assert_eq!(
            eval("(let ((f (case-lambda ((x) x) ((x y) (+ x y))))) (f 3 4))"),
            Value::Int(7)
        );
    }

    // -- Do --

    #[test]
    fn test_do() {
        assert_eq!(
            eval("(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 5) sum))"),
            Value::Int(10)
        );
    }

    // -- Delay/Force --

    #[test]
    fn test_delay_force() {
        assert_eq!(eval("(force (delay (+ 1 2)))"), Value::Int(3));
        // Test memoization
        assert_eq!(
            eval("(let ((p (delay (+ 1 2)))) (force p) (force p))"),
            Value::Int(3)
        );
    }
}
