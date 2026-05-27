//! R7RS standard library registration.
//!
//! After all primitives are registered as VM globals, this module creates
//! proper R7RS library facades in the `LibraryRegistry`. Each library is
//! a curated export list referencing the existing globals.
//!
//! This follows the Chibi-Scheme pattern: primitives live in the
//! interaction environment (globals), and standard libraries are
//! re-export facades that reference those primitives.
//!
//! ## Architecture (validated against prior art)
//!
//! - **Chibi-Scheme**: `.sld` files import from `(chibi)` internal module
//! - **Gauche**: `scheme.base` module wraps `gauche` built-in module
//! - **Guile**: R7RS libraries mapped to native Guile modules
//!
//! All three use the same pattern: internal primitives + standard facades.
//! See `20260527140000-mae_scheme_library_architecture.org` for full survey.
//!
//! @stability: unstable (Phase 13i)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::library::{Library, LibraryName};
use crate::value::Value;
use crate::vm::Vm;

/// Register all R7RS standard libraries as facades over existing globals.
///
/// Must be called AFTER `register_stdlib()` has populated `vm.globals`.
pub fn register_r7rs_libraries(vm: &mut Vm) {
    register_scheme_base(vm);
    register_scheme_case_lambda(vm);
    register_scheme_char(vm);
    register_scheme_complex(vm);
    register_scheme_cxr(vm);
    register_scheme_eval(vm);
    register_scheme_file(vm);
    register_scheme_inexact(vm);
    register_scheme_lazy(vm);
    register_scheme_load(vm);
    register_scheme_process_context(vm);
    register_scheme_read(vm);
    register_scheme_repl(vm);
    register_scheme_time(vm);
    register_scheme_write(vm);
}

/// Helper: build a Library from a list of export names, pulling values from globals.
fn library_from_globals(vm: &Vm, name: &[&str], export_names: &[&str]) -> Library {
    let mut exports = HashMap::new();
    for &n in export_names {
        if let Some(val) = vm.globals.get(n) {
            exports.insert(n.to_string(), val.clone());
        }
        // If not found in globals, it may be a special form handled by the
        // compiler. We insert a sentinel so (import (only ...)) can verify
        // the name exists. The actual binding comes from the compiler, not
        // the library export value.
    }
    Library {
        name: LibraryName(name.iter().map(|s| s.to_string()).collect()),
        exports,
    }
}

/// Helper: register special forms as Symbol sentinels in a library.
/// Special forms are handled by the compiler, not the VM, so they don't
/// have runtime values. We register them as symbols so `(import (only ...))`
/// can validate their existence.
fn add_special_forms(exports: &mut HashMap<String, Value>, names: &[&str]) {
    for &n in names {
        exports
            .entry(n.to_string())
            .or_insert_with(|| Value::symbol(n));
    }
}

// ---------------------------------------------------------------------------
// R7RS §5.7: Standard Libraries
// ---------------------------------------------------------------------------

/// `(scheme base)` — R7RS §5.7
///
/// The largest standard library. Contains most of R7RS-small:
/// equivalence, arithmetic, booleans, pairs, lists, symbols, characters,
/// strings, vectors, bytevectors, control, exceptions, eval, I/O basics.
fn register_scheme_base(vm: &mut Vm) {
    let mut lib = library_from_globals(
        vm,
        &["scheme", "base"],
        &[
            // §6.1 Equivalence
            "eq?",
            "eqv?",
            "equal?",
            // §6.2 Numbers — arithmetic
            "+",
            "-",
            "*",
            "/",
            "=",
            "<",
            ">",
            "<=",
            ">=",
            "abs",
            "max",
            "min",
            "floor",
            "ceiling",
            "round",
            "truncate",
            "floor/",
            "floor-quotient",
            "floor-remainder",
            "truncate/",
            "truncate-quotient",
            "truncate-remainder",
            "quotient",
            "remainder",
            "modulo",
            "gcd",
            "lcm",
            "expt",
            "sqrt",
            "square",
            "exact->inexact",
            "inexact->exact",
            "exact",
            "inexact",
            "number->string",
            "string->number",
            "exact-integer-sqrt",
            // §6.2 Numbers — predicates
            "number?",
            "complex?",
            "real?",
            "rational?",
            "integer?",
            "exact?",
            "inexact?",
            "exact-integer?",
            "zero?",
            "positive?",
            "negative?",
            "odd?",
            "even?",
            "nan?",
            "infinite?",
            "finite?",
            // §6.3 Booleans
            "not",
            "boolean?",
            "boolean=?",
            // §6.4 Pairs and lists
            "cons",
            "car",
            "cdr",
            "set-car!",
            "set-cdr!",
            "pair?",
            "null?",
            "list?",
            "list",
            "make-list",
            "length",
            "append",
            "reverse",
            "list-ref",
            "list-tail",
            "list-set!",
            "list-copy",
            "memq",
            "memv",
            "assq",
            "assv",
            // §6.5 Symbols
            "symbol?",
            "symbol=?",
            "symbol->string",
            "string->symbol",
            // §6.6 Characters (base subset)
            "char?",
            "char=?",
            "char<?",
            "char>?",
            "char<=?",
            "char>=?",
            "char->integer",
            "integer->char",
            "char-alphabetic?",
            "char-numeric?",
            "char-whitespace?",
            "char-upper-case?",
            "char-lower-case?",
            "char-upcase",
            "char-downcase",
            "char-foldcase",
            // §6.7 Strings (base subset)
            "string?",
            "make-string",
            "string",
            "string-length",
            "string-ref",
            "string-set!",
            "string=?",
            "string<?",
            "string>?",
            "string<=?",
            "string>=?",
            "substring",
            "string-append",
            "string->list",
            "list->string",
            "string-copy",
            "string-copy!",
            "string-fill!",
            "string-upcase",
            "string-downcase",
            "string-foldcase",
            // §6.8 Vectors
            "vector?",
            "make-vector",
            "vector",
            "vector-length",
            "vector-ref",
            "vector-set!",
            "vector->list",
            "list->vector",
            "vector-copy",
            "vector-copy!",
            "vector-fill!",
            "vector-append",
            "string->vector",
            "vector->string",
            // §6.9 Bytevectors
            "bytevector?",
            "make-bytevector",
            "bytevector",
            "bytevector-length",
            "bytevector-u8-ref",
            "bytevector-u8-set!",
            "bytevector-copy",
            "bytevector-copy!",
            "bytevector-append",
            "utf8->string",
            "string->utf8",
            // §6.10 Control
            "procedure?",
            "apply",
            "call-with-current-continuation",
            "call/cc",
            "values",
            "call-with-values",
            // §6.11 Exceptions
            "error",
            "error-object?",
            "error-object-message",
            "error-object-type",
            "error-object-irritants",
            "file-error?",
            "read-error?",
            "raise",
            // §6.13 I/O (base subset)
            "input-port?",
            "output-port?",
            "textual-port?",
            "binary-port?",
            "port?",
            "input-port-open?",
            "output-port-open?",
            "current-input-port",
            "current-output-port",
            "current-error-port",
            "close-port",
            "close-input-port",
            "close-output-port",
            "open-input-string",
            "open-output-string",
            "get-output-string",
            "open-input-bytevector",
            "open-output-bytevector",
            "get-output-bytevector",
            "read-char",
            "peek-char",
            "read-line",
            "char-ready?",
            "read-string",
            "read-u8",
            "peek-u8",
            "u8-ready?",
            "read-bytevector",
            "read-bytevector!",
            "write-char",
            "write-string",
            "write-u8",
            "write-bytevector",
            "newline",
            "flush-output-port",
            "eof-object",
            "eof-object?",
            // §6.14 System interface (base subset)
            "features",
            // Non-R7RS but expected by users
            "void",
            "void?",
            "format",
            "display-string",
        ],
    );

    // Special forms (handled by compiler, not runtime values)
    add_special_forms(
        &mut lib.exports,
        &[
            // §4.1 Primitive expression types
            "quote",
            "lambda",
            "if",
            "set!",
            // §4.2 Derived expression types
            "cond",
            "case",
            "and",
            "or",
            "when",
            "unless",
            "let",
            "let*",
            "letrec*",
            "let-values",
            "let*-values",
            "begin",
            "do",
            // §4.3 Macros
            "define-syntax",
            "syntax-rules",
            "syntax-error",
            "letrec-syntax",
            // §5.2 Definitions (also special forms)
            "define",
            "define-values",
            "define-record-type",
            // §6.10 Control (compiler-handled)
            "dynamic-wind",
            "with-exception-handler",
            "raise-continuable",
            "guard",
            // §4.2.7 Sequencing extensions
            "quasiquote",
            // §4.2.9
            "parameterize",
            // §4.2.10
            "include",
            "include-ci",
            // §5.5
            "cond-expand",
            // receive (SRFI-8, commonly available)
            "receive",
        ],
    );

    vm.libraries.register(lib);
}

/// `(scheme case-lambda)` — R7RS §5.7
fn register_scheme_case_lambda(vm: &mut Vm) {
    let mut exports = HashMap::new();
    // case-lambda is a compiler special form
    add_special_forms(&mut exports, &["case-lambda"]);
    vm.libraries.register(Library {
        name: LibraryName(vec!["scheme".into(), "case-lambda".into()]),
        exports,
    });
}

/// `(scheme char)` — R7RS §5.7
///
/// Character classification and case conversion.
fn register_scheme_char(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "char"],
        &[
            "char-alphabetic?",
            "char-numeric?",
            "char-whitespace?",
            "char-upper-case?",
            "char-lower-case?",
            "char-upcase",
            "char-downcase",
            "char-foldcase",
            "char-ci=?",
            "char-ci<?",
            "char-ci>?",
            "char-ci<=?",
            "char-ci>=?",
            "digit-value",
            "string-ci=?",
            "string-ci<?",
            "string-ci>?",
            "string-ci<=?",
            "string-ci>=?",
            "string-upcase",
            "string-downcase",
            "string-foldcase",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme complex)` — R7RS §5.7
///
/// mae-scheme does not support complex numbers. This library exists
/// but exports stubs that work on reals (R7RS §6.2.1: all reals are complex).
fn register_scheme_complex(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "complex"],
        &[
            // These work on reals since all reals are trivially complex
            "real-part",
            "imag-part",
            "magnitude",
            "angle",
            "make-rectangular",
            "make-polar",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme cxr)` — R7RS §5.7
///
/// Compositions of car and cdr up to 4 deep.
fn register_scheme_cxr(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "cxr"],
        &[
            "caar", "cadr", "cdar", "cddr", "caaar", "caadr", "cadar", "caddr", "cdaar", "cdadr",
            "cddar", "cdddr", "caaaar", "caaadr", "caadar", "caaddr", "cadaar", "cadadr", "caddar",
            "cadddr", "cdaaar", "cdaadr", "cdadar", "cdaddr", "cddaar", "cddadr", "cdddar",
            "cddddr",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme eval)` — R7RS §5.7
fn register_scheme_eval(vm: &mut Vm) {
    let mut lib = library_from_globals(
        vm,
        &["scheme", "eval"],
        &["interaction-environment", "scheme-report-environment"],
    );
    // eval is both a special form and a function
    add_special_forms(&mut lib.exports, &["eval"]);
    vm.libraries.register(lib);
}

/// `(scheme file)` — R7RS §5.7
fn register_scheme_file(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "file"],
        &[
            "open-input-file",
            "open-output-file",
            "open-binary-input-file",
            "open-binary-output-file",
            "file-exists?",
            "delete-file",
            // call-with-input-file and call-with-output-file are typically
            // defined in Scheme bootstrap; include if available
            "call-with-input-file",
            "call-with-output-file",
            "with-input-from-file",
            "with-output-to-file",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme inexact)` — R7RS §5.7
///
/// Inexact (floating-point) math functions.
fn register_scheme_inexact(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "inexact"],
        &[
            "sin",
            "cos",
            "tan",
            "asin",
            "acos",
            "atan",
            "exp",
            "log",
            "sqrt",
            "finite?",
            "infinite?",
            "nan?",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme lazy)` — R7RS §5.7
///
/// Lazy evaluation: delay/force/promises.
fn register_scheme_lazy(vm: &mut Vm) {
    let mut lib = library_from_globals(vm, &["scheme", "lazy"], &["make-promise", "promise?"]);
    // delay, delay-force, force are compiler special forms or bootstrap
    add_special_forms(&mut lib.exports, &["delay", "delay-force", "force"]);
    vm.libraries.register(lib);
}

/// `(scheme load)` — R7RS §5.7
fn register_scheme_load(vm: &mut Vm) {
    let mut exports = HashMap::new();
    // load is a special form handled by the compiler/VM
    add_special_forms(&mut exports, &["load"]);
    vm.libraries.register(Library {
        name: LibraryName(vec!["scheme".into(), "load".into()]),
        exports,
    });
}

/// `(scheme process-context)` — R7RS §5.7
fn register_scheme_process_context(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "process-context"],
        &[
            "exit",
            "emergency-exit",
            "command-line",
            "get-environment-variable",
            "get-environment-variables",
        ],
    );
    vm.libraries.register(lib);
}

/// `(scheme read)` — R7RS §5.7
fn register_scheme_read(vm: &mut Vm) {
    let lib = library_from_globals(vm, &["scheme", "read"], &["read"]);
    vm.libraries.register(lib);
}

/// `(scheme repl)` — R7RS §5.7
fn register_scheme_repl(vm: &mut Vm) {
    let lib = library_from_globals(vm, &["scheme", "repl"], &["interaction-environment"]);
    vm.libraries.register(lib);
}

/// `(scheme time)` — R7RS §5.7
fn register_scheme_time(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "time"],
        &["current-second", "current-jiffy", "jiffies-per-second"],
    );
    vm.libraries.register(lib);
}

/// `(scheme write)` — R7RS §5.7
fn register_scheme_write(vm: &mut Vm) {
    let lib = library_from_globals(
        vm,
        &["scheme", "write"],
        &["display", "write", "write-shared", "write-simple"],
    );
    vm.libraries.register(lib);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib;

    fn make_vm() -> Vm {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        register_r7rs_libraries(&mut vm);
        vm
    }

    #[test]
    fn scheme_base_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "base".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        // Should have a large number of exports
        assert!(
            lib.exports.len() > 100,
            "scheme base has {} exports",
            lib.exports.len()
        );
        // Spot-check key functions
        assert!(lib.exports.contains_key("+"));
        assert!(lib.exports.contains_key("car"));
        assert!(lib.exports.contains_key("define")); // special form
        assert!(lib.exports.contains_key("lambda")); // special form
        assert!(lib.exports.contains_key("if")); // special form
    }

    #[test]
    fn scheme_char_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "char".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("char-ci=?"));
        assert!(lib.exports.contains_key("digit-value"));
        assert!(lib.exports.contains_key("string-upcase"));
    }

    #[test]
    fn scheme_write_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "write".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("display"));
        assert!(lib.exports.contains_key("write"));
    }

    #[test]
    fn scheme_inexact_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "inexact".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("sin"));
        assert!(lib.exports.contains_key("cos"));
        assert!(lib.exports.contains_key("exp"));
    }

    #[test]
    fn scheme_file_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "file".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("open-input-file"));
        assert!(lib.exports.contains_key("file-exists?"));
    }

    #[test]
    fn scheme_time_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "time".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("current-second"));
        assert!(lib.exports.contains_key("current-jiffy"));
    }

    #[test]
    fn scheme_process_context_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "process-context".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("exit"));
        assert!(lib.exports.contains_key("command-line"));
    }

    #[test]
    fn scheme_case_lambda_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "case-lambda".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("case-lambda"));
    }

    #[test]
    fn scheme_eval_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "eval".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("eval"));
    }

    #[test]
    fn scheme_read_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "read".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("read"));
    }

    #[test]
    fn scheme_load_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "load".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("load"));
    }

    #[test]
    fn scheme_repl_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "repl".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("interaction-environment"));
    }

    #[test]
    fn scheme_lazy_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "lazy".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("delay"));
        assert!(lib.exports.contains_key("force"));
    }

    #[test]
    fn scheme_cxr_registered() {
        let vm = make_vm();
        let name = LibraryName(vec!["scheme".into(), "cxr".into()]);
        assert!(vm.libraries.contains(&name));
        let lib = vm.libraries.get(&name).unwrap();
        assert!(lib.exports.contains_key("cadr"));
        assert!(lib.exports.contains_key("caddr"));
    }

    #[test]
    fn all_r7rs_libraries_registered() {
        let vm = make_vm();
        let expected = [
            vec!["scheme", "base"],
            vec!["scheme", "case-lambda"],
            vec!["scheme", "char"],
            vec!["scheme", "complex"],
            vec!["scheme", "cxr"],
            vec!["scheme", "eval"],
            vec!["scheme", "file"],
            vec!["scheme", "inexact"],
            vec!["scheme", "lazy"],
            vec!["scheme", "load"],
            vec!["scheme", "process-context"],
            vec!["scheme", "read"],
            vec!["scheme", "repl"],
            vec!["scheme", "time"],
            vec!["scheme", "write"],
        ];
        for parts in &expected {
            let name = LibraryName(parts.iter().map(|s| s.to_string()).collect());
            assert!(vm.libraries.contains(&name), "missing library: {name}");
        }
    }

    #[test]
    fn import_scheme_base_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme base)) (+ 1 2)");
        assert_eq!(result.unwrap(), Value::Int(3));
    }

    #[test]
    fn import_scheme_write_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme write)) (display \"hello\")");
        assert!(result.is_ok());
    }

    #[test]
    fn import_only_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (only (scheme base) car cdr cons)) (car (cons 1 2))");
        assert_eq!(result.unwrap(), Value::Int(1));
    }

    #[test]
    fn import_scheme_inexact_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme inexact)) (sin 0.0)");
        assert_eq!(result.unwrap(), Value::Float(0.0));
    }

    #[test]
    fn import_scheme_char_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme char)) (char-ci=? #\\a #\\A)");
        assert_eq!(result.unwrap(), Value::Bool(true));
    }

    #[test]
    fn import_scheme_time_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme time)) (> (current-second) 0)");
        assert_eq!(result.unwrap(), Value::Bool(true));
    }

    #[test]
    fn import_scheme_process_context_works() {
        let mut vm = make_vm();
        let result = vm.eval("(import (scheme process-context)) (list? (command-line))");
        assert_eq!(result.unwrap(), Value::Bool(true));
    }

    #[test]
    fn define_library_can_import_scheme_base() {
        let mut vm = make_vm();
        let result = vm.eval(
            r#"
            (define-library (test mylib)
              (import (scheme base))
              (export double)
              (begin
                (define (double x) (+ x x))))
            (import (test mylib))
            (double 21)
        "#,
        );
        assert_eq!(result.unwrap(), Value::Int(42));
    }

    #[test]
    fn define_library_with_multiple_imports() {
        let mut vm = make_vm();
        let result = vm.eval(
            r#"
            (define-library (test mathlib)
              (import (scheme base) (scheme inexact))
              (export circle-area)
              (begin
                (define pi 3.14159265)
                (define (circle-area r) (* pi (* r r)))))
            (import (test mathlib))
            (> (circle-area 1.0) 3.0)
        "#,
        );
        assert_eq!(result.unwrap(), Value::Bool(true));
    }

    #[test]
    fn library_private_bindings_dont_leak() {
        // R7RS §5.6: library-internal defines should not be visible
        // in the interaction environment after the library is defined.
        let mut vm = make_vm();
        vm.eval(
            r#"
            (define-library (test private)
              (import (scheme base))
              (export public-fn)
              (begin
                (define secret 42)
                (define (helper x) (+ x secret))
                (define (public-fn x) (helper x))))
        "#,
        )
        .unwrap();
        // Import and use the public function
        let result = vm.eval("(import (test private)) (public-fn 8)");
        assert_eq!(result.unwrap(), Value::Int(50));
        // The private binding 'secret' should NOT be in the interaction env
        let result = vm.eval("secret");
        assert!(
            result.is_err(),
            "library-private 'secret' leaked to interaction env"
        );
    }

    #[test]
    fn library_closure_chains_resolve_private_bindings() {
        // Exported closures that call private helpers must work
        // even after the interaction environment is restored.
        let mut vm = make_vm();
        let result = vm.eval(
            r#"
            (define-library (test chain)
              (import (scheme base))
              (export outer)
              (begin
                (define (inner x) (* x x))
                (define (middle x) (+ (inner x) 1))
                (define (outer x) (middle (+ x 1)))))
            (import (test chain))
            (outer 3)
        "#,
        );
        // outer(3) = middle(4) = inner(4) + 1 = 16 + 1 = 17
        assert_eq!(result.unwrap(), Value::Int(17));
    }

    #[test]
    fn multiple_libraries_isolated_from_each_other() {
        // Two libraries with same-named private bindings should not conflict.
        let mut vm = make_vm();
        let result = vm.eval(
            r#"
            (define-library (test lib-a)
              (import (scheme base))
              (export get-a)
              (begin
                (define val 10)
                (define (get-a) val)))
            (define-library (test lib-b)
              (import (scheme base))
              (export get-b)
              (begin
                (define val 20)
                (define (get-b) val)))
            (import (test lib-a) (test lib-b))
            (+ (get-a) (get-b))
        "#,
        );
        assert_eq!(result.unwrap(), Value::Int(30));
    }
}
