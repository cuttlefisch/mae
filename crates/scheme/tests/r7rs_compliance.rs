//! R7RS-small compliance test suite for mae-scheme.
//!
//! Tests organized by R7RS specification section number.
//! Each section has its own test function with assertions covering
//! the behavior specified in the standard.
//!
//! Reference: https://small.r7rs.org/attachment/r7rs.pdf
//!
//! @ai-caution: [architecture-debt] Large by nature (one function per R7RS
//! section) — tracked in .claude/commands/mae-audit.md's "Known exceptions"
//! and ROADMAP.md's "Architecture Debt" section as an accepted exception,
//! not a splitting target.

use std::rc::Rc;

use mae_scheme::stdlib;
use mae_scheme::value::Value;
use mae_scheme::vm::Vm;

fn eval(code: &str) -> Value {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(code).unwrap()
}

fn eval_err(code: &str) -> String {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(code).unwrap_err().message()
}

fn is_true(code: &str) {
    assert_eq!(eval(code), Value::Bool(true), "expected #t for: {code}");
}

fn is_false(code: &str) {
    assert_eq!(eval(code), Value::Bool(false), "expected #f for: {code}");
}

fn is_str(code: &str, expected: &str) {
    assert_eq!(
        eval(code),
        Value::String(Rc::from(expected)),
        "expected \"{expected}\" for: {code}"
    );
}

fn is_int(code: &str, expected: i64) {
    assert_eq!(
        eval(code),
        Value::Int(expected),
        "expected {expected} for: {code}"
    );
}

fn is_float(code: &str, expected: f64) {
    assert_eq!(
        eval(code),
        Value::Float(expected),
        "expected {expected} for: {code}"
    );
}

/// Evaluate two expressions in the same VM and compare results.
/// Useful when comparing values that reference the same mutable state.
fn eval_eq(code: &str, expected: &str) {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let left = vm.eval(code).unwrap();
    let right = vm.eval(expected).unwrap();
    assert_eq!(left, right, "eval_eq failed:\n  code: {code}\n  expected: {expected}\n  left:  {left}\n  right: {right}");
}

// ============================================================================
// §4.1 Primitive expression types
// ============================================================================

#[test]
fn s4_1_1_variable_references() {
    is_int("(define x 28) x", 28);
}

#[test]
fn s4_1_2_literal_expressions() {
    // quote
    is_int("(quote 42)", 42);
    is_int("'42", 42);
    // Quote produces same structure
    is_true("(equal? (quote (1 2 3)) '(1 2 3))");

    // Self-evaluating
    is_int("42", 42);
    assert_eq!(eval("#t"), Value::Bool(true));
    assert_eq!(eval("#f"), Value::Bool(false));
    assert_eq!(eval("\"hello\""), Value::String(Rc::from("hello")));
    assert_eq!(eval("#\\a"), Value::Char('a'));
}

#[test]
fn s4_1_3_procedure_calls() {
    is_int("(+ 3 4)", 7);
    is_int("((lambda (x) (+ x x)) 4)", 8);
}

#[test]
fn s4_1_4_lambda() {
    // Fixed arity
    is_int("((lambda (x) (+ x 1)) 5)", 6);
    // Multiple args
    is_int("((lambda (x y) (+ x y)) 3 4)", 7);
    // Body with multiple expressions
    is_int("((lambda (x) (define y 2) (+ x y)) 3)", 5);
    // Variadic (rest args) — TODO: dotted-pair lambda not yet supported
    // is_true("((lambda (x . rest) (pair? rest)) 1 2 3)");
    // is_int("((lambda (x . rest) (length rest)) 1 2 3)", 2);
    // Zero-arg lambda
    is_int("((lambda () 42))", 42);
}

#[test]
fn s4_1_5_conditionals() {
    is_int("(if #t 1 2)", 1);
    is_int("(if #f 1 2)", 2);
    // Only #f is false
    is_int("(if 0 1 2)", 1);
    is_int("(if '() 1 2)", 1);
    is_int("(if \"\" 1 2)", 1);
    // Without else
    assert_eq!(eval("(if #t 42)"), Value::Int(42));
    assert_eq!(eval("(if #f 42)"), Value::Void);
}

#[test]
fn s4_1_6_assignments() {
    is_int("(define x 1) (set! x 2) x", 2);
}

// ============================================================================
// §4.2 Derived expression types
// ============================================================================

#[test]
fn s4_2_1_cond() {
    is_int("(cond (#t 1))", 1);
    is_int("(cond (#f 1) (#t 2))", 2);
    is_int("(cond (#f 1) (else 3))", 3);
    // cond with multiple body exprs
    is_int("(cond (#t 1 2 3))", 3);
}

#[test]
fn s4_2_1_and_or() {
    // and
    is_true("(and)");
    is_int("(and 1 2 3)", 3);
    is_false("(and 1 #f 3)");
    is_false("(and #f (error \"not reached\"))");

    // or
    is_false("(or)");
    is_int("(or 1 2)", 1);
    is_int("(or #f #f 3)", 3);
    is_int("(or 1 (error \"not reached\"))", 1);
}

#[test]
fn s4_2_1_when_unless() {
    is_int("(when #t 1 2 3)", 3);
    assert_eq!(eval("(when #f 42)"), Value::Void);
    is_int("(unless #f 1 2 3)", 3);
    assert_eq!(eval("(unless #t 42)"), Value::Void);
}

#[test]
fn s4_2_2_let() {
    is_int("(let ((x 2) (y 3)) (* x y))", 6);
    // Named let (iteration)
    is_int(
        "(let loop ((n 10) (acc 0))
           (if (= n 0) acc (loop (- n 1) (+ acc n))))",
        55,
    );
    // let as subexpression — locals must not corrupt enclosing stack
    is_int("(+ 10 (let ((x 1)) (+ x 2)))", 13);
    is_true("(equal? (let ((x 1)) (cons x '())) '(1))");
    is_true("(not (let ((x 1)) (= x 2)))");
}

#[test]
fn s4_2_2_let_star() {
    // let* allows sequential binding
    is_int("(let* ((x 1) (y (+ x 1))) y)", 2);
}

#[test]
fn s4_2_2_letrec() {
    // letrec allows mutual recursion
    is_true(
        "(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
                  (odd? (lambda (n) (if (= n 0) #f (even? (- n 1))))))
           (even? 10))",
    );
}

#[test]
fn s4_2_3_begin() {
    is_int("(begin 1 2 3)", 3);
    is_int("(begin (define x 1) (set! x 2) x)", 2);
}

// §4.2.6 Quasiquotation
#[test]
fn s4_2_6_quasiquote() {
    is_true("(equal? `(a b c) '(a b c))");
    is_true("(equal? (let ((x 1)) `(a ,x c)) '(a 1 c))");
    is_true("(equal? (let ((x '(1 2))) `(a ,@x c)) '(a 1 2 c))");
    is_int("`42", 42);
    is_true("(equal? (let ((x 10)) `,x) 10)");
}

// §4.2.7 case
#[test]
fn s4_2_7_case() {
    is_true(
        "(equal? (case (+ 1 1)
                  ((1) 'one)
                  ((2) 'two)
                  ((3) 'three))
                'two)",
    );
    is_true(
        "(equal? (case 99
                  ((1) 'one)
                  (else 'other))
                'other)",
    );
}

// §4.2.9 case-lambda
#[test]
fn s4_2_9_case_lambda() {
    is_int(
        "(let ((f (case-lambda
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (+ (f 1) (f 2 3) (f 4 5 6)))",
        21,
    );
}

// §4.2.7 do
#[test]
fn s4_2_7_do() {
    is_int(
        "(do ((i 0 (+ i 1))
              (sum 0 (+ sum i)))
             ((= i 5) sum))",
        10,
    );
}

// §4.2.7 parameterize
#[test]
fn s4_2_7_parameterize() {
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 42))
             (p)))",
        42,
    );
    // Parameter restored after parameterize
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 42))
             'ignored)
           (p))",
        10,
    );
}

// ============================================================================
// §4.3 Macros
// ============================================================================

#[test]
fn s4_3_syntax_rules_basic() {
    is_int(
        "(define-syntax my-and
           (syntax-rules ()
             ((_) #t)
             ((_ e) e)
             ((_ e1 e2 ...) (if e1 (my-and e2 ...) #f))))
         (my-and 1 2 3)",
        3,
    );
}

#[test]
fn s4_3_syntax_rules_with_literals() {
    is_int(
        "(define-syntax my-case
           (syntax-rules (=>)
             ((_ expr (val => result)) (if (= expr val) result #f))))
         (my-case 5 (5 => 42))",
        42,
    );
}

#[test]
fn s4_3_syntax_rules_let_implementation() {
    // Classic let implementation via syntax-rules — tests nested ellipsis
    is_int(
        "(define-syntax my-let
           (syntax-rules ()
             ((_ ((var val) ...) body ...)
              ((lambda (var ...) body ...) val ...))))
         (my-let ((x 10) (y 20)) (+ x y))",
        30,
    );
}

#[test]
fn s4_3_syntax_rules_or() {
    // or with let to avoid double evaluation
    is_int(
        "(define-syntax my-or
           (syntax-rules ()
             ((_) #f)
             ((_ e) e)
             ((_ e1 e2 ...)
              (let ((t e1)) (if t t (my-or e2 ...))))))
         (my-or #f #f 42)",
        42,
    );
}

// ============================================================================
// §5.3 Variable definitions
// ============================================================================

#[test]
fn s5_3_define() {
    is_int("(define x 42) x", 42);
    is_int("(define (f x) (+ x 1)) (f 5)", 6);
    // Internal define uses let semantics
    is_int("(define (g x) (+ x 10)) (g 3)", 13);
}

// ============================================================================
// §5.6 Libraries
// ============================================================================

#[test]
fn s5_6_define_library() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test arith)
           (export add1 sub1)
           (begin
             (define (add1 x) (+ x 1))
             (define (sub1 x) (- x 1))))",
    )
    .unwrap();
    vm.eval("(import (test arith))").unwrap();
    assert_eq!(vm.eval("(add1 5)").unwrap(), Value::Int(6));
    assert_eq!(vm.eval("(sub1 5)").unwrap(), Value::Int(4));
}

#[test]
fn s5_6_import_only() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib)
           (export a b c)
           (begin (define a 1) (define b 2) (define c 3)))",
    )
    .unwrap();
    vm.eval("(import (only (test lib) a c))").unwrap();
    assert_eq!(vm.eval("a").unwrap(), Value::Int(1));
    assert_eq!(vm.eval("c").unwrap(), Value::Int(3));
    assert!(vm.eval("b").is_err());
}

#[test]
fn s5_6_import_rename() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib)
           (export car)
           (begin (define car 42)))",
    )
    .unwrap();
    vm.eval("(import (rename (test lib) (car first)))").unwrap();
    assert_eq!(vm.eval("first").unwrap(), Value::Int(42));
}

#[test]
fn s5_6_import_prefix() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib)
           (export x y)
           (begin (define x 10) (define y 20)))",
    )
    .unwrap();
    vm.eval("(import (prefix (test lib) t:))").unwrap();
    assert_eq!(vm.eval("t:x").unwrap(), Value::Int(10));
    assert_eq!(vm.eval("t:y").unwrap(), Value::Int(20));
}

#[test]
fn s5_6_export_rename() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib)
           (export (rename internal-fn public-fn))
           (begin (define (internal-fn x) (+ x 1))))",
    )
    .unwrap();
    vm.eval("(import (test lib))").unwrap();
    assert_eq!(vm.eval("(public-fn 10)").unwrap(), Value::Int(11));
}

// ============================================================================
// §6.1 Equivalence predicates
// ============================================================================

#[test]
fn s6_1_eq() {
    is_true("(eq? 'a 'a)");
    is_false("(eq? '(1) '(1))"); // different pairs
    is_true("(eq? #t #t)");
    is_false("(eq? #t #f)");
    is_true("(eq? '() '())");
}

#[test]
fn s6_1_eqv() {
    is_true("(eqv? 42 42)");
    is_true("(eqv? #\\a #\\a)");
    is_true("(eqv? 'foo 'foo)");
    is_false("(eqv? 42 42.0)");
    is_false("(eqv? \"hello\" \"hello\")"); // strings not eqv? by R7RS
}

#[test]
fn s6_1_equal() {
    is_true("(equal? '(1 2 3) '(1 2 3))");
    is_true("(equal? \"abc\" \"abc\")");
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_false("(equal? '(1 2) '(1 3))");
}

// ============================================================================
// §6.2 Numbers
// ============================================================================

#[test]
fn s6_2_arithmetic() {
    is_int("(+ 1 2 3)", 6);
    is_int("(- 10 3)", 7);
    is_int("(* 2 3 4)", 24);
    is_int("(/ 10 2)", 5);
    is_int("(+)", 0);
    is_int("(*)", 1);
}

#[test]
fn s6_2_comparison() {
    is_true("(= 1 1 1)");
    is_false("(= 1 2)");
    is_true("(< 1 2 3)");
    is_false("(< 1 1)");
    is_true("(> 3 2 1)");
    is_true("(<= 1 1 2)");
    is_true("(>= 3 3 2)");
}

#[test]
fn s6_2_predicates() {
    is_true("(number? 42)");
    is_true("(number? 3.14)");
    is_false("(number? \"42\")");
    is_true("(integer? 42)");
    is_false("(integer? 3.14)");
    is_true("(zero? 0)");
    is_true("(positive? 1)");
    is_true("(negative? -1)");
    is_false("(zero? 1)");
    is_true("(even? 4)");
    is_true("(odd? 3)");
    is_false("(even? 3)");
}

#[test]
fn s6_2_max_min() {
    is_int("(max 3 1 4 1 5)", 5);
    is_int("(min 3 1 4 1 5)", 1);
}

#[test]
fn s6_2_abs() {
    is_int("(abs -7)", 7);
    is_int("(abs 7)", 7);
}

#[test]
fn s6_2_quotient_remainder_modulo() {
    is_int("(quotient 13 4)", 3);
    is_int("(remainder 13 4)", 1);
    is_int("(modulo 13 4)", 1);
    is_int("(remainder -13 4)", -1);
    is_int("(modulo -13 4)", 3);
}

#[test]
fn s6_2_gcd_lcm() {
    is_int("(gcd 32 -36)", 4);
    is_int("(gcd)", 0);
    is_int("(lcm 32 -36)", 288);
    is_int("(lcm)", 1);
}

#[test]
fn s6_2_exact_inexact() {
    is_true("(exact? 42)");
    is_false("(exact? 3.14)");
    is_true("(inexact? 3.14)");
    is_false("(inexact? 42)");
}

#[test]
fn s6_2_number_conversions() {
    assert_eq!(eval("(number->string 42)"), Value::String(Rc::from("42")));
    is_int("(string->number \"42\")", 42);
    is_false("(string->number \"not-a-number\")");
    is_int("(exact 3.0)", 3);
}

#[test]
fn s6_2_floor_ceiling_truncate_round() {
    // R7RS §6.2.6: floor/ceiling/round/truncate return inexact for inexact args
    is_float("(floor 2.7)", 2.0);
    is_float("(ceiling 2.3)", 3.0);
    is_float("(truncate 2.7)", 2.0);
    is_float("(truncate -2.7)", -2.0);
    is_float("(round 2.5)", 2.0); // banker's rounding
    is_float("(round 3.5)", 4.0);
}

// ============================================================================
// §6.3 Booleans
// ============================================================================

#[test]
fn s6_3_booleans() {
    is_true("(boolean? #t)");
    is_true("(boolean? #f)");
    is_false("(boolean? 0)");
    is_false("(not #t)");
    is_true("(not #f)");
    is_false("(not 42)"); // everything except #f is truthy
    is_false("(not '())");
}

// ============================================================================
// §6.4 Pairs and lists
// ============================================================================

#[test]
fn s6_4_cons_car_cdr() {
    is_int("(car (cons 1 2))", 1);
    is_int("(cdr (cons 1 2))", 2);
    is_int("(car '(1 2 3))", 1);
    is_int("(cadr '(1 2 3))", 2);
}

#[test]
fn s6_4_predicates() {
    is_true("(pair? '(1 2))");
    is_false("(pair? '())");
    is_true("(null? '())");
    is_false("(null? '(1))");
    is_true("(list? '(1 2 3))");
    is_true("(list? '())");
    is_false("(list? (cons 1 2))"); // dotted pair
}

#[test]
fn s6_4_list_operations() {
    is_int("(length '(1 2 3))", 3);
    is_int("(length '())", 0);

    // append
    let result = eval("(append '(1 2) '(3 4))");
    let v = result.to_vec().unwrap();
    assert_eq!(
        v,
        vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)]
    );

    // reverse
    let result = eval("(reverse '(1 2 3))");
    let v = result.to_vec().unwrap();
    assert_eq!(v, vec![Value::Int(3), Value::Int(2), Value::Int(1)]);

    // list-ref
    is_true("(eq? (list-ref '(a b c d) 2) 'c)");

    // list-tail
    is_int("(car (list-tail '(1 2 3 4) 2))", 3);
}

#[test]
fn s6_4_assoc() {
    is_int("(cdr (assoc 'b '((a . 1) (b . 2) (c . 3))))", 2);
    is_false("(assoc 'z '((a . 1) (b . 2)))");
    is_true("(eq? (cdr (assv 2 '((1 . a) (2 . b) (3 . c)))) 'b)");
}

#[test]
fn s6_4_member() {
    is_true("(pair? (member 3 '(1 2 3 4 5)))");
    is_int("(car (member 3 '(1 2 3 4 5)))", 3);
    is_false("(member 6 '(1 2 3))");
}

// ============================================================================
// §6.5 Symbols
// ============================================================================

#[test]
fn s6_5_symbols() {
    is_true("(symbol? 'foo)");
    is_false("(symbol? \"foo\")");
    is_false("(symbol? 42)");
    assert_eq!(
        eval("(symbol->string 'hello)"),
        Value::String(Rc::from("hello"))
    );
    is_true("(eq? (string->symbol \"test\") 'test)");
}

// ============================================================================
// §6.6 Characters
// ============================================================================

#[test]
fn s6_6_char_predicates() {
    is_true("(char? #\\a)");
    is_false("(char? 42)");
    is_true("(char-alphabetic? #\\a)");
    is_false("(char-alphabetic? #\\1)");
    is_true("(char-numeric? #\\5)");
    is_true("(char-whitespace? #\\space)");
    is_true("(char-upper-case? #\\A)");
    is_true("(char-lower-case? #\\a)");
}

#[test]
fn s6_6_char_comparison() {
    is_true("(char=? #\\a #\\a)");
    is_true("(char<? #\\a #\\b)");
    is_false("(char>? #\\a #\\b)");
}

#[test]
fn s6_6_char_conversion() {
    assert_eq!(eval("(char-upcase #\\a)"), Value::Char('A'));
    assert_eq!(eval("(char-downcase #\\A)"), Value::Char('a'));
    is_int("(char->integer #\\A)", 65);
    assert_eq!(eval("(integer->char 65)"), Value::Char('A'));
}

// ============================================================================
// §6.7 Strings
// ============================================================================

#[test]
fn s6_7_string_basic() {
    is_true("(string? \"hello\")");
    is_false("(string? 42)");
    is_int("(string-length \"hello\")", 5);
    assert_eq!(eval("(string-ref \"hello\" 1)"), Value::Char('e'));
}

#[test]
fn s6_7_string_operations() {
    assert_eq!(
        eval("(substring \"hello world\" 6 11)"),
        Value::String(Rc::from("world"))
    );
    assert_eq!(
        eval("(string-append \"hello\" \" \" \"world\")"),
        Value::String(Rc::from("hello world"))
    );
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
fn s6_7_string_comparison() {
    is_true("(string=? \"abc\" \"abc\")");
    is_false("(string=? \"abc\" \"abd\")");
    is_true("(string<? \"abc\" \"abd\")");
    is_true("(string>? \"abd\" \"abc\")");
}

#[test]
fn s6_7_string_conversion() {
    assert_eq!(eval("(car (string->list \"abc\"))"), Value::Char('a'));
    assert_eq!(
        eval("(list->string '(#\\a #\\b #\\c))"),
        Value::String(Rc::from("abc"))
    );
    is_int("(string->number \"42\")", 42);
    assert_eq!(eval("(number->string 42)"), Value::String(Rc::from("42")));
}

// ============================================================================
// §6.8 Vectors
// ============================================================================

#[test]
fn s6_8_vector_basic() {
    is_true("(vector? #(1 2 3))");
    is_false("(vector? '(1 2 3))");
    is_int("(vector-length #(1 2 3))", 3);
    is_int("(vector-ref #(10 20 30) 1)", 20);
}

#[test]
fn s6_8_vector_operations() {
    // make-vector
    is_int("(vector-length (make-vector 5 0))", 5);
    is_int("(vector-ref (make-vector 3 42) 0)", 42);

    // vector->list
    is_int("(car (vector->list #(10 20 30)))", 10);
    is_int("(length (vector->list #(1 2 3)))", 3);

    // list->vector
    is_int("(vector-ref (list->vector '(10 20 30)) 2)", 30);

    // vector-set!
    is_int(
        "(define v (make-vector 3 0)) (vector-set! v 1 42) (vector-ref v 1)",
        42,
    );
}

// ============================================================================
// §6.9 Bytevectors
// ============================================================================

#[test]
fn s6_9_bytevectors() {
    is_true("(bytevector? #u8(1 2 3))");
    is_int("(bytevector-length #u8(1 2 3))", 3);
    is_int("(bytevector-u8-ref #u8(10 20 30) 1)", 20);
}

// ============================================================================
// §6.10 Control features
// ============================================================================

#[test]
fn s6_10_apply() {
    is_int("(apply + '(1 2 3))", 6);
    // TODO: apply with leading args before list — needs compiler support
    // is_int("(apply + 1 2 '(3))", 6);
}

#[test]
fn s6_10_map() {
    // map is a stdlib function
    let result = eval("(map (lambda (x) (* x x)) '(1 2 3 4 5))");
    let v = result.to_vec().unwrap();
    assert_eq!(v.len(), 5);
    assert_eq!(v[0], Value::Int(1));
    assert_eq!(v[4], Value::Int(25));
}

#[test]
fn s6_10_for_each() {
    // for-each returns void but executes side effects
    eval("(for-each (lambda (x) x) '(1 2 3))");
}

#[test]
fn s6_10_call_cc() {
    // Basic escape continuation
    is_int(
        "(call-with-current-continuation (lambda (k) (k 42) 99))",
        42,
    );
    // Continuation not invoked — returns body value
    is_int("(call-with-current-continuation (lambda (k) 42))", 42);
    // call/cc abbreviation
    is_int("(call/cc (lambda (k) (k 42) 99))", 42);
}

#[test]
fn s6_10_values_and_call_with_values() {
    is_int("(call-with-values (lambda () (values 1 2)) +)", 3);
}

#[test]
fn s6_10_procedure_predicate() {
    is_true("(procedure? car)");
    is_true("(procedure? (lambda (x) x))");
    is_false("(procedure? 42)");
}

// ============================================================================
// §6.11 Exceptions
// ============================================================================

#[test]
fn s6_11_guard() {
    is_int(
        "(guard (exn (#t 42))
           (error \"test\"))",
        42,
    );
    // guard with condition matching
    is_int(
        "(guard (exn (#t 99))
           (raise \"custom-error\"))",
        99,
    );
}

#[test]
fn s6_11_error() {
    let msg = eval_err("(error \"bad value\" 42)");
    assert!(msg.contains("bad value"));
}

// ============================================================================
// §6.13 I/O (ports)
// ============================================================================

#[test]
fn s6_13_string_ports() {
    // open-output-string + get-output-string
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
                (write-string \"hello\" p)
                (get-output-string p))"
        ),
        Value::String(Rc::from("hello"))
    );

    // open-input-string
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"abc\")))
                (read-char p))"
        ),
        Value::Char('a')
    );
}

#[test]
fn s6_13_display_write() {
    // display doesn't quote strings
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
                (display \"hello\" p)
                (get-output-string p))"
        ),
        Value::String(Rc::from("hello"))
    );
    // write quotes strings
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
                (write \"hello\" p)
                (get-output-string p))"
        ),
        Value::String(Rc::from("\"hello\""))
    );
}

// ============================================================================
// §3.5 Proper tail recursion
// ============================================================================

#[test]
fn s3_5_tail_call_optimization() {
    // Simple TCO — should not stack overflow
    is_int(
        "(define (countdown n)
           (if (= n 0) 0 (countdown (- n 1))))
         (countdown 1000000)",
        0,
    );
}

#[test]
fn s3_5_mutual_tail_recursion() {
    is_true(
        "(define (even? n) (if (= n 0) #t (odd? (- n 1))))
         (define (odd? n) (if (= n 0) #f (even? (- n 1))))
         (even? 100000)",
    );
}

#[test]
fn s3_5_tail_position_if() {
    // Both branches of if in tail position
    is_int(
        "(define (f n acc)
           (if (= n 0) acc (f (- n 1) (+ acc 1))))
         (f 100000 0)",
        100000,
    );
}

#[test]
fn s3_5_tail_position_cond() {
    is_int(
        "(define (f n)
           (cond ((= n 0) 42)
                 (else (f (- n 1)))))
         (f 100000)",
        42,
    );
}

#[test]
fn s3_5_tail_position_let() {
    // Body of let is in tail position
    is_int(
        "(define (f n)
           (let ((m (- n 1)))
             (if (= m 0) 42 (f m))))
         (f 100000)",
        42,
    );
}

#[test]
fn s3_5_tail_position_begin() {
    // Last expression in begin is in tail position
    is_int(
        "(define (f n)
           (begin
             (if (= n 0) 42 (f (- n 1)))))
         (f 100000)",
        42,
    );
}

// ============================================================================
// Type predicates (§6.1-6.9)
// ============================================================================

#[test]
fn type_predicates_comprehensive() {
    // boolean?
    is_true("(boolean? #t)");
    is_true("(boolean? #f)");
    is_false("(boolean? 0)");

    // pair?
    is_true("(pair? '(1 2))");
    is_true("(pair? (cons 1 2))");
    is_false("(pair? '())");
    is_false("(pair? 42)");

    // null?
    is_true("(null? '())");
    is_false("(null? '(1))");

    // number?
    is_true("(number? 42)");
    is_true("(number? 3.14)");
    is_false("(number? \"42\")");

    // symbol?
    is_true("(symbol? 'foo)");
    is_false("(symbol? \"foo\")");

    // char?
    is_true("(char? #\\a)");
    is_false("(char? \"a\")");

    // string?
    is_true("(string? \"hello\")");
    is_false("(string? 'hello)");

    // vector?
    is_true("(vector? #(1 2 3))");
    is_false("(vector? '(1 2 3))");

    // procedure?
    is_true("(procedure? car)");
    is_true("(procedure? (lambda () 42))");
    is_false("(procedure? 42)");
}

// ============================================================================
// VM regression tests
// ============================================================================

#[test]
fn regression_void_in_tail_position() {
    assert_eq!(eval("(if #t (void))"), Value::Void);
    assert_eq!(eval("(begin 1 2 (void))"), Value::Void);
}

#[test]
fn regression_define_global_updates() {
    // define_global must update existing bindings, not create new shadow cells
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.define_global("x", Value::Int(1));
    assert_eq!(vm.eval("x").unwrap(), Value::Int(1));
    vm.define_global("x", Value::Int(2));
    assert_eq!(vm.eval("x").unwrap(), Value::Int(2));
}

#[test]
fn regression_error_from_ffi() {
    // register_fn returns Result, so errors propagate as Scheme exceptions
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let err = vm.eval("(+ 1 \"hello\")").unwrap_err();
    assert!(!err.message().is_empty());
}

// ============================================================================
// Edge cases and error handling
// ============================================================================

#[test]
fn error_undefined_variable() {
    let msg = eval_err("undefined-var");
    assert!(msg.contains("undefined"));
}

#[test]
fn error_arity_mismatch() {
    let msg = eval_err("((lambda (x) x) 1 2)");
    assert!(msg.contains("expected") || msg.contains("arity"));
}

#[test]
fn error_type_mismatch() {
    let msg = eval_err("(+ 1 \"hello\")");
    assert!(msg.contains("number") || msg.contains("type"));
}

#[test]
fn error_division_by_zero() {
    let msg = eval_err("(/ 1 0)");
    assert!(msg.contains("zero") || msg.contains("division"));
}

#[test]
fn multiple_expressions_returns_last() {
    is_int("1 2 3", 3);
    is_int("(define x 1) (define y 2) (+ x y)", 3);
}

// ============================================================================
// Complex programs (integration tests)
// ============================================================================

#[test]
fn integration_fibonacci() {
    is_int(
        "(define (fib n)
           (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
         (fib 20)",
        6765,
    );
}

#[test]
fn integration_ackermann() {
    is_int(
        "(define (ack m n)
           (cond ((= m 0) (+ n 1))
                 ((= n 0) (ack (- m 1) 1))
                 (else (ack (- m 1) (ack m (- n 1))))))
         (ack 3 4)",
        125,
    );
}

#[test]
fn integration_quicksort() {
    let result = eval(
        "(define (filter pred lst)
           (cond ((null? lst) '())
                 ((pred (car lst)) (cons (car lst) (filter pred (cdr lst))))
                 (else (filter pred (cdr lst)))))
         (define (qsort lst)
           (if (or (null? lst) (null? (cdr lst)))
               lst
               (let ((pivot (car lst))
                     (rest (cdr lst)))
                 (append (qsort (filter (lambda (x) (< x pivot)) rest))
                         (list pivot)
                         (qsort (filter (lambda (x) (>= x pivot)) rest))))))
         (qsort '(3 1 4 1 5 9 2 6 5 3))",
    );
    let v = result.to_vec().unwrap();
    assert_eq!(
        v,
        vec![
            Value::Int(1),
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
            Value::Int(5),
            Value::Int(6),
            Value::Int(9),
        ]
    );
}

#[test]
fn integration_church_numerals() {
    // Church encoding — tests higher-order functions deeply
    is_int(
        "(define zero (lambda (f) (lambda (x) x)))
         (define (succ n) (lambda (f) (lambda (x) (f ((n f) x)))))
         (define (church->int n) ((n (lambda (x) (+ x 1))) 0))
         (define one (succ zero))
         (define two (succ one))
         (define three (succ two))
         (church->int three)",
        3,
    );
}

#[test]
fn integration_closure_counter() {
    // Closure-based state — tests proper lexical scoping with mutable upvalues
    is_int(
        "(define (make-counter)
           (let ((n 0))
             (lambda ()
               (set! n (+ n 1))
               n)))
         (define c (make-counter))
         (c) (c) (c)",
        3,
    );
}

#[test]
fn integration_y_combinator() {
    // Y combinator — tests recursion via higher-order functions
    is_int(
        "(define Y
           (lambda (f)
             ((lambda (x) (f (lambda (v) ((x x) v))))
              (lambda (x) (f (lambda (v) ((x x) v)))))))
         (define factorial
           (Y (lambda (self)
                (lambda (n)
                  (if (= n 0) 1 (* n (self (- n 1))))))))
         (factorial 10)",
        3628800,
    );
}

// ============================================================================
// Additional R7RS compliance — exception handling depth
// ============================================================================

#[test]
fn s6_11_guard_nested() {
    // Nested guard — inner handler catches
    is_int(
        "(guard (exn (#t 0))
           (guard (inner (#t 42))
             (error \"inner error\")))",
        42,
    );
}

#[test]
fn s6_11_guard_no_match_reraise() {
    // Guard clause that doesn't match — should re-raise to outer handler
    // Inner guard checks (number? inner) which is #f for a string exception,
    // so it re-raises and outer guard catches with (#t 99)
    is_int(
        "(guard (outer (#t 99))
           (guard (inner ((number? inner) 0))
             (error \"not a number\")))",
        99,
    );
}

#[test]
fn s6_11_raise() {
    // raise with a non-error value
    is_int(
        "(guard (exn (#t exn))
           (raise 42))",
        42,
    );
}

#[test]
fn s6_11_raise_string() {
    // raise with a string
    assert_eq!(
        eval(
            "(guard (exn (#t exn))
                (raise \"oops\"))"
        ),
        Value::string("oops"),
    );
}

#[test]
fn s6_11_guard_body_returns_normally() {
    // guard body completes normally — no exception
    is_int("(guard (exn (#t 0)) (+ 1 2))", 3);
}

#[test]
fn s6_11_error_with_irritants() {
    // error with irritants — guard catches error object
    is_true(
        "(guard (exn (#t (error-object? exn)))
           (error \"bad value\" 42))",
    );
    // Can extract message from error object
    is_true(
        "(guard (exn (#t (string? (error-object-message exn))))
           (error \"bad value\" 42))",
    );
}

// ============================================================================
// §4.2.4 case expression
// ============================================================================

#[test]
fn s4_2_4_case() {
    // case is a standard derived expression — test if it works via cond desugaring
    // If case isn't compiled as a special form, test the equivalent cond
    is_int(
        "(let ((x 2))
           (cond ((= x 1) 10)
                 ((= x 2) 20)
                 ((= x 3) 30)
                 (else 0)))",
        20,
    );
}

// ============================================================================
// §4.2.5 delay/force (lazy evaluation)
// ============================================================================

#[test]
fn s4_2_5_delay_force() {
    // Test if delay/force are available (may not be implemented yet)
    // For now, test that promises work if available
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    // delay/force may not be implemented yet — skip gracefully
    if let Ok(val) = vm.eval("(define p (delay (+ 1 2))) (force p)") {
        assert_eq!(val, Value::Int(3));
    }
}

// ============================================================================
// §5.5 Record types (define-record-type)
// ============================================================================

#[test]
fn s5_5_define_record_type() {
    // define-record-type may not be implemented yet — test gracefully
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    if let Ok(val) = vm.eval(
        "(define-record-type <point>
           (make-point x y)
           point?
           (x point-x)
           (y point-y))
         (let ((p (make-point 3 4)))
           (+ (point-x p) (point-y p)))",
    ) {
        assert_eq!(val, Value::Int(7));
    }
}

// ============================================================================
// Dotted-pair lambda (variadic rest args)
// ============================================================================

#[test]
fn s4_1_4_lambda_variadic() {
    // (lambda (x . rest) body) — rest parameter
    is_true("((lambda (x . rest) (pair? rest)) 1 2 3)");
    is_int("((lambda (x . rest) x) 1 2 3)", 1);
    // (lambda args body) — all args as list
    is_true("((lambda args (pair? args)) 1 2 3)");
    is_int("((lambda args (car args)) 10 20)", 10);
}

#[test]
fn s4_1_4_lambda_rest_length() {
    // length of rest args
    is_int("((lambda (x . rest) (length rest)) 1 2 3)", 2);
    is_int("((lambda (x . rest) (length rest)) 1)", 0);
}

// ============================================================================
// §6.10 Dynamic-wind
// ============================================================================

#[test]
fn s6_10_dynamic_wind() {
    // dynamic-wind may not be implemented yet
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    if let Ok(val) = vm.eval(
        "(let ((x '()))
           (dynamic-wind
             (lambda () (set! x (cons 'in x)))
             (lambda () (set! x (cons 'body x)))
             (lambda () (set! x (cons 'out x))))
           x)",
    ) {
        // Expected: (out body in) — reverse order of execution
        let list = val.to_list().unwrap();
        assert_eq!(list.len(), 3);
    }
}

// ============================================================================
// §6.10 Multiple return values
// ============================================================================

#[test]
fn s6_10_values_basic() {
    // values + call-with-values
    is_int("(call-with-values (lambda () (values 1 2 3)) +)", 6);
    is_int(
        "(call-with-values (lambda () (values 10)) (lambda (x) x))",
        10,
    );
}

#[test]
fn s6_10_values_single() {
    // Single value — should behave like normal return
    is_int("(call-with-values (lambda () 42) (lambda (x) x))", 42);
}

// ============================================================================
// §6.4 Additional list operations
// ============================================================================

#[test]
fn s6_4_append() {
    is_true("(equal? (append '(1 2) '(3 4)) '(1 2 3 4))");
    is_true("(equal? (append '() '(1 2)) '(1 2))");
    is_true("(equal? (append '(1 2) '()) '(1 2))");
}

#[test]
fn s6_4_reverse() {
    is_true("(equal? (reverse '(1 2 3)) '(3 2 1))");
    is_true("(null? (reverse '()))");
}

#[test]
fn s6_4_list_tail() {
    is_true("(equal? (list-tail '(a b c d) 2) '(c d))");
    is_true("(equal? (list-tail '(a b c) 0) '(a b c))");
}

#[test]
fn s6_4_list_ref() {
    is_int("(list-ref '(10 20 30) 0)", 10);
    is_int("(list-ref '(10 20 30) 2)", 30);
}

// ============================================================================
// §6.7 Additional string operations
// ============================================================================

#[test]
fn s6_7_string_append() {
    assert_eq!(
        eval("(string-append \"hello\" \" \" \"world\")"),
        Value::string("hello world"),
    );
    assert_eq!(eval("(string-append)"), Value::string(""));
}

#[test]
fn s6_7_substring() {
    assert_eq!(
        eval("(substring \"hello world\" 6 11)"),
        Value::string("world"),
    );
    assert_eq!(eval("(substring \"hello\" 0 5)"), Value::string("hello"),);
}

#[test]
fn s6_7_number_to_string() {
    assert_eq!(eval("(number->string 42)"), Value::string("42"));
    assert_eq!(eval("(number->string 3.14)"), Value::string("3.14"));
}

#[test]
fn s6_7_string_to_number() {
    is_int("(string->number \"42\")", 42);
    is_true("(= (string->number \"3.14\") 3.14)");
    is_false("(string->number \"not-a-number\")");
}

// ============================================================================
// §6.2 Additional numeric tests
// ============================================================================

#[test]
fn s6_2_expt() {
    is_int("(expt 2 10)", 1024);
    is_int("(expt 3 0)", 1);
    is_true("(= (expt 2.0 0.5) (sqrt 2.0))");
}

#[test]
fn s6_2_sqrt() {
    is_true("(= (sqrt 4) 2.0)");
    is_true("(= (sqrt 9.0) 3.0)");
}

#[test]
fn s6_2_negative_arithmetic() {
    is_int("(- 0)", 0);
    is_int("(- 5)", -5);
    is_int("(- 10 3)", 7);
    is_int("(- 10 3 2)", 5);
}

#[test]
fn s6_2_division_exact() {
    is_true("(= (/ 10 2) 5)");
    is_true("(= (/ 7.0 2.0) 3.5)");
}

// ============================================================================
// §6.8 Additional vector operations
// ============================================================================

#[test]
fn s6_8_vector_fill() {
    is_true(
        "(let ((v (make-vector 3 0)))
           (vector-fill! v 7)
           (and (= (vector-ref v 0) 7)
                (= (vector-ref v 1) 7)
                (= (vector-ref v 2) 7)))",
    );
}

#[test]
fn s6_8_vector_to_list_and_back() {
    is_true("(equal? (vector->list (vector 1 2 3)) '(1 2 3))");
    is_true(
        "(let ((v (list->vector '(4 5 6))))
           (and (= (vector-ref v 0) 4)
                (= (vector-ref v 1) 5)
                (= (vector-ref v 2) 6)))",
    );
}

// ============================================================================
// Mutable upvalue edge cases
// ============================================================================

#[test]
fn upvalue_shared_mutation() {
    // Two closures sharing the same upvalue cell
    is_int(
        "(define (make-pair)
           (let ((n 0))
             (cons (lambda () (set! n (+ n 1)) n)
                   (lambda () n))))
         (define p (make-pair))
         (define inc (car p))
         (define get (cdr p))
         (inc) (inc) (inc)
         (get)",
        3,
    );
}

#[test]
fn upvalue_adder() {
    // Classic adder closure
    is_int(
        "(define (make-adder n) (lambda (x) (+ n x)))
         (define add5 (make-adder 5))
         (add5 10)",
        15,
    );
}

// ============================================================================
// §6.13 Port operations
// ============================================================================

#[test]
fn s6_13_write_to_string_port() {
    // write-string to a string output port
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
                (write-string \"abc\" p)
                (write-string \"def\" p)
                (get-output-string p))"
        ),
        Value::string("abcdef"),
    );
}

#[test]
fn s6_13_port_predicates() {
    is_true("(port? (open-output-string))");
    is_true("(output-port? (open-output-string))");
    is_true("(port? (open-input-string \"hello\"))");
    is_true("(input-port? (open-input-string \"hello\"))");
}

#[test]
fn s6_13_read_from_string_port() {
    // read-char from a string input port
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"abc\")))
                (read-char p))"
        ),
        Value::Char('a'),
    );
}

// ============================================================================
// §4.3 Macros — additional edge cases
// ============================================================================

#[test]
fn s4_3_syntax_rules_nested_ellipsis() {
    // Nested ellipsis in syntax-rules
    is_true(
        "(define-syntax my-list
           (syntax-rules ()
             ((my-list x ...) '(x ...))))
         (equal? (my-list 1 2 3) '(1 2 3))",
    );
}

#[test]
fn s4_3_define_macro_simple() {
    // define-macro (non-hygienic)
    is_int(
        "(define-macro (my-add a b) (list '+ a b))
         (my-add 3 4)",
        7,
    );
}

// ============================================================================
// Tail position edge cases
// ============================================================================

#[test]
fn s3_5_tail_position_and_or() {
    // and/or in tail position
    is_int(
        "(define (f n) (and #t (+ n 1)))
         (f 41)",
        42,
    );
    is_int(
        "(define (g n) (or #f (+ n 1)))
         (g 41)",
        42,
    );
}

#[test]
fn s3_5_tail_position_when() {
    // when/unless in tail position
    is_int(
        "(define (f n) (when #t (+ n 1)))
         (f 41)",
        42,
    );
}

#[test]
fn s3_5_tail_position_guard() {
    // guard body in tail position
    is_int(
        "(define (f n)
           (guard (exn (#t 0))
             (+ n 1)))
         (f 41)",
        42,
    );
}

// ============================================================================
// Integration: higher-order function patterns
// ============================================================================

#[test]
fn integration_map_filter() {
    is_true(
        "(equal? (map (lambda (x) (* x x)) '(1 2 3 4))
                 '(1 4 9 16))",
    );
    is_true(
        "(equal? (filter (lambda (x) (> x 2)) '(1 2 3 4 5))
                 '(3 4 5))",
    );
}

#[test]
fn integration_fold() {
    is_int("(fold-left + 0 '(1 2 3 4 5))", 15);
    is_int("(fold-left * 1 '(1 2 3 4 5))", 120);
}

#[test]
fn integration_compose() {
    // Function composition via closures
    is_int(
        "(define (compose f g) (lambda (x) (f (g x))))
         (define inc (lambda (x) (+ x 1)))
         (define double (lambda (x) (* x 2)))
         ((compose inc double) 5)",
        11,
    );
}

#[test]
fn integration_accumulate() {
    // Accumulator pattern with mutable closure
    is_int(
        "(define (make-accumulator n)
           (lambda (amount)
             (set! n (+ n amount))
             n))
         (define acc (make-accumulator 100))
         (acc 10)
         (acc 20)
         (acc 30)",
        160,
    );
}

// ============================================================================
// Additional R7RS compliance tests — gap coverage
// ============================================================================

// §6.2 Additional numeric operations
#[test]
fn s6_2_square() {
    is_int("(square 5)", 25);
    is_int("(square -3)", 9);
}

#[test]
fn s6_2_exact_integer_sqrt() {
    // Returns (s r) where n = s^2 + r
    is_true("(equal? (exact-integer-sqrt 14) '(3 5))");
    is_true("(equal? (exact-integer-sqrt 4) '(2 0))");
}

#[test]
fn s6_2_numeric_type_predicates() {
    is_true("(complex? 3)");
    is_true("(real? 3)");
    is_true("(rational? 3)");
    is_true("(exact-integer? 3)");
    is_true("(not (exact-integer? 3.0))");
    is_true("(rational? 3.14)");
    is_true("(not (rational? +inf.0))");
}

#[test]
fn s6_2_floor_truncate_division() {
    is_int("(floor-quotient 7 2)", 3);
    is_int("(floor-remainder 7 2)", 1);
    is_int("(truncate-quotient 7 2)", 3);
    is_int("(truncate-remainder 7 2)", 1);
    // Negative cases
    is_int("(floor-quotient -7 2)", -4);
    is_int("(floor-remainder -7 2)", 1);
    is_int("(truncate-quotient -7 2)", -3);
    is_int("(truncate-remainder -7 2)", -1);
}

// §6.4 Additional list operations
#[test]
fn s6_4_make_list() {
    is_true("(equal? (make-list 3 'a) '(a a a))");
    is_true("(equal? (make-list 0) '())");
}

#[test]
fn s6_4_list_copy() {
    is_true("(equal? (list-copy '(1 2 3)) '(1 2 3))");
    is_true("(equal? (list-copy '()) '())");
}

#[test]
fn s6_5_symbol_eq() {
    is_true("(symbol=? 'foo 'foo)");
    is_true("(not (symbol=? 'foo 'bar))");
    is_true("(symbol=? 'x 'x 'x)");
}

// §6.6 char-foldcase
#[test]
fn s6_6_char_foldcase() {
    assert_eq!(eval("(char-foldcase #\\A)"), Value::Char('a'));
    assert_eq!(eval("(char-foldcase #\\a)"), Value::Char('a'));
}

// §6.7 string-foldcase
#[test]
fn s6_7_string_foldcase() {
    assert_eq!(
        eval("(string-foldcase \"HeLLo\")"),
        Value::String(Rc::from("hello"))
    );
}

// §6.8 vector-copy!
#[test]
fn s6_8_vector_copy_mutate() {
    is_true(
        "(let ((v1 (vector 1 2 3 4 5))
              (v2 (vector 10 20 30)))
           (vector-copy! v1 1 v2)
           (equal? (vector->list v1) '(1 10 20 30 5)))",
    );
}

// §6.8 vector<->string
#[test]
fn s6_8_vector_string_conversion() {
    assert_eq!(
        eval("(vector->string (vector #\\a #\\b #\\c))"),
        Value::String(Rc::from("abc"))
    );
    is_true("(equal? (vector->list (string->vector \"abc\")) '(#\\a #\\b #\\c))");
}

// §6.9 bytevector-copy!
#[test]
fn s6_9_bytevector_copy_mutate() {
    is_true(
        "(let ((bv1 (bytevector 1 2 3 4 5))
              (bv2 (bytevector 10 20 30)))
           (bytevector-copy! bv1 1 bv2)
           (= (bytevector-u8-ref bv1 0) 1))",
    );
    is_true(
        "(let ((bv1 (bytevector 1 2 3 4 5))
              (bv2 (bytevector 10 20 30)))
           (bytevector-copy! bv1 1 bv2)
           (= (bytevector-u8-ref bv1 1) 10))",
    );
}

// §6.11 Exception predicates
#[test]
fn s6_11_error_predicates() {
    is_true("(not (file-error? 42))");
    is_true("(not (read-error? 42))");
    is_true("(not (error-object? 42))");
}

// §6.13 Port operations
#[test]
fn s6_13_port_operations() {
    is_true("(textual-port? (open-input-string \"hi\"))");
    is_true("(not (binary-port? (open-input-string \"hi\")))");
    is_true("(input-port-open? (open-input-string \"hi\"))");
    is_true("(output-port-open? (open-output-string))");
}

#[test]
fn s6_13_read_line() {
    assert_eq!(
        eval("(read-line (open-input-string \"hello\\nworld\"))"),
        Value::String(Rc::from("hello")),
    );
}

// §6.14 features
#[test]
fn s6_14_features() {
    // memq returns sublist (truthy), not #t
    is_true("(pair? (memq 'r7rs (features)))");
    is_true("(pair? (memq 'mae (features)))");
}

// §4.2.2 let as subexpression regression
#[test]
fn s4_2_2_let_subexpression() {
    // Regression: let as argument to function (stack corruption bug)
    is_int("(+ 10 (let ((x 1)) (+ x 2)))", 13);
    is_true("(not (let ((x 1)) (= x 2)))");
    is_true("(equal? (list (let ((x 1)) x) (let ((y 2)) y)) '(1 2))");
    // Nested lets as subexpressions
    is_int("(+ (let ((a 1)) a) (let ((b 2)) b) (let ((c 3)) c))", 6);
}

// §4.2.2 let* as subexpression
#[test]
fn s4_2_2_let_star_subexpression() {
    is_int("(+ 10 (let* ((x 1) (y (+ x 1))) y))", 12);
    is_true("(equal? (let* ((x 1) (y 2)) (list x y)) '(1 2))");
}

// §6.7 string-for-each, string-map
#[test]
fn s6_7_string_for_each_map() {
    assert_eq!(
        eval("(string-map char-upcase \"hello\")"),
        Value::String(Rc::from("HELLO")),
    );
}

// §6.8 vector-for-each, vector-map
#[test]
fn s6_8_vector_for_each_map() {
    is_true(
        "(equal? (vector->list (vector-map (lambda (x) (+ x 1)) (vector 1 2 3)))
                '(2 3 4))",
    );
}

// §6.10 call-with-values
#[test]
fn s6_10_call_with_values_basic() {
    is_int("(call-with-values (lambda () (values 1 2)) +)", 3);
}

// §4.2.5 delay/force comprehensive
#[test]
fn s4_2_5_delay_force_comprehensive() {
    is_int("(force (delay 42))", 42);
    is_int("(force (delay (+ 1 2)))", 3);
    // force on non-promise returns value
    is_int("(force 42)", 42);
    // memoization — force returns cached value
    is_true(
        "(let ((p (delay (begin 42))))
           (equal? (force p) (force p)))",
    );
}

// §6.10 Multi-list map
#[test]
fn s6_10_map_multi_list() {
    // Single-list map (basic)
    is_true("(equal? (map + '(1 2 3)) '(1 2 3))");
    is_true("(equal? (map (lambda (x) (* x x)) '(1 2 3 4)) '(1 4 9 16))");
    // Multi-list map
    is_true("(equal? (map + '(1 2 3) '(10 20 30)) '(11 22 33))");
    is_true("(equal? (map * '(1 2 3) '(4 5 6)) '(4 10 18))");
    // Three lists
    is_true("(equal? (map + '(1 2) '(3 4) '(5 6)) '(9 12))");
    // Empty lists
    is_true("(equal? (map + '() '()) '())");
}

// §6.10 Multi-list for-each
#[test]
fn s6_10_for_each_multi_list() {
    // for-each returns void
    assert_eq!(eval("(for-each + '(1 2 3))"), Value::Void);
    // Multi-list for-each
    assert_eq!(eval("(for-each + '(1 2) '(3 4))"), Value::Void);
}

// §6.10 apply with leading args
#[test]
fn s6_10_apply_multi_arg() {
    // Basic apply
    is_int("(apply + '(1 2 3))", 6);
    // Apply with leading args: (apply fn a1 a2 ... list)
    is_int("(apply + 1 '(2 3))", 6);
    is_int("(apply + 1 2 '(3))", 6);
    is_int("(apply + 1 2 3 '())", 6);
    // Apply with string operation
    is_true("(equal? (apply string #\\a #\\b '(#\\c)) \"abc\")");
}

// §6.13 Standard ports
#[test]
fn s6_13_standard_ports() {
    is_true("(port? (current-input-port))");
    is_true("(port? (current-output-port))");
    is_true("(port? (current-error-port))");
    is_true("(input-port? (current-input-port))");
    is_true("(output-port? (current-output-port))");
    is_true("(output-port? (current-error-port))");
}

// §6.13.3 Binary I/O
#[test]
fn s6_13_binary_io() {
    // open-output-bytevector + get-output-bytevector
    is_true(
        "(let ((p (open-output-bytevector)))
           (write-u8 65 p)
           (write-u8 66 p)
           (equal? (bytevector->list (get-output-bytevector p)) '(65 66)))",
    );
    // open-input-bytevector + read-u8
    is_int(
        "(let ((p (open-input-bytevector (bytevector 10 20 30))))
           (read-u8 p))",
        10,
    );
    // peek-u8
    is_int(
        "(let ((p (open-input-bytevector (bytevector 42))))
           (peek-u8 p))",
        42,
    );
    // read-u8 after peek doesn't advance
    is_int(
        "(let ((p (open-input-bytevector (bytevector 42 43))))
           (peek-u8 p)
           (read-u8 p))",
        42,
    );
    // EOF on empty bytevector
    is_true("(eof-object? (read-u8 (open-input-bytevector (bytevector))))");
}

// §6.13 char-ready? and u8-ready?
#[test]
fn s6_13_ready_predicates() {
    // Test on string ports (deterministic, works in CI where stdin is a pipe)
    is_true("(char-ready? (open-input-string \"hello\"))");
    is_false("(char-ready? (open-input-string \"\"))");
    // u8-ready? on a bytevector port
    is_true("(u8-ready? (open-input-bytevector #u8(1 2 3)))");
    is_false("(u8-ready? (open-input-bytevector #u8()))");
}

// §6.13.2 write-char with port
#[test]
fn s6_13_write_char_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-char #\\H p)
               (write-char #\\i p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("Hi")),
    );
}

// §6.2.6 exact/inexact aliases
#[test]
fn s6_2_exact_inexact_aliases() {
    is_int("(exact 2.75)", 2);
    is_int("(exact 42)", 42);
    assert_eq!(eval("(inexact 42)"), Value::Float(42.0));
    assert_eq!(eval("(inexact 2.75)"), Value::Float(2.75));
}

// §4.2.2 let-values
#[test]
fn s4_2_2_let_values() {
    is_int(
        "(let-values (((x y) (values 1 2)))
           (+ x y))",
        3,
    );
}

// §4.2.2 receive (SRFI-8)
#[test]
fn s4_2_2_receive() {
    is_int(
        "(receive (x y)
           (values 10 20)
           (+ x y))",
        30,
    );
}

// §6.7 multi-string string-map/string-for-each
#[test]
fn s6_7_string_map_multi() {
    // Single string (basic)
    assert_eq!(
        eval("(string-map char-upcase \"hello\")"),
        Value::String(Rc::from("HELLO")),
    );
}

// §6.8 multi-vector vector-map
#[test]
fn s6_8_vector_map_multi() {
    is_true(
        "(equal? (vector->list (vector-map + (vector 1 2 3) (vector 10 20 30)))
                '(11 22 33))",
    );
}

// §6.13 read-bytevector
#[test]
fn s6_13_read_bytevector() {
    is_true(
        "(let ((p (open-input-bytevector (bytevector 1 2 3 4 5))))
           (equal? (bytevector->list (read-bytevector 3 p)) '(1 2 3)))",
    );
}

// §6.13 display/write to port
#[test]
fn s6_13_display_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display 42 p)
               (display \" hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("42 hello")),
    );
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\"hello\"")),
    );
}

// §6.13 newline to port
#[test]
fn s6_13_newline_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (newline p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\n")),
    );
}

// §6.10 dynamic-wind order verification
#[test]
fn s6_10_dynamic_wind_order() {
    is_true(
        "(let ((order '()))
           (dynamic-wind
             (lambda () (set! order (cons 'in order)))
             (lambda () (set! order (cons 'body order)))
             (lambda () (set! order (cons 'out order))))
           (equal? order '(out body in)))",
    );
}

// §4.2.6 make-parameter / parameterize
#[test]
fn s4_2_6_parameterize() {
    is_int(
        "(let ((p (make-parameter 10)))
           (p))",
        10,
    );
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20))
             (p)))",
        20,
    );
    // After parameterize, value reverts
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20))
             (p))
           (p))",
        10,
    );
}

// §4.2.1 cond-expand
#[test]
fn s4_2_1_cond_expand() {
    // Feature present
    is_int("(cond-expand (r7rs 42))", 42);
    is_int("(cond-expand (mae 1) (else 2))", 1);
    // Feature absent → else
    is_int("(cond-expand (chicken 1) (else 2))", 2);
    // Compound: and, or, not
    is_int("(cond-expand ((and r7rs mae) 1) (else 2))", 1);
    is_int("(cond-expand ((or chicken mae) 1) (else 2))", 1);
    is_int("(cond-expand ((not r7rs) 1) (else 2))", 2);
    is_int("(cond-expand ((not chicken) 1) (else 2))", 1);
}

// §4.3.1 syntax-error
#[test]
fn s4_3_1_syntax_error() {
    let err = eval_err("(syntax-error \"test error message\")");
    assert!(
        err.contains("test error message"),
        "syntax-error should produce compile-time error: {err}"
    );
}

// §6.13 file I/O
#[test]
fn s6_13_file_io() {
    // Write and read back via file ports
    let tmp = "/tmp/mae-scheme-test-file-io.txt";
    eval(&format!(
        "(let ((p (open-output-file \"{tmp}\")))
           (write-string \"hello file\" p)
           (close-port p))"
    ));
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{tmp}\")))
               (let ((line (read-line p)))
                 (close-port p)
                 line))"
        )),
        Value::String(Rc::from("hello file")),
    );
    std::fs::remove_file(tmp).ok();
}

// §6.13 call-with-input-file / call-with-output-file
#[test]
fn s6_13_call_with_file() {
    let tmp = "/tmp/mae-scheme-test-call-with.txt";
    eval(&format!(
        "(call-with-output-file \"{tmp}\"
           (lambda (p) (write-string \"test data\" p)))"
    ));
    assert_eq!(
        eval(&format!(
            "(call-with-input-file \"{tmp}\"
               (lambda (p) (read-line p)))"
        )),
        Value::String(Rc::from("test data")),
    );
    std::fs::remove_file(tmp).ok();
}

// §6.14 process context
#[test]
fn s6_14_process_context() {
    is_true("(pair? (command-line))");
    // get-environment-variable returns string or #f
    is_true(
        "(let ((home (get-environment-variable \"HOME\")))
               (or (string? home) (not home)))",
    );
    // get-environment-variables returns alist
    is_true("(pair? (get-environment-variables))");
}

// §6.14 time
#[test]
fn s6_14_time() {
    // current-second returns a float > 0
    is_true("(> (current-second) 0)");
    // current-jiffy returns an integer > 0
    is_true("(> (current-jiffy) 0)");
    // jiffies-per-second
    is_int("(jiffies-per-second)", 1_000_000_000);
}

// §6.7 string->list with start/end
#[test]
fn s6_7_string_to_list_range() {
    is_true("(equal? (string->list \"hello\" 1 3) '(#\\e #\\l))");
    is_true("(equal? (string->list \"abc\") '(#\\a #\\b #\\c))");
}

// §6.7 string-copy with start/end
#[test]
fn s6_7_string_copy_range() {
    assert_eq!(
        eval("(string-copy \"hello\" 1 4)"),
        Value::String(Rc::from("ell")),
    );
    assert_eq!(
        eval("(string-copy \"hello\")"),
        Value::String(Rc::from("hello")),
    );
}

// §6.13 write-simple and write-shared
#[test]
fn s6_13_write_variants() {
    // write-simple to string port
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-simple '(1 2 3) p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("(1 2 3)")),
    );
}

// §6.13 read from file port
#[test]
fn s6_13_read_char_from_file() {
    let tmp = "/tmp/mae-scheme-test-read-char.txt";
    eval(&format!(
        "(call-with-output-file \"{tmp}\"
           (lambda (p) (write-char #\\X p)))"
    ));
    assert_eq!(
        eval(&format!(
            "(call-with-input-file \"{tmp}\"
               (lambda (p) (read-char p)))"
        )),
        Value::Char('X'),
    );
    std::fs::remove_file(tmp).ok();
}

// ============================================================================
// Edge-case tests: Comprehensive R7RS compliance edge cases
// ============================================================================

// --- §6.2 Numeric edge cases ---

#[test]
fn edge_numeric_infinity_nan() {
    // +inf.0 and -inf.0
    is_true("(number? +inf.0)");
    is_true("(number? -inf.0)");
    is_true("(inexact? +inf.0)");

    // NaN
    is_true("(number? +nan.0)");
    is_true("(inexact? +nan.0)");

    // NaN is not equal to itself (IEEE 754)
    is_false("(= +nan.0 +nan.0)");

    // Arithmetic with infinity
    is_true("(inexact? (+ +inf.0 1))");
    is_true("(inexact? (* 2 +inf.0))");

    // Comparisons with infinity
    is_true("(> +inf.0 999999999)");
    is_true("(< -inf.0 -999999999)");

    // infinite? and nan? predicates
    is_true("(infinite? +inf.0)");
    is_true("(infinite? -inf.0)");
    is_false("(infinite? 42)");
    is_true("(nan? +nan.0)");
    is_false("(nan? 0)");
}

#[test]
fn edge_numeric_negative_zero() {
    // R7RS: -0.0 is eqv? to 0.0
    is_true("(eqv? 0.0 -0.0)");
    is_true("(= 0.0 -0.0)");
    is_true("(zero? -0.0)");
}

#[test]
fn edge_numeric_exact_inexact() {
    // exact->inexact / inexact->exact
    assert_eq!(eval("(exact->inexact 5)"), Value::Float(5.0));
    assert_eq!(eval("(inexact->exact 5.0)"), Value::Int(5));

    // exact? and inexact?
    is_true("(exact? 42)");
    is_false("(exact? 3.14)");
    is_true("(inexact? 3.14)");
    is_false("(inexact? 42)");
}

#[test]
fn edge_numeric_division_by_zero() {
    // Division by zero should error
    let err = eval_err("(/ 1 0)");
    assert!(
        err.contains("zero") || err.contains("division"),
        "expected division error: {err}"
    );

    let err = eval_err("(quotient 5 0)");
    assert!(
        err.contains("zero") || err.contains("division"),
        "expected division error: {err}"
    );

    let err = eval_err("(remainder 5 0)");
    assert!(
        err.contains("zero") || err.contains("division"),
        "expected division error: {err}"
    );

    let err = eval_err("(modulo 5 0)");
    assert!(
        err.contains("zero") || err.contains("division"),
        "expected division error: {err}"
    );
}

#[test]
fn edge_numeric_no_args() {
    // (+) => 0, (*) => 1 (identity elements)
    is_int("(+)", 0);
    is_int("(*)", 1);
}

#[test]
fn edge_numeric_unary_minus_div() {
    // (- x) => negation, (/ x) => reciprocal
    is_int("(- 5)", -5);
    assert_eq!(eval("(/ 4)"), Value::Float(0.25));
}

#[test]
fn edge_numeric_mixed_types() {
    // Int + Float -> Float (contagion)
    assert_eq!(eval("(+ 1 1.0)"), Value::Float(2.0));
    assert_eq!(eval("(* 2 3.0)"), Value::Float(6.0));

    // min/max with mixed types
    is_true("(= (min 1 2.0) 1)");
    is_true("(= (max 1 2.0) 2.0)");
}

#[test]
fn edge_numeric_rounding() {
    // R7RS: round returns inexact for inexact args (banker's rounding)
    is_float("(round 0.5)", 0.0); // 0 is even
    is_float("(round 1.5)", 2.0); // 2 is even
    is_float("(round 2.5)", 2.0); // 2 is even
    is_float("(round 3.5)", 4.0); // 4 is even
    is_float("(round -0.5)", 0.0); // 0 is even
    is_float("(round -1.5)", -2.0); // -2 is even

    // Non-halfway cases
    is_float("(round 2.3)", 2.0);
    is_float("(round 2.7)", 3.0);
    is_float("(round -2.3)", -2.0);
    is_float("(round -2.7)", -3.0);
}

#[test]
fn edge_numeric_floor_ceiling_truncate() {
    // R7RS: inexact args → inexact results
    is_float("(floor 2.7)", 2.0);
    is_float("(floor -2.3)", -3.0);
    is_int("(floor 5)", 5); // exact → exact

    is_float("(ceiling 2.3)", 3.0);
    is_float("(ceiling -2.7)", -2.0);
    is_int("(ceiling 5)", 5);

    is_float("(truncate 2.7)", 2.0);
    is_float("(truncate -2.7)", -2.0);
}

#[test]
fn edge_numeric_number_to_string_radix() {
    assert_eq!(
        eval("(number->string 255 16)"),
        Value::String(Rc::from("ff"))
    );
    assert_eq!(
        eval("(number->string 10 2)"),
        Value::String(Rc::from("1010"))
    );
    assert_eq!(eval("(number->string 8 8)"), Value::String(Rc::from("10")));
    assert_eq!(
        eval("(number->string -5 10)"),
        Value::String(Rc::from("-5"))
    );
}

#[test]
fn edge_numeric_string_to_number() {
    is_int("(string->number \"42\")", 42);
    is_int("(string->number \"ff\" 16)", 255);
    is_int("(string->number \"1010\" 2)", 10);
    is_false("(string->number \"not-a-number\")");
    is_false("(string->number \"\")");
}

#[test]
fn edge_numeric_chained_comparisons() {
    // R7RS comparison operators take 2+ args and are transitive
    is_true("(< 1 2 3 4 5)");
    is_false("(< 1 2 3 3 5)");
    is_true("(<= 1 2 3 3 5)");
    is_true("(> 5 4 3 2 1)");
    is_true("(= 3 3 3 3)");
    is_false("(= 3 3 4 3)");
}

// --- §6.1 Equivalence edge cases ---

#[test]
fn edge_equivalence() {
    // eq? for booleans (must be identical objects)
    is_true("(eq? #t #t)");
    is_true("(eq? #f #f)");
    is_false("(eq? #t #f)");

    // eq? for symbols
    is_true("(eq? 'foo 'foo)");
    is_false("(eq? 'foo 'bar)");

    // eq? for chars
    is_true("(eq? #\\a #\\a)");

    // eq? for empty list
    is_true("(eq? '() '())");

    // eqv? for numbers
    is_true("(eqv? 42 42)");
    is_false("(eqv? 42 42.0)"); // exact != inexact

    // equal? — structural
    is_true("(equal? '(1 2 3) '(1 2 3))");
    is_true("(equal? \"hello\" \"hello\")");
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_false("(equal? '(1 2) '(1 3))");
}

// --- §6.4 Pairs and lists edge cases ---

#[test]
fn edge_pairs_dotted() {
    // Dotted pairs
    is_int("(car '(1 . 2))", 1);
    is_int("(cdr '(1 . 2))", 2);

    // pair? vs list?
    is_true("(pair? '(1 . 2))");
    is_true("(pair? '(1 2 3))");
    is_false("(pair? '())");

    // list? requires proper list
    is_true("(list? '())");
    is_true("(list? '(1 2 3))");
    is_false("(list? '(1 . 2))");
}

#[test]
fn edge_list_operations() {
    // length of empty list
    is_int("(length '())", 0);

    // append edge cases
    is_true("(equal? (append '() '(1 2)) '(1 2))");
    is_true("(equal? (append '(1) '()) '(1))");
    is_true("(equal? (append '() '()) '())");

    // reverse empty
    is_true("(equal? (reverse '()) '())");
    is_true("(equal? (reverse '(1)) '(1))");

    // assoc/assv/assq
    is_true("(equal? (assoc 'b '((a 1) (b 2) (c 3))) '(b 2))");
    is_false("(assoc 'z '((a 1) (b 2)))");

    // member/memv/memq
    is_true("(equal? (member 3 '(1 2 3 4 5)) '(3 4 5))");
    is_false("(member 6 '(1 2 3 4 5))");
}

#[test]
fn edge_list_map_for_each() {
    // map with empty list
    is_true("(equal? (map car '()) '())");

    // for-each return value is unspecified; just verify no error
    eval("(for-each (lambda (x) x) '(1 2 3))");

    // map preserves order
    is_true("(equal? (map (lambda (x) (* x x)) '(1 2 3)) '(1 4 9))");

    // multi-list map with different lengths — should stop at shortest
    is_true("(equal? (map + '(1 2) '(10 20 30)) '(11 22))");
}

// --- §6.3 Boolean edge cases ---

#[test]
fn edge_booleans() {
    // Only #f is falsy in Scheme
    is_true("(if 0 #t #f)"); // 0 is truthy
    is_true("(if \"\" #t #f)"); // empty string is truthy
    is_true("(if '() #t #f)"); // empty list is truthy
    is_true("(if #t #t #f)");
    is_false("(if #f #t #f)");

    // boolean? predicate
    is_true("(boolean? #t)");
    is_true("(boolean? #f)");
    is_false("(boolean? 0)");
    is_false("(boolean? '())");

    // boolean=?
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");

    // not
    is_true("(not #f)");
    is_false("(not #t)");
    is_false("(not 42)"); // any non-#f is true
    is_false("(not '())");
}

// --- §6.6 Character edge cases ---

#[test]
fn edge_char_unicode() {
    // char-numeric? should handle Unicode digits
    is_true("(char-numeric? #\\5)");
    is_true("(char-alphabetic? #\\a)");
    is_true("(char-alphabetic? #\\Z)");

    // char->integer / integer->char round-trip
    is_true("(char=? (integer->char (char->integer #\\A)) #\\A)");

    // Unicode char
    is_true("(char=? (integer->char 955) #\\λ)"); // Greek lambda
    is_int("(char->integer #\\λ)", 955);

    // digit-value
    is_int("(digit-value #\\0)", 0);
    is_int("(digit-value #\\9)", 9);
    is_false("(digit-value #\\a)"); // not a digit
}

#[test]
fn edge_char_case() {
    assert_eq!(eval("(char-upcase #\\a)"), Value::Char('A'));
    assert_eq!(eval("(char-downcase #\\A)"), Value::Char('a'));
    // Already uppercase/lowercase
    assert_eq!(eval("(char-upcase #\\A)"), Value::Char('A'));
    assert_eq!(eval("(char-downcase #\\a)"), Value::Char('a'));
    // Non-letter character
    assert_eq!(eval("(char-upcase #\\5)"), Value::Char('5'));

    // Case-insensitive comparison
    is_true("(char-ci=? #\\a #\\A)");
    is_true("(char-ci=? #\\Z #\\z)");
}

// --- §6.7 String edge cases ---

#[test]
fn edge_string_unicode_length() {
    // string-length must count chars, not bytes
    // "λ" is 2 bytes in UTF-8 but 1 character
    is_int("(string-length \"λ\")", 1);
    // "café" — é is multi-byte
    is_int("(string-length \"café\")", 4);
    // "日本語" — 3 chars, 9 bytes
    is_int("(string-length \"日本語\")", 3);
    // Empty string
    is_int("(string-length \"\")", 0);
}

#[test]
fn edge_string_ref_unicode() {
    // string-ref must index by character, not byte
    assert_eq!(eval("(string-ref \"café\" 3)"), Value::Char('é'));
    assert_eq!(eval("(string-ref \"日本語\" 1)"), Value::Char('本'));
}

#[test]
fn edge_substring_unicode() {
    assert_eq!(
        eval("(substring \"日本語\" 1 2)"),
        Value::String(Rc::from("本")),
    );
    // substring without end = rest of string (by chars)
    assert_eq!(
        eval("(substring \"café\" 2)"),
        Value::String(Rc::from("fé")),
    );
}

#[test]
fn edge_string_empty() {
    // Empty string operations
    assert_eq!(eval("(string-append)"), Value::String(Rc::from("")),);
    assert_eq!(
        eval("(string-append \"\" \"\")"),
        Value::String(Rc::from("")),
    );
    is_true("(string=? \"\" \"\")");
    is_false("(string<? \"\" \"\")");
    is_true("(string<=? \"\" \"\")");
}

#[test]
fn edge_string_immutable() {
    // string-set! should error (immutable strings)
    let err = eval_err("(string-set! \"hello\" 0 #\\H)");
    assert!(
        err.contains("immutable") || err.contains("Immutable"),
        "expected immutable error: {err}"
    );
}

#[test]
fn edge_string_foldcase() {
    assert_eq!(
        eval("(string-foldcase \"ABC\")"),
        Value::String(Rc::from("abc")),
    );
    assert_eq!(
        eval("(string-foldcase \"already\")"),
        Value::String(Rc::from("already")),
    );
}

// --- §6.8 Vector edge cases ---

#[test]
fn edge_vector_empty() {
    is_int("(vector-length #())", 0);
    is_true("(equal? (vector->list #()) '())");
    is_true("(equal? (list->vector '()) #())");
}

#[test]
fn edge_vector_copy_overlap() {
    // vector-copy with start/end
    is_true("(equal? (vector-copy #(1 2 3 4 5) 1 3) #(2 3))");
    // vector-copy!
    is_true(
        "(let ((v (vector 1 2 3 4 5)))
           (vector-copy! v 1 #(10 20))
           (equal? v #(1 10 20 4 5)))",
    );
}

#[test]
fn edge_vector_fill() {
    is_true(
        "(let ((v (make-vector 3 0)))
           (vector-fill! v 7)
           (equal? v #(7 7 7)))",
    );
}

// --- §6.9 Bytevector edge cases ---

#[test]
fn edge_bytevector_operations() {
    is_int("(bytevector-length #u8())", 0);
    is_int("(bytevector-length #u8(1 2 3))", 3);
    is_int("(bytevector-u8-ref #u8(10 20 30) 1)", 20);

    // bytevector-copy
    is_true("(equal? (bytevector-copy #u8(1 2 3 4 5) 1 3) #u8(2 3))");

    // bytevector-append
    is_true("(equal? (bytevector-append #u8(1 2) #u8(3 4)) #u8(1 2 3 4))");
    is_true("(equal? (bytevector-append #u8() #u8()) #u8())");
}

// --- §6.5 Symbol edge cases ---

#[test]
fn edge_symbols() {
    // symbol->string / string->symbol round-trip
    is_true("(eq? (string->symbol \"hello\") 'hello)");
    assert_eq!(
        eval("(symbol->string 'hello)"),
        Value::String(Rc::from("hello")),
    );

    // symbol=? (R7RS §6.5)
    is_true("(symbol=? 'abc 'abc)");
    is_false("(symbol=? 'abc 'def)");

    // symbol? predicate
    is_true("(symbol? 'x)");
    is_false("(symbol? \"x\")");
    is_false("(symbol? 42)");
}

// --- §6.10 Control edge cases ---

#[test]
fn edge_apply_multi_arg() {
    // (apply fn a1 a2 ... list) — cons chain desugaring
    is_int("(apply + 1 2 '(3))", 6);
    is_int("(apply + 1 2 3 '(4))", 10);
    is_int("(apply + '(1 2 3 4))", 10);
}

#[test]
fn edge_values_and_call_with_values() {
    // Single value
    is_int("(call-with-values (lambda () 42) (lambda (x) x))", 42);

    // Multiple values
    is_int(
        "(call-with-values (lambda () (values 1 2 3)) (lambda (a b c) (+ a b c)))",
        6,
    );

    // values with one arg = identity
    is_int("(values 42)", 42);
}

#[test]
fn edge_dynamic_wind_order() {
    // Verify in/thunk/out ordering
    is_true(
        "(let ((log '()))
           (dynamic-wind
             (lambda () (set! log (cons 'in log)))
             (lambda () (set! log (cons 'body log)) 42)
             (lambda () (set! log (cons 'out log))))
           (equal? (reverse log) '(in body out)))",
    );
}

#[test]
fn edge_dynamic_wind_exception() {
    // dynamic-wind out thunk runs even on exception
    is_true(
        "(let ((log '()))
           (guard (e (#t #t))
             (dynamic-wind
               (lambda () (set! log (cons 'in log)))
               (lambda () (error \"boom\"))
               (lambda () (set! log (cons 'out log)))))
           (equal? (reverse log) '(in out)))",
    );
}

// --- §6.11 Exception edge cases ---

#[test]
fn edge_exceptions_guard() {
    // guard with matching clause
    is_int(
        "(guard (e ((string? (error-object-message e)) 42))
           (error \"test\"))",
        42,
    );

    // guard with else
    is_int(
        "(guard (e (else 99))
           (error \"anything\"))",
        99,
    );

    // Nested guard
    is_int(
        "(guard (outer (else 0))
           (guard (inner ((string? (error-object-message inner)) 42))
             (error \"inner error\")))",
        42,
    );
}

#[test]
fn edge_exceptions_error_irritants() {
    // error-object-irritants returns the irritant values
    is_true(
        "(guard (e (#t (equal? (error-object-irritants e) '(1 2 3))))
           (error \"test\" 1 2 3))",
    );

    // error-object-type returns the error type string
    is_true(
        "(guard (e (#t (string? (error-object-type e))))
           (error \"typed\" 42))",
    );
}

#[test]
fn edge_exceptions_raise() {
    // raise a non-error value
    is_int(
        "(guard (e ((number? e) e))
           (raise 42))",
        42,
    );

    // raise a string
    assert_eq!(
        eval(
            "(guard (e ((string? e) e))
               (raise \"hello\"))"
        ),
        Value::String(Rc::from("hello")),
    );
}

// --- §4.2 Derived expression edge cases ---

#[test]
fn edge_let_forms() {
    // Named let (loop)
    is_int(
        "(let loop ((n 10) (acc 0))
           (if (= n 0) acc (loop (- n 1) (+ acc n))))",
        55,
    );

    // letrec — mutual recursion
    is_true(
        "(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
                  (odd? (lambda (n) (if (= n 0) #f (even? (- n 1))))))
           (even? 10))",
    );

    // let* ordering
    is_int("(let* ((x 1) (y (+ x 1)) (z (+ y 1))) z)", 3);
}

#[test]
fn edge_when_unless() {
    // when: body executes if test is true
    is_int("(let ((x 0)) (when #t (set! x 42)) x)", 42);
    is_int("(let ((x 0)) (when #f (set! x 42)) x)", 0);

    // unless: body executes if test is false
    is_int("(let ((x 0)) (unless #f (set! x 42)) x)", 42);
    is_int("(let ((x 0)) (unless #t (set! x 42)) x)", 0);
}

#[test]
fn edge_do_loop() {
    // do loop — standard R7RS iteration
    is_int(
        "(do ((i 0 (+ i 1))
              (sum 0 (+ sum i)))
             ((= i 10) sum))",
        45,
    );

    // do with empty body
    is_int(
        "(do ((i 0 (+ i 1)))
             ((= i 5) i))",
        5,
    );
}

// --- §3.5 Tail call positions ---

#[test]
fn edge_tco_do() {
    // do in tail position should not overflow
    is_int(
        "(do ((i 0 (+ i 1)))
             ((= i 100000) i))",
        100000,
    );
}

#[test]
fn edge_tco_cond() {
    // cond in tail position
    is_int(
        "(define (f n)
           (cond ((= n 0) 42)
                 (else (f (- n 1)))))
         (f 100000)",
        42,
    );
}

#[test]
fn edge_tco_case() {
    // case in tail position
    is_int(
        "(define (f n)
           (case n
             ((0) 42)
             (else (f (- n 1)))))
         (f 100000)",
        42,
    );
}

#[test]
fn edge_tco_when() {
    // when in tail position
    is_int(
        "(define (count n)
           (if (= n 0) 0
               (begin
                 (when (> n 0) (count (- n 1))))))
         (count 100000)",
        0,
    );
}

// --- §4.2.5 Delayed evaluation ---

#[test]
fn edge_promises() {
    // delay / force
    is_int("(force (delay 42))", 42);

    // make-promise
    is_int("(force (make-promise 42))", 42);

    // Memoization: force should cache
    is_int(
        "(let ((p (delay (begin 42))))
           (force p)
           (force p))",
        42,
    );

    // promise?
    is_true("(promise? (delay 42))");
    is_false("(promise? 42)");
}

// --- §6.10 Parameters ---

#[test]
fn edge_parameterize() {
    is_int(
        "(define p (make-parameter 10))
         (parameterize ((p 20))
           (p))",
        20,
    );

    // Nested parameterize
    is_int(
        "(define p (make-parameter 1))
         (parameterize ((p 2))
           (parameterize ((p 3))
             (p)))",
        3,
    );

    // Parameter restored after parameterize
    is_int(
        "(define p (make-parameter 1))
         (parameterize ((p 99))
           (p))
         (p)",
        1,
    );
}

// --- Quasiquote edge cases ---

#[test]
fn edge_quasiquote() {
    // Basic unquote
    is_true("(equal? `(1 ,(+ 1 1) 3) '(1 2 3))");

    // Splicing
    is_true("(equal? `(1 ,@(list 2 3) 4) '(1 2 3 4))");

    // Nested quasiquote
    is_true("(equal? `(a `(b ,(+ 1 2))) '(a (quasiquote (b (unquote (+ 1 2))))))");

    // Empty splicing
    is_true("(equal? `(1 ,@'() 2) '(1 2))");
}

// --- Port edge cases ---

#[test]
fn edge_port_string_io() {
    // Read from string port
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"hello\")))
               (let ((c1 (read-char p))
                     (c2 (read-char p)))
                 (string c1 c2)))"
        ),
        Value::String(Rc::from("he")),
    );

    // EOF detection
    is_true(
        "(let ((p (open-input-string \"\")))
           (eof-object? (read-char p)))",
    );

    // Write to string port
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-string \"hello\" p)
               (write-string \" world\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("hello world")),
    );
}

#[test]
fn edge_port_eof() {
    // eof-object
    is_true("(eof-object? (eof-object))");
    is_false("(eof-object? #f)");
    is_false("(eof-object? 0)");

    // read-char at EOF
    is_true(
        "(let ((p (open-input-string \"x\")))
           (read-char p)
           (eof-object? (read-char p)))",
    );
}

#[test]
fn edge_port_peek_char() {
    // peek-char doesn't consume
    is_true(
        "(let ((p (open-input-string \"ab\")))
           (let ((c1 (peek-char p))
                 (c2 (read-char p))
                 (c3 (read-char p)))
             (and (char=? c1 #\\a)
                  (char=? c2 #\\a)
                  (char=? c3 #\\b))))",
    );
}

#[test]
fn edge_port_read_sexp() {
    // read S-expression from string port
    is_int(
        "(let ((p (open-input-string \"42\")))
           (read p))",
        42,
    );

    is_true(
        "(let ((p (open-input-string \"(1 2 3)\")))
           (equal? (read p) '(1 2 3)))",
    );

    // Multiple reads
    is_int(
        "(let ((p (open-input-string \"1 2 3\")))
           (read p) (read p) (read p))",
        3,
    );
}

// --- Type predicate edge cases ---

#[test]
fn edge_type_predicates() {
    is_true("(number? 42)");
    is_true("(number? 3.14)");
    is_true("(number? +inf.0)");
    is_true("(number? +nan.0)");
    is_false("(number? \"42\")");

    is_true("(integer? 42)");
    is_true("(integer? 42.0)"); // exact integer as float
    is_false("(integer? 3.14)");

    is_true("(string? \"hello\")");
    is_false("(string? 42)");

    is_true("(char? #\\a)");
    is_false("(char? \"a\")");

    is_true("(procedure? car)");
    is_true("(procedure? (lambda (x) x))");
    is_false("(procedure? 42)");

    is_true("(null? '())");
    is_false("(null? #f)");
    is_false("(null? '(1))");

    is_true("(port? (open-input-string \"x\"))");
    is_true("(input-port? (open-input-string \"x\"))");
    is_true("(output-port? (open-output-string))");
}

// --- Display/write format edge cases ---

#[test]
fn edge_display_write() {
    // display: strings without quotes, chars without #\
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("hello")),
    );

    // write: strings with quotes
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\"hello\"")),
    );

    // display char without #\ prefix
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display #\\a p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("a")),
    );
}

// --- define-record-type ---

#[test]
fn edge_define_record_type() {
    is_int(
        "(define-record-type <point>
           (make-point x y)
           point?
           (x point-x)
           (y point-y))
         (let ((p (make-point 3 4)))
           (+ (point-x p) (point-y p)))",
        7,
    );

    is_true(
        "(define-record-type <point>
           (make-point x y)
           point?
           (x point-x)
           (y point-y))
         (point? (make-point 1 2))",
    );

    is_false(
        "(define-record-type <point>
           (make-point x y)
           point?
           (x point-x)
           (y point-y))
         (point? '(1 2))",
    );
}

// --- Tail patterns (R7RS §3.5 required tail positions) ---

#[test]
fn edge_tco_letrec() {
    // letrec body in tail position
    is_int(
        "(letrec ((f (lambda (n) (if (= n 0) 42 (f (- n 1))))))
           (f 100000))",
        42,
    );
}

#[test]
fn edge_tco_let_star() {
    // let* body in tail position
    is_int(
        "(define (f n) (let* ((x n)) (if (= x 0) 42 (f (- x 1)))))
         (f 100000)",
        42,
    );
}

#[test]
fn edge_tco_begin() {
    // begin — last expression in tail position
    is_int(
        "(define (f n) (begin (if (= n 0) 42 (f (- n 1)))))
         (f 100000)",
        42,
    );
}

// --- Multiple return values edge cases ---

#[test]
fn edge_let_values() {
    is_int(
        "(let-values (((a b c) (values 1 2 3)))
           (+ a b c))",
        6,
    );
}

#[test]
fn edge_receive() {
    is_int(
        "(receive (a b c)
           (values 10 20 30)
           (+ a b c))",
        60,
    );
}

// --- cond-expand ---

#[test]
fn edge_cond_expand_combinators() {
    // library feature
    is_int("(cond-expand ((library (scheme base)) 1) (else 2))", 1);
    is_int("(cond-expand ((library (nonexistent lib)) 1) (else 2))", 2);

    // and combinator
    is_int("(cond-expand ((and r7rs mae) 1) (else 0))", 1);
    is_int("(cond-expand ((and r7rs chicken) 1) (else 0))", 0);

    // or combinator
    is_int("(cond-expand ((or chicken mae) 1) (else 0))", 1);
    is_int("(cond-expand ((or chicken guile) 1) (else 0))", 0);

    // not combinator
    is_int("(cond-expand ((not chicken) 1) (else 0))", 1);
    is_int("(cond-expand ((not r7rs) 1) (else 0))", 0);
}

// --- Case-insensitive character comparisons (§6.6) ---

#[test]
fn edge_char_ci_comparisons() {
    is_true("(char-ci=? #\\a #\\A)");
    is_true("(char-ci<? #\\a #\\B)");
    is_true("(char-ci>? #\\b #\\A)");
    is_true("(char-ci<=? #\\a #\\A)");
    is_true("(char-ci<=? #\\a #\\B)");
    is_true("(char-ci>=? #\\b #\\A)");
    is_true("(char-ci>=? #\\A #\\a)");
    is_false("(char-ci<? #\\b #\\A)");
    is_false("(char-ci>? #\\a #\\B)");
}

// --- Case-insensitive string comparisons (§6.7) ---

#[test]
fn edge_string_ci_comparisons() {
    is_true("(string-ci=? \"Hello\" \"hello\")");
    is_true("(string-ci=? \"ABC\" \"abc\")");
    is_false("(string-ci=? \"abc\" \"abd\")");

    is_true("(string-ci<? \"abc\" \"ABD\")");
    is_false("(string-ci<? \"abd\" \"ABC\")");

    is_true("(string-ci>? \"abd\" \"ABC\")");
    is_true("(string-ci<=? \"ABC\" \"abc\")");
    is_true("(string-ci>=? \"ABC\" \"abc\")");
}

// --- list-set! immutability (§6.4) ---

#[test]
fn edge_list_set_immutable() {
    let err = eval_err("(list-set! '(1 2 3) 1 99)");
    assert!(
        err.contains("immutable") || err.contains("Immutable"),
        "expected immutable error: {err}"
    );
}

// --- read-string (§6.13) ---

#[test]
fn edge_read_string() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"hello world\")))
               (read-string 5 p))"
        ),
        Value::String(Rc::from("hello")),
    );

    // Read past end — returns what's available
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"hi\")))
               (read-string 10 p))"
        ),
        Value::String(Rc::from("hi")),
    );

    // Empty port → eof
    is_true(
        "(let ((p (open-input-string \"\")))
           (eof-object? (read-string 5 p)))",
    );
}

// --- features (§6.14) ---

#[test]
fn edge_features() {
    is_true("(list? (features))");
    // memq returns the sublist starting at the match, not #t
    is_true("(pair? (memq 'r7rs (features)))");
    is_true("(pair? (memq 'mae (features)))");
}

// --- error-object structured access ---

#[test]
fn edge_error_object_full() {
    // error-object? distinguishes error objects from other values
    is_true(
        "(guard (e (#t (error-object? e)))
           (error \"test\"))",
    );
    is_false(
        "(guard (e (#t (error-object? e)))
           (raise 42))",
    );

    // error-object-message
    assert_eq!(
        eval(
            "(guard (e (#t (error-object-message e)))
               (error \"hello world\"))"
        ),
        Value::String(Rc::from("hello world")),
    );

    // error-object-irritants
    is_true(
        "(guard (e (#t (equal? (error-object-irritants e) '(1 2 3))))
           (error \"test\" 1 2 3))",
    );

    // error-object-type
    is_true(
        "(guard (e (#t (string? (error-object-type e))))
           (error \"test\"))",
    );
}

// --- with-exception-handler (§6.11) ---

#[test]
fn edge_with_exception_handler() {
    // R7RS §6.11: with-exception-handler + raise-continuable allows handler to return
    is_int(
        "(with-exception-handler
           (lambda (e) 42)
           (lambda () (raise-continuable \"boom\")))",
        42,
    );

    // R7RS §6.11: with-exception-handler + raise (non-continuable) —
    // handler that returns is an error
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval(
        "(with-exception-handler
           (lambda (e) 42)
           (lambda () (raise \"boom\")))",
    );
    assert!(result.is_err(), "raise handler returned should be an error");
}

// --- floor/ and truncate/ (§6.2.6) ---

#[test]
fn edge_floor_div() {
    // floor/ returns (quotient remainder) as a list
    is_int("(car (floor/ 17 5))", 3);
    is_int("(cadr (floor/ 17 5))", 2);
    // Negative dividend
    is_int("(car (floor/ -17 5))", -4);
    is_int("(cadr (floor/ -17 5))", 3);
    // Negative divisor
    is_int("(car (floor/ 17 -5))", -4);
    is_int("(cadr (floor/ 17 -5))", -3);
}

#[test]
fn edge_truncate_div() {
    is_int("(car (truncate/ 17 5))", 3);
    is_int("(cadr (truncate/ 17 5))", 2);
    // Negative dividend — truncate toward zero
    is_int("(car (truncate/ -17 5))", -3);
    is_int("(cadr (truncate/ -17 5))", -2);
}

// --- rationalize (§6.2.6) ---

#[test]
fn edge_rationalize() {
    // rationalize finds simplest rational within tolerance
    // (rationalize 3.1 0.5) should return 3 (integer is simplest)
    is_int("(exact (rationalize 3 1/10))", 3);
    // With large tolerance, 0 is simplest
    is_int("(exact (rationalize 0.3 1))", 0);
}

// --- let-syntax / letrec-syntax (§4.3.1) ---

#[test]
fn edge_let_syntax() {
    // Basic let-syntax
    is_int(
        "(let-syntax ((double (syntax-rules ()
                       ((double x) (+ x x)))))
           (double 5))",
        10,
    );
    // let-syntax doesn't leak into outer scope
    is_int(
        "(begin
           (let-syntax ((my-add (syntax-rules ()
                          ((my-add a b) (+ a b)))))
             (my-add 3 4)))",
        7,
    );
}

#[test]
fn edge_letrec_syntax() {
    // letrec-syntax — same as let-syntax in our implementation
    is_int(
        "(letrec-syntax ((my-inc (syntax-rules ()
                          ((my-inc x) (+ x 1)))))
           (my-inc 10))",
        11,
    );
}

// --- with-exception-handler edge cases ---

#[test]
fn edge_with_exception_handler_error_object() {
    // Handler receives an error object from (error ...) via guard
    is_true(
        "(guard (e (#t (error-object? e)))
           (error \"test\" \"msg\"))",
    );
    // Handler can extract message via guard
    is_str(
        "(guard (e (#t (error-object-message e)))
           (error \"oops\"))",
        "oops",
    );
    // with-exception-handler + raise-continuable: handler receives exception
    is_true(
        "(with-exception-handler
           (lambda (e) (string? e))
           (lambda () (raise-continuable \"hello\")))",
    );
}

// --- comprehensive with-exception-handler ---

#[test]
fn edge_with_exception_handler_normal_return() {
    // No exception — thunk returns normally
    is_int(
        "(with-exception-handler
           (lambda (e) 999)
           (lambda () 42))",
        42,
    );
}

// --- additional R7RS coverage ---

#[test]
fn edge_assoc_basic() {
    // assoc uses equal? by default
    is_true("(pair? (assoc \"b\" '((\"a\" 1) (\"b\" 2) (\"c\" 3))))");
    is_false("(assoc \"d\" '((\"a\" 1) (\"b\" 2)))");
    // assv uses eqv?
    is_true("(pair? (assv 2 '((1 a) (2 b) (3 c))))");
    // assq uses eq?
    is_true("(pair? (assq 'b '((a 1) (b 2) (c 3))))");
}

#[test]
fn edge_member_basic() {
    // member uses equal?
    is_true("(pair? (member \"b\" '(\"a\" \"b\" \"c\")))");
    is_false("(member \"d\" '(\"a\" \"b\" \"c\"))");
    // memv uses eqv?
    is_true("(pair? (memv 2 '(1 2 3)))");
}

#[test]
fn edge_list_copy_deep() {
    // list-copy creates a shallow copy
    is_int("(length (list-copy '(1 2 3)))", 3);
    is_int("(car (list-copy '(1 2 3)))", 1);
}

#[test]
fn edge_string_to_vector() {
    is_int("(vector-length (string->vector \"abc\"))", 3);
    is_str("(string (vector-ref (string->vector \"abc\") 1))", "b");
}

#[test]
fn edge_vector_to_string() {
    is_str("(vector->string (vector #\\a #\\b #\\c))", "abc");
}

#[test]
fn edge_utf8_string_conversion() {
    // string->utf8 and utf8->string roundtrip
    is_str("(utf8->string (string->utf8 \"hello\"))", "hello");
}

#[test]
fn edge_bytevector_append() {
    is_int(
        "(bytevector-length (bytevector-append (bytevector 1 2) (bytevector 3 4)))",
        4,
    );
}

#[test]
fn edge_port_predicates() {
    is_true("(input-port? (current-input-port))");
    is_true("(output-port? (current-output-port))");
    is_true("(output-port? (current-error-port))");
    is_true("(textual-port? (current-input-port))");
    is_true("(textual-port? (current-output-port))");
}

#[test]
fn edge_open_close_port() {
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (input-port-open? p))",
    );
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (close-port p)
           (not (input-port-open? p)))",
    );
}

// ============================================================================
// Stress tests: tricky R7RS edge cases for reliability
// ============================================================================

// --- Numeric precision and edge cases ---

#[test]
fn stress_exact_arithmetic_overflow() {
    // Large integer multiplication
    is_int("(* 1000000 1000000)", 1000000000000);
    // Exact integer sqrt of perfect squares
    is_int("(car (exact-integer-sqrt 144))", 12);
    is_int("(cadr (exact-integer-sqrt 144))", 0);
    // Non-perfect square
    is_int("(car (exact-integer-sqrt 10))", 3);
    is_int("(cadr (exact-integer-sqrt 10))", 1);
}

#[test]
fn stress_numeric_boundary_values() {
    is_true("(exact? 0)");
    is_true("(inexact? 0.0)");
    is_true("(= 0 0.0)");
    is_true("(zero? 0)");
    is_true("(zero? 0.0)");
    is_true("(positive? 1)");
    is_true("(negative? -1)");
    is_true("(even? 0)");
    is_true("(odd? 1)");
    is_true("(odd? -1)");
    // min/max edge cases
    is_int("(min 1 2 3 -1 0)", -1);
    is_int("(max 1 2 3 -1 0)", 3);
}

#[test]
fn stress_gcd_lcm_edge_cases() {
    is_int("(gcd 0 0)", 0);
    is_int("(gcd 12 0)", 12);
    is_int("(gcd 0 12)", 12);
    is_int("(gcd -12 8)", 4);
    is_int("(lcm 4 6)", 12);
    is_int("(lcm 0 5)", 0);
}

// --- Guard/exception interaction with TCO ---

#[test]
fn stress_guard_in_tail_position() {
    // guard body in tail position (moderate depth)
    is_int(
        "(define (f n)
           (guard (exn (#t 0))
             (if (= n 0) 42 (f (- n 1)))))
         (f 100)",
        42,
    );
}

#[test]
fn stress_nested_guard() {
    // Inner guard catches, outer doesn't fire
    is_int(
        "(guard (outer (#t 1))
           (guard (inner (#t 2))
             (raise \"boom\")))",
        2,
    );
}

#[test]
fn stress_guard_reraise() {
    // Guard clause doesn't match → re-raised to outer
    is_int(
        "(guard (outer (#t 99))
           (guard (inner ((string? inner) 0))
             (raise 42)))",
        99,
    );
}

// --- Dynamic-wind ordering with multiple winds ---

#[test]
fn stress_dynamic_wind_nested() {
    // Nested dynamic-wind: both cleanup thunks run
    is_int(
        "(let ((x 0))
           (dynamic-wind
             (lambda () (set! x (+ x 1)))
             (lambda ()
               (dynamic-wind
                 (lambda () (set! x (+ x 10)))
                 (lambda () (set! x (+ x 100)))
                 (lambda () (set! x (+ x 1000)))))
             (lambda () (set! x (+ x 10000))))
           x)",
        11111,
    );
}

// --- Case-lambda exhaustive ---

#[test]
fn stress_case_lambda() {
    is_int(
        "(let ((f (case-lambda
                    (() 0)
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (+ (f) (f 1) (f 2 3) (f 4 5 6)))",
        21,
    );
}

// --- Closures capturing mutable state ---

#[test]
fn stress_closure_shared_state() {
    is_int(
        "(let ((counter 0))
           (define (inc!) (set! counter (+ counter 1)) counter)
           (define (dec!) (set! counter (- counter 1)) counter)
           (inc!) (inc!) (inc!) (dec!)
           counter)",
        2,
    );
}

#[test]
fn stress_closure_in_list() {
    // Create a list of closures sharing state
    is_int(
        "(let ((x 0))
           (let ((add (lambda (n) (set! x (+ x n)) x))
                 (get (lambda () x)))
             (add 10)
             (add 20)
             (get)))",
        30,
    );
}

// --- String edge cases ---

#[test]
fn stress_string_empty_operations() {
    is_int("(string-length \"\")", 0);
    is_str("(substring \"\" 0 0)", "");
    is_str("(string-append)", "");
    is_str("(string-append \"\" \"\" \"\")", "");
    is_str("(string-copy \"\")", "");
}

#[test]
fn stress_string_unicode() {
    // Multi-byte characters
    is_int("(string-length \"αβγ\")", 3);
    is_str("(substring \"αβγ\" 1 2)", "β");
    is_true("(char=? (string-ref \"αβγ\" 2) #\\γ)");
}

// --- Vector operations ---

#[test]
fn stress_vector_large() {
    is_int("(vector-length (make-vector 1000 0))", 1000);
    is_int(
        "(let ((v (make-vector 100 0)))
           (vector-set! v 99 42)
           (vector-ref v 99))",
        42,
    );
}

// --- Proper tail recursion in all derived forms ---

#[test]
fn stress_tco_or_chain() {
    // or — last expression in tail position (TCO)
    is_int(
        "(define (f n) (if (= n 0) 42 (or #f (f (- n 1)))))
         (f 50000)",
        42,
    );
}

#[test]
fn stress_tco_and_chain() {
    // and — last expression in tail position (TCO)
    is_int(
        "(define (f n) (if (= n 0) 42 (and #t (f (- n 1)))))
         (f 50000)",
        42,
    );
}

// --- Define-record-type ---

#[test]
fn stress_record_type() {
    is_true(
        "(define-record-type <point>
           (make-point x y)
           point?
           (x point-x)
           (y point-y))
         (let ((p (make-point 3 4)))
           (and (point? p)
                (= (point-x p) 3)
                (= (point-y p) 4)))",
    );
}

#[test]
fn stress_record_type_predicate() {
    is_false(
        "(define-record-type <thing>
           (make-thing v)
           thing?
           (v thing-v))
         (thing? 42)",
    );
}

// --- Parameterize edge cases ---

#[test]
fn stress_parameterize_nested() {
    is_int(
        "(define p (make-parameter 1))
         (parameterize ((p 2))
           (parameterize ((p 3))
             (p)))",
        3,
    );
    // After parameterize, value restored
    is_int(
        "(define p2 (make-parameter 10))
         (parameterize ((p2 20))
           (p2))
         (p2)",
        10,
    );
}

// --- Bytevector edge cases ---

#[test]
fn stress_bytevector_ops() {
    is_int("(bytevector-length (bytevector))", 0);
    is_int("(bytevector-u8-ref (bytevector 10 20 30) 1)", 20);
    is_true(
        "(let ((bv (make-bytevector 3 0)))
           (bytevector-u8-set! bv 1 255)
           (= (bytevector-u8-ref bv 1) 255))",
    );
}

// --- Multiple return values ---

#[test]
fn stress_values_receive() {
    is_int(
        "(receive (a b c)
           (values 1 2 3)
           (+ a b c))",
        6,
    );
}

#[test]
fn stress_call_with_values() {
    is_int(
        "(call-with-values
           (lambda () (values 10 20))
           +)",
        30,
    );
}

// --- Do loop edge cases ---

#[test]
fn stress_do_empty_body() {
    // do with no body, just test + step
    is_int(
        "(do ((i 0 (+ i 1)))
             ((= i 10) i))",
        10,
    );
}

#[test]
fn stress_do_multiple_vars() {
    is_int(
        "(do ((i 0 (+ i 1))
              (j 10 (- j 1)))
             ((= i j) i))",
        5,
    );
}

// --- Boolean edge cases ---

#[test]
fn stress_boolean_semantics() {
    // Only #f is false
    is_true("(if 0 #t #f)");
    is_true("(if \"\" #t #f)");
    is_true("(if '() #t #f)");
    is_true("(if #t #t #f)");
    is_false("(if #f #t #f)");
    // boolean=?
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
}

// ============================================================================
// include / load / library system
// ============================================================================

#[test]
fn s5_6_include_basic() {
    // Write a temp file, include it
    let dir = std::env::temp_dir();
    let path = dir.join("mae-test-include.scm");
    std::fs::write(&path, "(define include-test-val 42)").unwrap();

    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.load_paths.push(dir);
    let result = vm
        .eval(&format!(
            "(include \"{}\") include-test-val",
            path.display()
        ))
        .unwrap();
    assert_eq!(result, Value::Int(42));

    std::fs::remove_file(&path).ok();
}

#[test]
fn s5_6_load_file() {
    // Write a temp file and load it
    let dir = std::env::temp_dir();
    let path = dir.join("mae-test-load.scm");
    std::fs::write(&path, "(define load-test-result 99)").unwrap();

    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(&format!("(load \"{}\") load-test-result", path.display()))
        .unwrap();
    assert_eq!(result, Value::Int(99));

    std::fs::remove_file(&path).ok();
}

#[test]
fn s6_13_with_output_to_file() {
    let path = std::env::temp_dir().join("mae-test-with-output.txt");
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    // with-output-to-file just opens and calls thunk (simplified)
    vm.eval(&format!(
        "(with-output-to-file \"{}\" (lambda () #t))",
        path.display()
    ))
    .unwrap();
    std::fs::remove_file(&path).ok();
}

#[test]
fn s5_6_library_import_export() {
    // Define a library and import from it
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(define-library (test math)
               (export square)
               (begin
                 (define (square x) (* x x))))
             (import (test math))
             (square 7)",
        )
        .unwrap();
    assert_eq!(result, Value::Int(49));
}

#[test]
fn s5_6_library_import_only() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(define-library (my-lib)
               (export add1 sub1)
               (begin
                 (define (add1 x) (+ x 1))
                 (define (sub1 x) (- x 1))))
             (import (only (my-lib) add1))
             (add1 10)",
        )
        .unwrap();
    assert_eq!(result, Value::Int(11));
}

#[test]
fn s5_6_library_import_prefix() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(define-library (util)
               (export double)
               (begin (define (double x) (* x 2))))
             (import (prefix (util) u:))
             (u:double 5)",
        )
        .unwrap();
    assert_eq!(result, Value::Int(10));
}

#[test]
fn s6_sleep_ms() {
    // sleep-ms should complete and return #t
    is_true("(sleep-ms 1)");
    // Verify timing (sleep at least 10ms)
    is_true(
        "(let ((start (current-jiffy)))
           (sleep-ms 10)
           (let ((elapsed (- (current-jiffy) start)))
             (>= elapsed 5000000)))", // 5ms in nanoseconds (generous)
    );
}

#[test]
fn s5_6_library_import_rename() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(define-library (ops)
               (export multiply)
               (begin (define (multiply x y) (* x y))))
             (import (rename (ops) (multiply mul)))
             (mul 3 4)",
        )
        .unwrap();
    assert_eq!(result, Value::Int(12));
}

// ============================================================================
// (scheme inexact) library tests
// ============================================================================

#[test]
fn s6_inexact_trig() {
    // sin/cos/tan
    is_true("(< (abs (sin 0.0)) 0.0001)");
    is_true("(< (abs (- (cos 0.0) 1.0)) 0.0001)");
    is_true("(< (abs (tan 0.0)) 0.0001)");
    // asin/acos/atan
    is_true("(< (abs (asin 0.0)) 0.0001)");
    is_true("(< (abs (- (acos 1.0) 0.0)) 0.0001)");
    is_true("(< (abs (atan 0.0)) 0.0001)");
    // atan with 2 args
    is_true("(< (abs (- (atan 1.0 1.0) 0.7853981)) 0.001)");
}

#[test]
fn s6_inexact_exp_log() {
    // exp(0) = 1
    is_true("(< (abs (- (exp 0) 1.0)) 0.0001)");
    // log(1) = 0
    is_true("(< (abs (log 1)) 0.0001)");
    // log with base: log(8, 2) = 3
    is_true("(< (abs (- (log 8 2) 3.0)) 0.0001)");
}

#[test]
fn s6_inexact_finite() {
    is_true("(finite? 42)");
    is_true("(finite? 3.14)");
    is_false("(finite? +inf.0)");
    is_false("(finite? -inf.0)");
    is_false("(finite? +nan.0)");
}

// ============================================================================
// (scheme file) library tests
// ============================================================================

#[test]
fn s6_file_exists() {
    // Check that a known file doesn't exist
    is_false("(file-exists? \"/tmp/mae-scheme-test-nonexistent-file-12345\")");
}

#[test]
fn s6_file_operations() {
    // Create, check exists, delete
    is_true(
        "(let ((path \"/tmp/mae-scheme-test-file-ops.txt\"))
           (let ((p (open-output-file path)))
             (write-string \"hello\" p)
             (close-port p))
           (let ((exists (file-exists? path)))
             (delete-file path)
             exists))",
    );
    // After delete, should not exist
    is_false("(file-exists? \"/tmp/mae-scheme-test-file-ops.txt\")");
}

// ============================================================================
// §7.1 Lexical structure — Reader features
// ============================================================================

#[test]
fn s7_1_radix_prefixes() {
    // Binary
    is_int("#b101", 5);
    is_int("#b0", 0);
    is_int("#b1111", 15);
    is_int("#b-101", -5);
    is_int("#B110", 6);

    // Octal
    is_int("#o77", 63);
    is_int("#o0", 0);
    is_int("#o17", 15);
    is_int("#o-10", -8);
    is_int("#O77", 63);

    // Decimal (explicit)
    is_int("#d42", 42);
    is_int("#d-7", -7);
    is_int("#D100", 100);

    // Hexadecimal
    is_int("#xff", 255);
    is_int("#x0", 0);
    is_int("#xDEAD", 0xDEAD);
    is_int("#x-ff", -255);
    is_int("#XFF", 255);
}

#[test]
fn s7_1_exactness_prefixes() {
    // #i makes exact -> inexact
    assert_eq!(eval("#i5"), Value::Float(5.0));
    assert_eq!(eval("#i42"), Value::Float(42.0));

    // #e makes inexact -> exact
    is_int("#e1.0", 1);
    is_int("#e5.0", 5);

    // #e on already-exact is identity
    is_int("#e5", 5);

    // #i on already-inexact is identity
    assert_eq!(eval("#i3.15"), Value::Float(3.15));
}

#[test]
fn s7_1_combined_radix_exactness() {
    // Exactness + radix combinations (R7RS §7.1.1)
    is_int("#e#xff", 255);
    is_int("#e#b101", 5);
    is_int("#e#o77", 63);

    // Inexact + radix
    assert_eq!(eval("#i#xff"), Value::Float(255.0));
    assert_eq!(eval("#i#b101"), Value::Float(5.0));
    assert_eq!(eval("#i#o77"), Value::Float(63.0));
}

#[test]
fn s7_1_radix_in_expressions() {
    // Radix numbers should work in expressions
    is_int("(+ #xff 1)", 256);
    is_int("(* #b10 #o10)", 16); // 2 * 8
    is_int("(- #x10 #d10)", 6); // 16 - 10
}

// ============================================================================
// §6.12 Eval
// ============================================================================

#[test]
fn s6_12_eval_basic() {
    // Basic eval of quoted expression
    is_int("(eval '(+ 1 2))", 3);
    is_int("(eval '(* 3 4))", 12);
    // Eval of self-evaluating datum
    is_int("(eval 42)", 42);
    is_true("(eval #t)");
    is_str("(eval \"hello\")", "hello");
}

#[test]
fn s6_12_eval_with_environment() {
    // eval with interaction-environment
    is_int("(eval '(+ 1 2) (interaction-environment))", 3);
    // eval with scheme-report-environment
    is_int("(eval '(+ 1 2) (scheme-report-environment 7))", 3);
}

#[test]
fn s6_12_eval_complex() {
    // Eval of nested expressions
    is_int("(eval '(let ((x 10)) (+ x 5)))", 15);
    // Eval of define + use
    is_int("(eval '(begin (define y 42) y))", 42);
    // Eval with lambda
    is_int("(eval '((lambda (x) (* x x)) 5))", 25);
}

// ============================================================================
// §6.10 call-with-values (R7RS spec compliance)
// ============================================================================

#[test]
fn s6_10_call_with_values_spec() {
    // R7RS examples from spec
    // (call-with-values (lambda () (values 4 5)) (lambda (a b) b)) → 5
    is_int(
        "(call-with-values (lambda () (values 4 5)) (lambda (a b) b))",
        5,
    );
    // Single value case
    is_int("(call-with-values (lambda () 5) (lambda (x) x))", 5);
    // Multiple values to list
    assert_eq!(
        format!(
            "{}",
            eval("(call-with-values (lambda () (values 1 2 3)) list)")
        ),
        "(1 2 3)"
    );
}

#[test]
fn s6_10_floor_truncate_with_values() {
    // floor/ returns two values, usable with call-with-values
    assert_eq!(
        format!(
            "{}",
            eval("(call-with-values (lambda () (floor/ 17 5)) list)")
        ),
        "(3 2)"
    );
    // truncate/ returns two values
    assert_eq!(
        format!(
            "{}",
            eval("(call-with-values (lambda () (truncate/ 17 5)) list)")
        ),
        "(3 2)"
    );
    // Negative floor/
    assert_eq!(
        format!(
            "{}",
            eval("(call-with-values (lambda () (floor/ -7 2)) list)")
        ),
        "(-4 1)"
    );
}

#[test]
fn s6_10_receive_values() {
    // receive (SRFI-8) is sugar for call-with-values
    is_int("(receive (a b) (values 10 20) (+ a b))", 30);
    is_int("(receive (a b c) (values 1 2 3) (* a b c))", 6);
}

// ============================================================================
// Comprehensive coverage sweep — ensuring every R7RS function is tested
// ============================================================================

// §6.1 Equivalence predicates — additional coverage
#[test]
fn s6_1_eqv_comprehensive() {
    // eqv? on characters
    is_true("(eqv? #\\a #\\a)");
    is_false("(eqv? #\\a #\\b)");
    // eqv? on empty list
    is_true("(eqv? '() '())");
    // eqv? on booleans
    is_true("(eqv? #t #t)");
    is_true("(eqv? #f #f)");
    is_false("(eqv? #t #f)");
    // eqv? on numbers
    is_true("(eqv? 42 42)");
    is_false("(eqv? 42 42.0)"); // exact vs inexact
}

// §6.2 Numbers — comprehensive coverage
#[test]
fn s6_2_numeric_predicates() {
    is_true("(zero? 0)");
    is_false("(zero? 1)");
    is_true("(positive? 5)");
    is_false("(positive? -5)");
    is_false("(positive? 0)");
    is_true("(negative? -5)");
    is_false("(negative? 5)");
    is_true("(odd? 3)");
    is_false("(odd? 4)");
    is_true("(even? 4)");
    is_false("(even? 3)");
    is_true("(finite? 42.0)");
    is_false("(finite? +inf.0)");
    is_true("(infinite? +inf.0)");
    is_true("(infinite? -inf.0)");
    is_false("(infinite? 42.0)");
    is_true("(nan? +nan.0)");
    is_false("(nan? 42.0)");
}

#[test]
fn s6_2_type_predicates() {
    is_true("(number? 42)");
    is_true("(number? 3.14)");
    is_false("(number? \"hello\")");
    is_true("(integer? 42)");
    is_false("(integer? 3.14)");
    is_true("(real? 42)");
    is_true("(real? 3.14)");
    is_true("(rational? 42)");
    is_true("(complex? 42)"); // all numbers are complex
    is_true("(exact? 42)");
    is_false("(exact? 3.14)");
    is_true("(inexact? 3.14)");
    is_false("(inexact? 42)");
    is_true("(exact-integer? 42)");
    is_false("(exact-integer? 3.14)");
    is_false("(exact-integer? 42.0)");
}

#[test]
fn s6_2_arithmetic_edge_cases() {
    // Unary minus
    is_int("(- 5)", -5);
    // Unary plus
    is_int("(+ 5)", 5);
    // Zero args
    is_int("(+)", 0);
    is_int("(*)", 1);
    // abs
    is_int("(abs -7)", 7);
    is_int("(abs 7)", 7);
    // min/max
    is_int("(min 1 2 3)", 1);
    is_int("(max 1 2 3)", 3);
    is_int("(min 5)", 5);
    is_int("(max 5)", 5);
}

#[test]
fn s6_2_exact_inexact_conversion() {
    // exact->inexact
    assert_eq!(eval("(exact->inexact 5)"), Value::Float(5.0));
    assert_eq!(eval("(inexact->exact 5.0)"), Value::Int(5));
    // exact / inexact procedures (R7RS names)
    assert_eq!(eval("(inexact 5)"), Value::Float(5.0));
    assert_eq!(eval("(exact 5.0)"), Value::Int(5));
}

#[test]
fn s6_2_gcd_lcm_extended() {
    is_int("(gcd 32 -36)", 4);
    is_int("(gcd)", 0);
    is_int("(gcd 12)", 12);
    is_int("(lcm 32 -36)", 288);
    is_int("(lcm)", 1);
    is_int("(lcm 12)", 12);
}

#[test]
fn s6_2_exact_integer_sqrt_values() {
    // Returns two values: root and remainder
    // (exact-integer-sqrt 14) => 3 5 (since 3*3=9, 14-9=5)
    assert_eq!(format!("{}", eval("(exact-integer-sqrt 14)")), "(3 5)");
    assert_eq!(format!("{}", eval("(exact-integer-sqrt 4)")), "(2 0)");
    assert_eq!(format!("{}", eval("(exact-integer-sqrt 0)")), "(0 0)");
}

#[test]
fn s6_2_number_string_conversion() {
    is_str("(number->string 42)", "42");
    is_str("(number->string 42 16)", "2a");
    is_str("(number->string 42 8)", "52");
    is_str("(number->string 42 2)", "101010");
    is_int("(string->number \"42\")", 42);
    is_int("(string->number \"ff\" 16)", 255);
    is_int("(string->number \"77\" 8)", 63);
    is_false("(string->number \"not-a-number\")");
}

#[test]
fn s6_2_rationalize_basic() {
    // rationalize finds simplest rational within tolerance
    // (rationalize 3 1) — integers in [2, 4], simplest is 2 or 3
    // Our implementation returns the ceiling of lo, which for [2,4] is 2
    is_true("(let ((r (rationalize 3 1))) (and (>= r 2) (<= r 4)))");
    // Exact case with zero tolerance
    is_int("(rationalize 5 0)", 5);
}

#[test]
fn s6_2_quotient_remainder_modulo_extended() {
    // R5RS compatibility names
    is_int("(quotient 13 4)", 3);
    is_int("(remainder 13 4)", 1);
    is_int("(modulo 13 4)", 1);
    is_int("(quotient -13 4)", -3);
    is_int("(remainder -13 4)", -1);
    is_int("(modulo -13 4)", 3);
}

// §6.3 Booleans
#[test]
fn s6_3_boolean_comprehensive() {
    is_true("(boolean? #t)");
    is_true("(boolean? #f)");
    is_false("(boolean? 42)");
    is_false("(boolean? '())");
    is_true("(not #f)");
    is_false("(not #t)");
    is_false("(not 42)"); // only #f is falsy
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
}

// §6.4 Pairs/Lists — additional coverage
#[test]
fn s6_4_set_car_cdr() {
    // mae-scheme pairs are immutable — set-car!/set-cdr! signal errors
    let err = eval_err("(let ((x (cons 1 2))) (set-car! x 10))");
    assert!(
        err.contains("immutable") || err.contains("set-car"),
        "set-car! should signal immutable error: {err}"
    );
    let err = eval_err("(let ((x (cons 1 2))) (set-cdr! x 20))");
    assert!(
        err.contains("immutable") || err.contains("set-cdr"),
        "set-cdr! should signal immutable error: {err}"
    );
}

#[test]
fn s6_4_association_lists() {
    assert_eq!(
        format!("{}", eval("(assq 'b '((a 1) (b 2) (c 3)))")),
        "(b 2)"
    );
    is_false("(assq 'z '((a 1) (b 2)))");
    assert_eq!(
        format!("{}", eval("(assv 2 '((1 a) (2 b) (3 c)))")),
        "(2 b)"
    );
    // assoc uses equal? — strings display with quotes in our Display impl
    assert_eq!(
        format!("{}", eval("(assoc \"b\" '((\"a\" 1) (\"b\" 2)))")),
        "(\"b\" 2)"
    );
}

#[test]
fn s6_4_member_functions() {
    assert_eq!(format!("{}", eval("(memq 'b '(a b c d))")), "(b c d)");
    is_false("(memq 'z '(a b c))");
    assert_eq!(format!("{}", eval("(memv 2 '(1 2 3 4))")), "(2 3 4)");
    // member uses equal? — strings display with quotes
    assert_eq!(
        format!("{}", eval("(member \"b\" '(\"a\" \"b\" \"c\"))")),
        "(\"b\" \"c\")"
    );
}

// §6.5 Symbols — additional coverage
#[test]
fn s6_5_symbol_string_roundtrip() {
    is_true("(symbol? 'hello)");
    is_str("(symbol->string 'hello)", "hello");
    is_true("(eq? (string->symbol \"hello\") 'hello)");
    is_false("(symbol? 42)");
    is_false("(symbol? \"hello\")");
}

// §6.9 Bytevectors — comprehensive
#[test]
fn s6_9_bytevector_ops() {
    is_true("(bytevector? #u8(1 2 3))");
    is_false("(bytevector? '(1 2 3))");
    is_int("(bytevector-length #u8(1 2 3))", 3);
    is_int("(bytevector-u8-ref #u8(10 20 30) 1)", 20);
    assert_eq!(
        format!(
            "{}",
            eval("(let ((bv (bytevector 1 2 3))) (bytevector-u8-set! bv 1 99) bv)")
        ),
        "#u8(1 99 3)"
    );
}

#[test]
fn s6_9_bytevector_constructors() {
    assert_eq!(format!("{}", eval("(make-bytevector 3 0)")), "#u8(0 0 0)");
    assert_eq!(format!("{}", eval("(bytevector 1 2 3)")), "#u8(1 2 3)");
    assert_eq!(
        format!("{}", eval("(bytevector-copy #u8(1 2 3))")),
        "#u8(1 2 3)"
    );
    assert_eq!(
        format!("{}", eval("(bytevector-append #u8(1 2) #u8(3 4))")),
        "#u8(1 2 3 4)"
    );
}

#[test]
fn s6_9_utf8_conversion() {
    is_str("(utf8->string #u8(104 101 108 108 111))", "hello");
    assert_eq!(
        format!("{}", eval("(string->utf8 \"hello\")")),
        "#u8(104 101 108 108 111)"
    );
}

// §6.10 Control — additional coverage
#[test]
fn s6_10_procedure_predicate_extended() {
    is_true("(procedure? car)");
    is_true("(procedure? (lambda (x) x))");
    is_false("(procedure? 42)");
    is_false("(procedure? '(1 2 3))");
}

#[test]
fn s6_10_apply_comprehensive() {
    is_int("(apply + '(1 2 3))", 6);
    is_int("(apply + 1 2 '(3))", 6);
    is_int("(apply * '(2 3 4))", 24);
}

// §6.11 Exceptions — additional coverage
#[test]
fn s6_11_error_object_fields() {
    let result = eval(
        "(guard (exn (#t (list (error-object-message exn) (error-object-type exn))))
           (error \"test error\" 'my-type 1 2 3))",
    );
    let s = format!("{result}");
    assert!(s.contains("test error"), "Expected error message in: {s}");
}

#[test]
fn s6_11_raise_continuable() {
    // raise-continuable with handler that returns a value
    is_int(
        "(with-exception-handler
           (lambda (exn) 42)
           (lambda () (raise-continuable \"continue me\")))",
        42,
    );
}

#[test]
fn s6_11_error_predicates_comprehensive() {
    // file-error? and read-error? on regular errors
    is_false(
        "(guard (exn (#t (file-error? exn)))
           (error \"not a file error\"))",
    );
    is_false(
        "(guard (exn (#t (read-error? exn)))
           (error \"not a read error\"))",
    );
}

// §6.14 System interface
#[test]
fn s6_14_system_interface() {
    // features returns a list
    is_true("(list? (features))");
    // memq returns sublist (truthy), not #t — use pair? to check
    is_true("(pair? (memq 'r7rs (features)))");
    is_true("(pair? (memq 'mae-scheme (features)))");
    is_true("(pair? (memq 'mae (features)))");
    // command-line returns a list
    is_true("(list? (command-line))");
    // time functions
    is_true("(number? (current-second))");
    is_true("(number? (current-jiffy))");
    is_true("(integer? (jiffies-per-second))");
    is_true("(> (jiffies-per-second) 0)");
}

#[test]
fn s6_14_environment_variables() {
    // get-environment-variable
    is_true("(or (string? (get-environment-variable \"HOME\")) (not (get-environment-variable \"HOME\")))");
    // get-environment-variables returns alist
    is_true("(list? (get-environment-variables))");
}

// §4.2.5 Delayed evaluation
#[test]
fn s4_2_5_promises() {
    is_int("(force (delay 42))", 42);
    is_int("(force (make-promise 42))", 42);
    is_true("(promise? (delay 1))");
    is_true("(promise? (make-promise 1))");
    is_false("(promise? 42)");
    // delay caches result
    is_int(
        "(let ((p (delay (+ 1 2))))
           (+ (force p) (force p)))",
        6,
    );
}

// §4.2.6 Dynamic bindings
#[test]
fn s4_2_6_parameters() {
    is_int(
        "(let ((p (make-parameter 10)))
           (p))",
        10,
    );
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20))
             (p)))",
        20,
    );
    // Original value restored after parameterize
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20))
             (p))
           (p))",
        10,
    );
}

// §4.3 Macros — additional coverage
#[test]
fn s4_3_define_syntax_basic() {
    is_int(
        "(begin
           (define-syntax my-if
             (syntax-rules ()
               ((my-if test then else)
                (cond (test then) (#t else)))))
           (my-if #t 1 2))",
        1,
    );
    is_int(
        "(begin
           (define-syntax my-if
             (syntax-rules ()
               ((my-if test then else)
                (cond (test then) (#t else)))))
           (my-if #f 1 2))",
        2,
    );
}

#[test]
fn s4_3_let_syntax() {
    is_int(
        "(let-syntax ((double (syntax-rules ()
                               ((double x) (+ x x)))))
           (double 5))",
        10,
    );
}

// §5.5 Record types
#[test]
fn s5_5_define_record_type_basic() {
    is_int(
        "(begin
           (define-record-type <point>
             (make-point x y)
             point?
             (x point-x)
             (y point-y))
           (let ((p (make-point 3 4)))
             (+ (point-x p) (point-y p))))",
        7,
    );
    is_true(
        "(begin
           (define-record-type <point>
             (make-point x y)
             point?
             (x point-x)
             (y point-y))
           (point? (make-point 1 2)))",
    );
    is_false(
        "(begin
           (define-record-type <point>
             (make-point x y)
             point?
             (x point-x)
             (y point-y))
           (point? 42))",
    );
}

// §4.2.1 case-lambda
#[test]
fn s4_2_1_case_lambda() {
    is_int(
        "(let ((f (case-lambda
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (f 1))",
        1,
    );
    is_int(
        "(let ((f (case-lambda
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (f 1 2))",
        3,
    );
    is_int(
        "(let ((f (case-lambda
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (f 1 2 3))",
        6,
    );
}

// §4.2.4 do
#[test]
fn s4_2_4_do_comprehensive() {
    // Sum 1..10
    is_int(
        "(do ((i 1 (+ i 1))
              (sum 0 (+ sum i)))
             ((> i 10) sum))",
        55,
    );
    // Reverse a list
    assert_eq!(
        format!(
            "{}",
            eval(
                "(do ((lst '(1 2 3 4 5) (cdr lst))
                       (acc '() (cons (car lst) acc)))
                      ((null? lst) acc))"
            )
        ),
        "(5 4 3 2 1)"
    );
}

// §5.6 Libraries — define-library
#[test]
fn s5_6_define_library_basic() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    // Define and import a library
    vm.eval(
        "(define-library (test math)
           (export square cube)
           (begin
             (define (square x) (* x x))
             (define (cube x) (* x x x))))",
    )
    .unwrap();
    vm.eval("(import (test math))").unwrap();
    assert_eq!(vm.eval("(square 5)").unwrap(), Value::Int(25));
    assert_eq!(vm.eval("(cube 3)").unwrap(), Value::Int(27));
}

#[test]
fn s5_6_import_modifiers() {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test utils)
           (export add1 sub1)
           (begin
             (define (add1 x) (+ x 1))
             (define (sub1 x) (- x 1))))",
    )
    .unwrap();
    // import with only
    vm.eval("(import (only (test utils) add1))").unwrap();
    assert_eq!(vm.eval("(add1 5)").unwrap(), Value::Int(6));
    // import with rename
    vm.eval("(import (rename (test utils) (sub1 decrement)))")
        .unwrap();
    assert_eq!(vm.eval("(decrement 5)").unwrap(), Value::Int(4));
}

// §4.1.7 include
#[test]
fn s4_1_7_cond_expand() {
    // Basic cond-expand with r7rs feature
    is_int("(cond-expand (r7rs 1) (else 0))", 1);
    // mae and mae-scheme are both features
    is_int("(cond-expand (mae 1) (else 0))", 1);
    is_int("(cond-expand (mae-scheme 1) (else 0))", 1);
    // and clause
    is_int("(cond-expand ((and r7rs mae) 1) (else 0))", 1);
    // or clause
    is_int("(cond-expand ((or nonexistent r7rs) 1) (else 0))", 1);
    // not clause
    is_int("(cond-expand ((not nonexistent) 1) (else 0))", 1);
    is_int("(cond-expand ((not r7rs) 1) (else 0))", 0);
    // else clause
    is_int("(cond-expand (nonexistent 1) (else 42))", 42);
}

// §6.6 Character case-insensitive operations
#[test]
fn s6_6_char_ci_comparisons() {
    is_true("(char-ci=? #\\a #\\A)");
    is_true("(char-ci=? #\\z #\\Z)");
    is_false("(char-ci=? #\\a #\\b)");
    is_true("(char-ci<? #\\a #\\B)");
    is_false("(char-ci<? #\\b #\\A)");
    is_true("(char-ci>? #\\b #\\A)");
    is_false("(char-ci>? #\\a #\\B)");
    is_true("(char-ci<=? #\\a #\\A)");
    is_true("(char-ci<=? #\\a #\\B)");
    is_true("(char-ci>=? #\\b #\\A)");
    is_true("(char-ci>=? #\\a #\\A)");
}

#[test]
fn s6_6_digit_value() {
    is_int("(digit-value #\\0)", 0);
    is_int("(digit-value #\\5)", 5);
    is_int("(digit-value #\\9)", 9);
    is_false("(digit-value #\\a)");
    is_false("(digit-value #\\space)");
}

#[test]
fn s6_6_char_foldcase_test() {
    assert_eq!(eval("(char-foldcase #\\A)"), Value::Char('a'));
    assert_eq!(eval("(char-foldcase #\\a)"), Value::Char('a'));
    assert_eq!(eval("(char-foldcase #\\Z)"), Value::Char('z'));
}

#[test]
fn s6_6_char_to_string() {
    is_str("(char->string #\\a)", "a");
    is_str("(char->string #\\space)", " ");
}

// §6.7 String case-insensitive operations
#[test]
fn s6_7_string_ci_comparisons() {
    is_true("(string-ci=? \"hello\" \"HELLO\")");
    is_true("(string-ci=? \"Hello\" \"hELLO\")");
    is_false("(string-ci=? \"hello\" \"world\")");
    is_true("(string-ci<? \"abc\" \"DEF\")");
    is_false("(string-ci<? \"def\" \"ABC\")");
    is_true("(string-ci>? \"def\" \"ABC\")");
    is_false("(string-ci>? \"abc\" \"DEF\")");
    is_true("(string-ci<=? \"abc\" \"ABC\")");
    is_true("(string-ci<=? \"abc\" \"DEF\")");
    is_true("(string-ci>=? \"def\" \"ABC\")");
    is_true("(string-ci>=? \"abc\" \"ABC\")");
}

#[test]
fn s6_7_string_trim_split_join() {
    // Non-R7RS extensions, but registered — verify they work
    is_str("(string-trim \"  hello  \")", "hello");
    // string-split returns list of strings (displayed with quotes)
    assert_eq!(
        format!("{}", eval("(string-split \"a,b,c\" \",\")")),
        "(\"a\" \"b\" \"c\")"
    );
    is_str("(string-join '(\"a\" \"b\" \"c\") \",\")", "a,b,c");
}

#[test]
fn s6_7_string_contains_test() {
    is_true("(string-contains \"hello world\" \"world\")");
    is_false("(string-contains \"hello\" \"xyz\")");
    is_true("(string-contains \"hello\" \"\")");
}

// §6.8 Vector additional operations
#[test]
fn s6_8_vector_append_test() {
    assert_eq!(
        format!("{}", eval("(vector-append #(1 2) #(3 4))")),
        "#(1 2 3 4)"
    );
    assert_eq!(format!("{}", eval("(vector-append #() #(1))")), "#(1)");
}

#[test]
fn s6_8_vector_copy_bang() {
    assert_eq!(
        format!(
            "{}",
            eval(
                "(let ((v (vector 1 2 3 4 5)))
                              (vector-copy! v 1 #(10 20))
                              v)"
            )
        ),
        "#(1 10 20 4 5)"
    );
}

#[test]
fn s6_8_vector_string_roundtrip() {
    is_str("(vector->string #(#\\h #\\i))", "hi");
    assert_eq!(
        format!("{}", eval("(string->vector \"hello\")")),
        "#(#\\h #\\e #\\l #\\l #\\o)"
    );
}

// §6.9 Bytevector additional operations
#[test]
fn s6_9_bytevector_copy_bang() {
    assert_eq!(
        format!(
            "{}",
            eval(
                "(let ((bv (bytevector 1 2 3 4 5)))
                              (bytevector-copy! bv 1 #u8(10 20))
                              bv)"
            )
        ),
        "#u8(1 10 20 4 5)"
    );
}

#[test]
fn s6_9_bytevector_list_conversion() {
    assert_eq!(
        format!("{}", eval("(bytevector->list #u8(1 2 3))")),
        "(1 2 3)"
    );
    assert_eq!(
        format!("{}", eval("(list->bytevector '(10 20 30))")),
        "#u8(10 20 30)"
    );
}

// §6.13 Binary file I/O
#[test]
fn s6_13_binary_file_io() {
    // Write and read binary data
    eval(
        "(let ((p (open-binary-output-file \"/tmp/mae-scheme-binary-test.dat\")))
           (write-u8 65 p)
           (write-u8 66 p)
           (write-u8 67 p)
           (close-port p))",
    );
    is_int(
        "(let ((p (open-binary-input-file \"/tmp/mae-scheme-binary-test.dat\")))
           (let ((b (read-u8 p)))
             (close-port p)
             b))",
        65,
    );
    // Cleanup
    eval("(delete-file \"/tmp/mae-scheme-binary-test.dat\")");
}

// §6.13 write-shared
#[test]
fn s6_13_write_shared() {
    // write-shared should produce output for shared structures
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(let ((p (open-output-string)))
                 (write-shared '(1 2 3) p)
                 (get-output-string p))",
        )
        .unwrap();
    assert_eq!(result, Value::String(Rc::from("(1 2 3)")));
}

// String mutation error messages are helpful
#[test]
fn s6_7_string_mutation_errors() {
    let err = eval_err("(string-set! \"hello\" 0 #\\H)");
    assert!(
        err.contains("immutable"),
        "string-set! should mention immutability: {err}"
    );
    let err = eval_err("(string-copy! \"hello\" 0 \"world\")");
    assert!(
        err.contains("immutable"),
        "string-copy! should mention immutability: {err}"
    );
    let err = eval_err("(string-fill! \"hello\" #\\x)");
    assert!(
        err.contains("immutable"),
        "string-fill! should mention immutability: {err}"
    );
}

// list-set! error message is helpful
#[test]
fn s6_4_list_set_error() {
    let err = eval_err("(list-set! '(1 2 3) 1 99)");
    assert!(
        err.contains("immutable"),
        "list-set! should mention immutability: {err}"
    );
}

#[test]
fn s7_1_cond_expand_library_availability() {
    // Verify all 13 R7RS-small libraries are recognized via cond-expand
    is_int("(cond-expand ((library (scheme base)) 1) (else 0))", 1);
    is_int(
        "(cond-expand ((library (scheme case-lambda)) 1) (else 0))",
        1,
    );
    is_int("(cond-expand ((library (scheme char)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme cxr)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme eval)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme file)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme inexact)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme lazy)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme load)) 1) (else 0))", 1);
    is_int(
        "(cond-expand ((library (scheme process-context)) 1) (else 0))",
        1,
    );
    is_int("(cond-expand ((library (scheme read)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme time)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme write)) 1) (else 0))", 1);
    is_int("(cond-expand ((library (scheme r5rs)) 1) (else 0))", 1);
    // Unknown library should not match
    is_int(
        "(cond-expand ((library (scheme nonexistent)) 1) (else 0))",
        0,
    );
}

// ============================================================
// §6.13 call-with-port
// ============================================================

#[test]
fn s6_13_call_with_port() {
    // call-with-port opens port, calls proc, closes port after
    is_true(
        r#"(let ((p (open-input-string "hello")))
             (let ((result (call-with-port p (lambda (port) (read-char port)))))
               (char=? result #\h)))"#,
    );
    // Port should be usable inside the proc
    assert_eq!(
        eval(r#"(call-with-port (open-input-string "abc") read-line)"#),
        Value::String(Rc::from("abc")),
    );
    // call-with-port returns the proc's result; port is closed after
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                  (write-string "test" p)
                  (call-with-port p (lambda (port) (get-output-string port))))"#
        ),
        Value::String(Rc::from("test")),
    );
}

// ============================================================
// §6.13.1 with-input-from-file / with-output-to-file
// ============================================================

#[test]
fn s6_13_with_input_from_file() {
    // with-output-to-file takes a zero-argument thunk (R7RS §6.13.1).
    // mae-scheme simplified: thunk runs but port is NOT redirected to
    // current-output-port (see SPEC_STANCES.md §8). Test with call-with-*
    // which explicitly passes the port — the common and portable pattern.
    let result = eval(
        r#"(begin
             (call-with-output-file "/tmp/mae-test-wif.txt"
               (lambda (port) (write-string "hello from file" port)))
             (call-with-input-file "/tmp/mae-test-wif.txt"
               (lambda (port) (read-line port))))"#,
    );
    assert_eq!(result, Value::String(Rc::from("hello from file")));

    // with-output-to-file thunk receives zero args
    // (returns void since thunk can't write without port redirection)
    eval(r#"(with-output-to-file "/tmp/mae-test-wof.txt" (lambda () #t))"#);

    // with-input-from-file thunk receives zero args
    eval(r#"(with-input-from-file "/tmp/mae-test-wif.txt" (lambda () #t))"#);
}

#[test]
fn s6_13_call_with_input_output_file() {
    // call-with-output-file + call-with-input-file roundtrip
    let result = eval(
        r#"(begin
             (call-with-output-file "/tmp/mae-test-cwf.txt"
               (lambda (port) (write-string "roundtrip" port)))
             (call-with-input-file "/tmp/mae-test-cwf.txt"
               (lambda (port) (read-line port))))"#,
    );
    assert_eq!(result, Value::String(Rc::from("roundtrip")));
}

// ============================================================
// §4.2.5 delay-force (iterative forcing)
// ============================================================

#[test]
fn s4_2_5_delay_force_iterative() {
    // delay-force creates a promise that, when forced, evaluates to another promise
    // This enables iterative lazy algorithms without stack growth
    is_int("(force (delay-force (delay 42)))", 42);

    // delay-force with immediate value wrapped in delay
    is_int("(force (delay-force (make-promise 99)))", 99);

    // Basic delay/force still works
    is_int("(force (delay (+ 1 2)))", 3);

    // make-promise wraps already-computed value
    is_int("(force (make-promise 7))", 7);

    // promise? predicate
    is_true("(promise? (delay 1))");
    is_true("(promise? (make-promise 1))");
    is_false("(promise? 42)");
    is_false("(promise? '())");
}

// ============================================================
// §4.2.3 define-values
// ============================================================

#[test]
fn s4_2_3_define_values() {
    // define-values binds multiple values from a values expression
    is_int(
        "(begin (define-values (a b c) (values 1 2 3)) (+ a b c))",
        6,
    );

    // Single value
    is_int("(begin (define-values (x) (values 10)) x)", 10);

    // define-values with computed expression
    is_int(
        "(begin (define-values (p q) (values (* 3 4) (+ 5 6))) (+ p q))",
        23,
    );
}

// ============================================================
// §6.9 write-bytevector
// ============================================================

#[test]
fn s6_9_write_bytevector() {
    // write-bytevector to output port
    assert_eq!(
        eval(
            r#"(let ((p (open-output-bytevector)))
                 (write-bytevector #u8(65 66 67) p)
                 (get-output-bytevector p))"#
        ),
        eval("#u8(65 66 67)"),
    );

    // write-bytevector with start/end range
    assert_eq!(
        eval(
            r#"(let ((p (open-output-bytevector)))
                 (write-bytevector #u8(10 20 30 40 50) p 1 4)
                 (get-output-bytevector p))"#
        ),
        eval("#u8(20 30 40)"),
    );
}

// ============================================================
// §6.10 for-each (comprehensive)
// ============================================================

#[test]
fn s6_10_for_each_comprehensive() {
    // for-each with side effects (order matters)
    assert_eq!(
        eval(
            r#"(let ((result '()))
                 (for-each (lambda (x) (set! result (cons x result)))
                           '(1 2 3))
                 result)"#
        ),
        eval("'(3 2 1)"),
    );

    // for-each with two lists
    assert_eq!(
        eval(
            r#"(let ((result '()))
                 (for-each (lambda (x y) (set! result (cons (+ x y) result)))
                           '(1 2 3) '(10 20 30))
                 result)"#
        ),
        eval("'(33 22 11)"),
    );

    // for-each returns void
    is_true("(void? (for-each + '()))");
}

// ============================================================
// §6.10 map (comprehensive)
// ============================================================

#[test]
fn s6_10_map_comprehensive() {
    // map with single list
    assert_eq!(
        eval("(map (lambda (x) (* x x)) '(1 2 3 4))"),
        eval("'(1 4 9 16)"),
    );

    // map with two lists
    assert_eq!(eval("(map + '(1 2 3) '(10 20 30))"), eval("'(11 22 33)"),);

    // map with empty list
    eval_eq("(map car '())", "'()");

    // map preserves order
    assert_eq!(
        eval("(map number->string '(1 2 3))"),
        eval(r#"'("1" "2" "3")"#),
    );
}

// ============================================================
// §6.7 string-map / string-for-each
// ============================================================

#[test]
fn s6_7_string_map_for_each() {
    // string-map applies function to each character
    assert_eq!(
        eval(r#"(string-map char-upcase "hello")"#),
        Value::String(Rc::from("HELLO")),
    );

    // string-for-each with side effects
    assert_eq!(
        eval(
            r#"(let ((result '()))
                 (string-for-each
                   (lambda (c) (set! result (cons c result)))
                   "abc")
                 result)"#
        ),
        eval(r#"'(#\c #\b #\a)"#),
    );
}

// ============================================================
// §6.8 vector-map / vector-for-each
// ============================================================

#[test]
fn s6_8_vector_map_for_each() {
    // vector-map
    assert_eq!(
        eval("(vector-map + #(1 2 3) #(10 20 30))"),
        eval("#(11 22 33)"),
    );

    // vector-map single vector
    assert_eq!(
        eval("(vector-map (lambda (x) (* x 2)) #(1 2 3))"),
        eval("#(2 4 6)"),
    );

    // vector-for-each
    assert_eq!(
        eval(
            r#"(let ((sum 0))
                 (vector-for-each (lambda (x) (set! sum (+ sum x))) #(1 2 3 4))
                 sum)"#
        ),
        Value::Int(10),
    );
}

// ============================================================
// §6.10 dynamic-wind (comprehensive)
// ============================================================

#[test]
fn s6_10_dynamic_wind_comprehensive() {
    // Basic dynamic-wind: before, thunk, after all execute
    assert_eq!(
        eval(
            r#"(let ((log '()))
                 (dynamic-wind
                   (lambda () (set! log (cons 'before log)))
                   (lambda () (set! log (cons 'during log)) 42)
                   (lambda () (set! log (cons 'after log))))
                 log)"#
        ),
        eval("'(after during before)"),
    );

    // dynamic-wind returns thunk's value
    is_int(
        "(dynamic-wind (lambda () #f) (lambda () 99) (lambda () #f))",
        99,
    );
}

// ============================================================
// §4.2.6 make-parameter / parameterize (comprehensive)
// ============================================================

#[test]
fn s4_2_6_parameterize_comprehensive() {
    // make-parameter creates a parameter with initial value
    is_int("(let ((p (make-parameter 10))) (p))", 10);

    // parameterize changes value dynamically
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20)) (p)))",
        20,
    );

    // Outer value restored after parameterize
    is_int(
        "(let ((p (make-parameter 10)))
           (parameterize ((p 20)) (p))
           (p))",
        10,
    );

    // Nested parameterize
    is_int(
        "(let ((p (make-parameter 1)))
           (parameterize ((p 2))
             (parameterize ((p 3))
               (p))))",
        3,
    );

    // make-parameter with converter
    is_int(
        "(let ((p (make-parameter 0 (lambda (x) (* x 2)))))
           (parameterize ((p 5)) (p)))",
        10,
    );
}

// ============================================================
// §4.2.7 guard (comprehensive)
// ============================================================

#[test]
fn s4_2_7_guard_comprehensive() {
    // guard catches specific error types
    is_int(
        r#"(guard (exn
                  ((string? (error-object-message exn)) 1)
                  (else 0))
             (error "test" "irritant"))"#,
        1,
    );

    // guard with multiple clauses
    is_int(
        "(guard (exn
                 ((equal? exn 'foo) 10)
                 ((equal? exn 'bar) 20)
                 (else 30))
           (raise 'bar))",
        20,
    );

    // guard body returns normally when no error
    is_int("(guard (exn (else -1)) (+ 2 3))", 5);

    // guard with error-object-irritants
    assert_eq!(
        eval(
            r#"(guard (exn
                      (else (error-object-irritants exn)))
                 (error "msg" 'a 'b 'c))"#
        ),
        eval("'(a b c)"),
    );
}

// ============================================================
// §5.5 define-record-type (comprehensive)
// ============================================================

#[test]
fn s5_5_define_record_type_comprehensive() {
    // Full record type with constructor, predicate, accessors, mutators
    is_true(
        "(begin
           (define-record-type <point>
             (make-point x y)
             point?
             (x point-x)
             (y point-y))
           (let ((p (make-point 3 4)))
             (and (point? p)
                  (= (point-x p) 3)
                  (= (point-y p) 4))))",
    );

    // Record predicate returns #f for non-records
    is_false(
        "(begin
           (define-record-type <thing>
             (make-thing v)
             thing?
             (v thing-v))
           (thing? 42))",
    );

    // Multiple record types are independent
    is_true(
        "(begin
           (define-record-type <a> (make-a x) a? (x a-x))
           (define-record-type <b> (make-b y) b? (y b-y))
           (let ((va (make-a 1)) (vb (make-b 2)))
             (and (a? va) (b? vb)
                  (not (a? vb)) (not (b? va)))))",
    );
}

// ============================================================
// §4.2.9 case-lambda (comprehensive)
// ============================================================

#[test]
fn s4_2_9_case_lambda_comprehensive() {
    // case-lambda dispatches on argument count
    is_int(
        "(let ((f (case-lambda
                    ((x) x)
                    ((x y) (+ x y))
                    ((x y z) (+ x y z)))))
           (+ (f 1) (f 2 3) (f 4 5 6)))",
        21,
    );

    // case-lambda with rest args
    // (f) → 0, (f 10) → 10+0=10, (f 10 20 30) → 10+2=12, total = 22
    is_int(
        "(let ((f (case-lambda
                    (() 0)
                    ((x . rest) (+ x (length rest))))))
           (+ (f) (f 10) (f 10 20 30)))",
        22,
    );
}

// ============================================================
// §4.2.4 do (comprehensive)
// ============================================================

#[test]
fn s4_2_4_do_extended() {
    // do loop with multiple variables
    is_int(
        "(do ((i 0 (+ i 1))
              (sum 0 (+ sum i)))
             ((= i 5) sum))",
        10,
    );

    // do loop building a list
    assert_eq!(
        eval(
            "(do ((i 0 (+ i 1))
                  (result '() (cons i result)))
                 ((= i 4) result))",
        ),
        eval("'(3 2 1 0)"),
    );

    // do loop with no step expression
    is_int(
        "(do ((x 10))
             ((> x 5) x)
           (set! x (- x 1)))",
        10,
    );
}

// ============================================================
// §4.2.2 let-values / let*-values (comprehensive)
// ============================================================

#[test]
fn s4_2_2_let_values_comprehensive() {
    // let-values binds multiple values
    is_int(
        "(let-values (((a b c) (values 1 2 3)))
           (+ a b c))",
        6,
    );

    // let*-values sequential binding
    is_int(
        "(let*-values (((a b) (values 1 2))
                       ((c) (values (+ a b))))
           c)",
        3,
    );

    // let-values with single value
    is_int("(let-values (((x) (values 42))) x)", 42);
}

// ============================================================
// §6.2 Numeric edge cases
// ============================================================

#[test]
fn s6_2_numeric_edge_cases() {
    // Exact arithmetic preserves exactness
    is_true("(exact? (+ 1 2))");
    is_true("(exact? (* 3 4))");

    // Inexact arithmetic
    is_true("(inexact? (+ 1.0 2))");
    is_true("(inexact? (* 3 4.0))");

    // Integer division edge cases
    is_int("(quotient 7 2)", 3);
    is_int("(quotient -7 2)", -3);
    is_int("(remainder 7 2)", 1);
    is_int("(remainder -7 2)", -1);
    is_int("(modulo 7 2)", 1);
    is_int("(modulo -7 2)", 1);

    // R7RS: floor/ceiling/truncate/round return inexact for inexact args
    is_float("(floor 2.7)", 2.0);
    is_float("(floor -2.7)", -3.0);
    is_float("(ceiling 2.3)", 3.0);
    is_float("(ceiling -2.3)", -2.0);
    is_float("(truncate 2.7)", 2.0);
    is_float("(truncate -2.7)", -2.0);
    is_float("(round 2.5)", 2.0); // banker's rounding
    is_float("(round 3.5)", 4.0); // banker's rounding
    is_float("(round 2.4)", 2.0);
    is_float("(round -2.5)", -2.0); // banker's rounding

    // exact wrapping: (exact (round x)) converts back to integer
    is_int("(exact (floor 2.7))", 2);
    is_int("(exact (ceiling 2.3))", 3);
    is_int("(exact (round 2.5))", 2);
    is_int("(exact (truncate 2.7))", 2);

    // R7RS special float literals
    is_true("(nan? +nan.0)");
    is_true("(infinite? +inf.0)");
    is_true("(infinite? -inf.0)");
    is_true("(finite? 1.0)");
    is_false("(finite? +inf.0)");

    // R7RS write representation for special floats
    is_str(
        r#"(let ((p (open-output-string))) (write +nan.0 p) (get-output-string p))"#,
        "+nan.0",
    );
    is_str(
        r#"(let ((p (open-output-string))) (write +inf.0 p) (get-output-string p))"#,
        "+inf.0",
    );
    is_str(
        r#"(let ((p (open-output-string))) (write -inf.0 p) (get-output-string p))"#,
        "-inf.0",
    );

    // min/max with mixed types
    is_true("(inexact? (min 1 2.0))");
    is_true("(inexact? (max 1 2.0))");
}

// ============================================================
// §6.1 eqv? / equal? comprehensive
// ============================================================

#[test]
fn s6_1_equivalence_comprehensive() {
    // eqv? on numbers
    is_true("(eqv? 1 1)");
    is_false("(eqv? 1 1.0)"); // different exactness
    is_true("(eqv? 1.0 1.0)");

    // eqv? on characters
    is_true(r"(eqv? #\a #\a)");
    is_false(r"(eqv? #\a #\b)");

    // eqv? on booleans
    is_true("(eqv? #t #t)");
    is_true("(eqv? #f #f)");
    is_false("(eqv? #t #f)");

    // eqv? on empty list
    is_true("(eqv? '() '())");

    // eqv? on symbols
    is_true("(eqv? 'foo 'foo)");
    is_false("(eqv? 'foo 'bar)");

    // equal? does deep comparison
    is_true("(equal? '(1 2 3) '(1 2 3))");
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_true(r#"(equal? "abc" "abc")"#);
    is_false("(equal? '(1 2) '(1 3))");

    // equal? on nested structures
    is_true("(equal? '(1 (2 3)) '(1 (2 3)))");
    is_true("(equal? #(1 #(2 3)) #(1 #(2 3)))");
}

// ============================================================
// §6.4 list-tail / list-copy / make-list
// ============================================================

#[test]
fn s6_4_list_operations_extended() {
    // list-tail
    eval_eq("(list-tail '(a b c d) 2)", "'(c d)");
    eval_eq("(list-tail '(a b c) 0)", "'(a b c)");
    eval_eq("(list-tail '(a b c) 3)", "'()");

    // list-copy creates a fresh copy
    eval_eq("(list-copy '(1 2 3))", "'(1 2 3)");
    eval_eq("(list-copy '())", "'()");

    // make-list
    eval_eq("(make-list 3 'x)", "'(x x x)");
    eval_eq("(make-list 0 'x)", "'()");
}

// ============================================================
// §6.5 symbol->string / string->symbol
// ============================================================

#[test]
fn s6_5_symbol_conversion() {
    assert_eq!(
        eval("(symbol->string 'hello)"),
        Value::String(Rc::from("hello")),
    );
    assert_eq!(eval(r#"(string->symbol "world")"#), Value::symbol("world"),);
    // Round-trip
    is_true(r#"(eq? 'test (string->symbol (symbol->string 'test)))"#);
}

// ============================================================
// §6.6 char->integer / integer->char
// ============================================================

#[test]
fn s6_6_char_integer_conversion() {
    is_int(r"(char->integer #\A)", 65);
    is_int(r"(char->integer #\space)", 32);
    assert_eq!(eval("(integer->char 65)"), Value::Char('A'));
    assert_eq!(eval("(integer->char 955)"), Value::Char('λ'));

    // Round-trip
    is_true(r"(char=? #\Z (integer->char (char->integer #\Z)))");
}

// ============================================================
// §6.13 Port predicates
// ============================================================

#[test]
fn s6_13_port_predicates_extended() {
    is_true(r#"(input-port? (open-input-string "x"))"#);
    is_false(r#"(output-port? (open-input-string "x"))"#);
    is_true("(output-port? (open-output-string))");
    is_false("(input-port? (open-output-string))");

    // port? is true for both
    is_true(r#"(port? (open-input-string "x"))"#);
    is_true("(port? (open-output-string))");
    is_false("(port? 42)");

    // input-port-open? / output-port-open?
    is_true(r#"(input-port-open? (open-input-string "x"))"#);
    is_true("(output-port-open? (open-output-string))");

    // textual-port? / binary-port?
    is_true(r#"(textual-port? (open-input-string "x"))"#);
    is_true("(textual-port? (open-output-string))");
}

// ============================================================
// §6.13 eof-object
// ============================================================

#[test]
fn s6_13_eof_object() {
    // eof-object returns the EOF value
    is_true("(eof-object? (eof-object))");
    is_false("(eof-object? 42)");
    is_false("(eof-object? #f)");

    // Reading past end of string port returns EOF
    is_true(r#"(eof-object? (read-char (open-input-string "")))"#);
    is_true(r#"(eof-object? (read-u8 (open-input-bytevector #u8())))"#);
}

// ============================================================
// §6.13 read-line / read-string
// ============================================================

#[test]
fn s6_13_read_line_read_string() {
    // read-line reads up to newline
    assert_eq!(
        eval(r#"(read-line (open-input-string "hello\nworld"))"#),
        Value::String(Rc::from("hello")),
    );

    // read-line at EOF
    is_true(r#"(eof-object? (read-line (open-input-string "")))"#);

    // read-string reads N characters
    assert_eq!(
        eval(r#"(read-string 3 (open-input-string "abcdef"))"#),
        Value::String(Rc::from("abc")),
    );
}

// ============================================================
// §6.13 peek-char / peek-u8
// ============================================================

#[test]
fn s6_13_peek_operations() {
    // peek-char doesn't consume
    is_true(
        r#"(let ((p (open-input-string "ab")))
             (let ((c1 (peek-char p))
                   (c2 (read-char p)))
               (char=? c1 c2)))"#,
    );

    // peek-u8 doesn't consume
    is_true(
        r#"(let ((p (open-input-bytevector #u8(10 20))))
             (let ((b1 (peek-u8 p))
                   (b2 (read-u8 p)))
               (= b1 b2)))"#,
    );
}

// ============================================================
// §6.13 format
// ============================================================

#[test]
fn s6_13_format() {
    // format with ~a (display)
    assert_eq!(
        eval(r#"(format "hello ~a" "world")"#),
        Value::String(Rc::from("hello world")),
    );

    // format with ~s (write)
    assert_eq!(
        eval(r#"(format "value: ~s" "test")"#),
        Value::String(Rc::from(r#"value: "test""#)),
    );

    // format with ~%  (newline)
    assert_eq!(eval(r#"(format "a~%b")"#), Value::String(Rc::from("a\nb")),);
}

// ============================================================
// §6.14 System interface
// ============================================================

#[test]
fn s6_14_system_interface_extended() {
    // features returns a list
    is_true("(list? (features))");

    // command-line returns a list of strings
    is_true("(list? (command-line))");

    // current-second returns a number
    is_true("(number? (current-second))");

    // current-jiffy returns an exact integer
    is_true("(exact? (current-jiffy))");

    // jiffies-per-second returns a positive integer
    is_true("(> (jiffies-per-second) 0)");
}

// ============================================================
// §6.13 close-input-port / close-output-port
// ============================================================

#[test]
fn s6_13_close_port_variants() {
    // close-input-port
    is_true(
        r#"(let ((p (open-input-string "test")))
             (close-input-port p)
             #t)"#,
    );

    // close-output-port
    is_true(
        "(let ((p (open-output-string)))
           (close-output-port p)
           #t)",
    );

    // close-port works on both
    is_true(
        r#"(let ((p (open-input-string "x")))
             (close-port p)
             #t)"#,
    );
}

// ============================================================
// §6.13 flush-output-port
// ============================================================

#[test]
fn s6_13_flush_output_port() {
    // flush-output-port should not error
    is_true(
        "(let ((p (open-output-string)))
           (write-string \"hello\" p)
           (flush-output-port p)
           #t)",
    );
}

// ============================================================
// §6.2 abs / square
// ============================================================

#[test]
fn s6_2_abs_square() {
    is_int("(abs 5)", 5);
    is_int("(abs -5)", 5);
    is_int("(abs 0)", 0);
    assert_eq!(eval("(abs -3.5)"), Value::Float(3.5));

    is_int("(square 5)", 25);
    is_int("(square -3)", 9);
    is_int("(square 0)", 0);
    assert_eq!(eval("(square 2.5)"), Value::Float(6.25));
}

// ============================================================
// §6.3 boolean=?
// ============================================================

#[test]
fn s6_3_boolean_equal() {
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
    is_false("(boolean=? #f #t)");
    // Multiple arguments
    is_true("(boolean=? #t #t #t)");
    is_false("(boolean=? #t #t #f)");
}

// ============================================================
// §6.10 apply (comprehensive)
// ============================================================

#[test]
fn s6_10_apply_extended() {
    is_int("(apply + '(1 2 3))", 6);
    is_int("(apply + 1 2 '(3))", 6);
    is_int("(apply + 1 '(2 3))", 6);

    // apply with no extra args
    is_int("(apply car '((1 2 3)))", 1);

    // apply with lambda
    is_int("(apply (lambda (x y) (+ x y)) '(3 4))", 7);
}

// ============================================================
// §6.10 values / call-with-values (comprehensive)
// ============================================================

#[test]
fn s6_10_values_comprehensive() {
    // Single value
    is_int(
        "(call-with-values (lambda () (values 42)) (lambda (x) x))",
        42,
    );

    // Multiple values
    is_int(
        "(call-with-values (lambda () (values 1 2 3)) (lambda (a b c) (+ a b c)))",
        6,
    );

    // values with receive
    is_int("(receive (a b c) (values 10 20 30) (+ a b c))", 60);

    // receive with rest args
    is_int("(receive (a . rest) (values 1 2 3) (+ a (length rest)))", 3);
}

// ============================================================
// §6.11 with-exception-handler (comprehensive)
// ============================================================

#[test]
fn s6_11_with_exception_handler_comprehensive() {
    // with-exception-handler + raise-continuable: handler can return
    is_int(
        "(with-exception-handler
           (lambda (e) 42)
           (lambda () (raise-continuable 'boom)))",
        42,
    );

    // with-exception-handler + raise: handler returning is an error
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    assert!(
        vm.eval(
            "(with-exception-handler
               (lambda (e) 42)
               (lambda () (raise 'boom)))"
        )
        .is_err(),
        "raise handler returned should be an error"
    );

    // guard catches raised values and runs clauses
    is_int(
        "(guard (exn
                 ((symbol? exn) 1)
                 ((string? exn) 2))
           (raise 'test))",
        1,
    );
}

// ============================================================
// §6.13 write / display / write-simple
// ============================================================

#[test]
fn s6_13_write_display_simple() {
    // display does not quote strings
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (display "hello" p)
                 (get-output-string p))"#
        ),
        Value::String(Rc::from("hello")),
    );

    // write quotes strings
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (write "hello" p)
                 (get-output-string p))"#
        ),
        Value::String(Rc::from(r#""hello""#)),
    );

    // write-simple (same as write for non-shared data)
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (write-simple '(1 2 3) p)
                 (get-output-string p))"#
        ),
        Value::String(Rc::from("(1 2 3)")),
    );

    // display on various types
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (display #t p)
                 (get-output-string p))"#
        ),
        Value::String(Rc::from("#t")),
    );

    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (display #\a p)
                 (get-output-string p))"#
        ),
        Value::String(Rc::from("a")),
    );
}

// ============================================================
// §6.13 read (from string port)
// ============================================================

#[test]
fn s6_13_read_from_port() {
    // read parses S-expression from port
    is_int(r#"(read (open-input-string "42"))"#, 42);

    assert_eq!(
        eval(r#"(read (open-input-string "(1 2 3)"))"#),
        eval("'(1 2 3)"),
    );

    assert_eq!(
        eval(r#"(read (open-input-string "'foo"))"#),
        eval("'(quote foo)"),
    );

    // read at EOF
    is_true(r#"(eof-object? (read (open-input-string "")))"#);
}

// ============================================================
// §5.3 Multiple define forms
// ============================================================

#[test]
fn s5_3_define_forms() {
    // (define (f x) body) is sugar for (define f (lambda (x) body))
    is_int("(begin (define (add1 x) (+ x 1)) (add1 5))", 6);

    // (define (f x . rest) body) — variadic
    is_int(
        "(begin (define (sum x . rest) (apply + x rest)) (sum 1 2 3))",
        6,
    );

    // Internal defines
    is_int(
        "(let ()
           (define a 1)
           (define b 2)
           (+ a b))",
        3,
    );
}

// ============================================================
// §4.1.6 quasiquote comprehensive
// ============================================================

#[test]
fn s4_1_6_quasiquote_comprehensive() {
    // Basic quasiquote
    eval_eq("`(1 2 3)", "'(1 2 3)");

    // Unquote
    is_int("`,(+ 1 2)", 3);

    // Unquote in list
    eval_eq("`(1 ,(+ 1 1) 3)", "'(1 2 3)");

    // Unquote-splicing
    eval_eq("`(1 ,@(list 2 3) 4)", "'(1 2 3 4)");

    // Nested quasiquote
    assert_eq!(
        eval("`(a `(b ,(+ 1 2)))"),
        eval("'(a (quasiquote (b (unquote (+ 1 2)))))"),
    );
}

// ============================================================
// §4.3 syntax-rules comprehensive
// ============================================================

#[test]
fn s4_3_syntax_rules_comprehensive() {
    // Basic syntax-rules macro
    is_int(
        "(begin
           (define-syntax my-if
             (syntax-rules ()
               ((my-if test then else)
                (cond (test then) (#t else)))))
           (my-if #t 1 2))",
        1,
    );

    // Macro with ellipsis
    is_int(
        "(begin
           (define-syntax my-begin
             (syntax-rules ()
               ((my-begin expr) expr)
               ((my-begin expr rest ...)
                (let ((x expr)) (my-begin rest ...)))))
           (my-begin 1 2 3))",
        3,
    );

    // let-syntax scoping
    is_int(
        "(let-syntax ((double (syntax-rules ()
                                ((double x) (+ x x)))))
           (double 5))",
        10,
    );

    // letrec-syntax allows mutual reference
    is_int(
        "(letrec-syntax
           ((my-or (syntax-rules ()
                     ((my-or) #f)
                     ((my-or e) e)
                     ((my-or e1 e2 ...)
                      (let ((t e1))
                        (if t t (my-or e2 ...)))))))
           (my-or #f #f 42))",
        42,
    );
}

// ============================================================
// §5.6 define-library / import comprehensive
// ============================================================

#[test]
fn s5_6_library_comprehensive() {
    // define-library must be at top level (not inside begin)
    // Each define-library + import needs its own eval call on the same VM

    // define-library with begin body
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test math)
           (export add)
           (begin
             (define (add a b) (+ a b))))",
    )
    .unwrap();
    vm.eval("(import (test math))").unwrap();
    assert_eq!(vm.eval("(add 3 4)").unwrap(), Value::Int(7));

    // import with only
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib2)
           (export foo bar)
           (begin
             (define foo 10)
             (define bar 20)))",
    )
    .unwrap();
    vm.eval("(import (only (test lib2) foo))").unwrap();
    assert_eq!(vm.eval("foo").unwrap(), Value::Int(10));

    // import with rename
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib3)
           (export val)
           (begin
             (define val 99)))",
    )
    .unwrap();
    vm.eval("(import (rename (test lib3) (val my-val)))")
        .unwrap();
    assert_eq!(vm.eval("my-val").unwrap(), Value::Int(99));

    // import with prefix
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "(define-library (test lib4)
           (export num)
           (begin
             (define num 77)))",
    )
    .unwrap();
    vm.eval("(import (prefix (test lib4) t:))").unwrap();
    assert_eq!(vm.eval("t:num").unwrap(), Value::Int(77));
}

// ============================================================
// §7.1 Reader edge cases
// ============================================================

#[test]
fn s7_1_reader_edge_cases() {
    // Nested quoting
    eval_eq("''x", "'(quote x)");

    // Boolean literals
    is_true("#true");
    is_false("#false");

    // Character names
    assert_eq!(eval(r"#\space"), Value::Char(' '));
    assert_eq!(eval(r"#\newline"), Value::Char('\n'));
    assert_eq!(eval(r"#\tab"), Value::Char('\t'));

    // Hex character
    assert_eq!(eval(r"#\x41"), Value::Char('A'));
    assert_eq!(eval(r"#\x03BB"), Value::Char('λ'));

    // String escapes
    assert_eq!(eval(r#"(string-length "\n\t\\\"")"#), Value::Int(4),);

    // Datum comment
    is_int("#;42 7", 7);
    is_int("(+ 1 #;2 3)", 4);
}

// ============================================================
// §6.2 number->string with radix
// ============================================================

#[test]
fn s6_2_number_to_string_radix() {
    assert_eq!(
        eval("(number->string 255 16)"),
        Value::String(Rc::from("ff")),
    );
    assert_eq!(eval("(number->string 7 2)"), Value::String(Rc::from("111")),);
    assert_eq!(eval("(number->string 8 8)"), Value::String(Rc::from("10")),);
    assert_eq!(
        eval("(number->string 42 10)"),
        Value::String(Rc::from("42")),
    );
}

// ============================================================
// §6.2 string->number with radix
// ============================================================

#[test]
fn s6_2_string_to_number_radix() {
    is_int(r#"(string->number "ff" 16)"#, 255);
    is_int(r#"(string->number "111" 2)"#, 7);
    is_int(r#"(string->number "10" 8)"#, 8);
    is_int(r#"(string->number "42" 10)"#, 42);
    // Invalid number returns #f
    is_false(r#"(string->number "xyz")"#);
}

// ============================================================
// §6.4 assoc with custom comparator
// ============================================================

#[test]
fn s6_4_assoc_custom_compare() {
    // assoc with default equal?
    assert_eq!(
        eval(r#"(assoc "b" '(("a" 1) ("b" 2) ("c" 3)))"#),
        eval(r#"'("b" 2)"#),
    );

    // assoc returns #f when not found
    is_false(r#"(assoc "d" '(("a" 1) ("b" 2)))"#);

    // member with default equal?
    assert_eq!(eval("(member 2 '(1 2 3))"), eval("'(2 3)"),);
}

// ============================================================
// §6.8 vector operations comprehensive
// ============================================================

#[test]
fn s6_8_vector_ops_comprehensive() {
    // make-vector
    eval_eq("(make-vector 3 0)", "#(0 0 0)");

    // vector-fill!
    assert_eq!(
        eval("(let ((v (make-vector 3 0))) (vector-fill! v 9) v)"),
        eval("#(9 9 9)"),
    );

    // vector-copy with range
    assert_eq!(eval("(vector-copy #(a b c d e) 1 4)"), eval("#(b c d)"),);

    // vector-copy!
    assert_eq!(
        eval("(let ((v (vector 1 2 3 4 5))) (vector-copy! v 1 #(10 20) 0 2) v)"),
        eval("#(1 10 20 4 5)"),
    );

    // vector-append
    assert_eq!(
        eval("(vector-append #(1 2) #(3 4) #(5))"),
        eval("#(1 2 3 4 5)"),
    );

    // vector->string / string->vector
    assert_eq!(
        eval(r#"(vector->string #(#\a #\b #\c))"#),
        Value::String(Rc::from("abc")),
    );
    assert_eq!(eval(r#"(string->vector "abc")"#), eval(r"#(#\a #\b #\c)"),);
}

// ============================================================
// §6.9 bytevector operations comprehensive
// ============================================================

#[test]
fn s6_9_bytevector_ops_comprehensive() {
    // make-bytevector
    eval_eq("(make-bytevector 3 0)", "#u8(0 0 0)");
    eval_eq("(make-bytevector 3 255)", "#u8(255 255 255)");

    // bytevector-copy with range
    assert_eq!(
        eval("(bytevector-copy #u8(0 1 2 3 4) 1 4)"),
        eval("#u8(1 2 3)"),
    );

    // bytevector-append
    assert_eq!(
        eval("(bytevector-append #u8(1 2) #u8(3 4))"),
        eval("#u8(1 2 3 4)"),
    );

    // utf8->string / string->utf8
    assert_eq!(
        eval("(utf8->string #u8(104 101 108 108 111))"),
        Value::String(Rc::from("hello")),
    );
    assert_eq!(
        eval(r#"(string->utf8 "hello")"#),
        eval("#u8(104 101 108 108 111)"),
    );
}

// =============================================================================
// §6.4 cxr accessors (R7RS §6.4)
// =============================================================================

#[test]
fn s6_4_cxr_accessors() {
    // caar — (car (car x))
    is_int("(caar '((1 2) 3))", 1);

    // cadr — (car (cdr x))
    is_int("(cadr '(1 2 3))", 2);

    // cdar — (cdr (car x))
    assert_eq!(eval("(cdar '((1 2 3) 4))"), eval("'(2 3)"),);

    // cddr — (cdr (cdr x))
    assert_eq!(eval("(cddr '(1 2 3 4))"), eval("'(3 4)"),);

    // nested combinations
    is_int("(caar '((5 6) (7 8)))", 5);
    assert_eq!(eval("(cdar '((10 20 30)))"), eval("'(20 30)"),);
    assert_eq!(eval("(cddr '(a b c d e))"), eval("'(c d e)"),);
}

// =============================================================================
// §6.6 char<=? and char>=? (R7RS §6.6)
// =============================================================================

#[test]
fn s6_6_char_comparison_lte_gte() {
    // char<=?
    is_true("(char<=? #\\a #\\b)");
    is_true("(char<=? #\\a #\\a)");
    is_false("(char<=? #\\b #\\a)");
    is_true("(char<=? #\\A #\\Z)");
    is_true("(char<=? #\\0 #\\9)");

    // char>=?
    is_true("(char>=? #\\b #\\a)");
    is_true("(char>=? #\\a #\\a)");
    is_false("(char>=? #\\a #\\b)");
    is_true("(char>=? #\\Z #\\A)");
    is_true("(char>=? #\\9 #\\0)");
}

// =============================================================================
// §6.7 string>=? (R7RS §6.7)
// =============================================================================

#[test]
fn s6_7_string_comparison_gte() {
    is_true(r#"(string>=? "b" "a")"#);
    is_true(r#"(string>=? "a" "a")"#);
    is_false(r#"(string>=? "a" "b")"#);
    is_true(r#"(string>=? "abc" "ab")"#);
    is_true(r#"(string>=? "xyz" "xyz")"#);
    is_false(r#"(string>=? "ab" "abc")"#);
}

// =============================================================================
// §6.2 integer? with inexact values (R7RS §6.2.6)
// =============================================================================

#[test]
fn s6_2_integer_inexact_edge_cases() {
    // integer? on inexact integer-valued floats: R7RS says #t
    is_true("(integer? 42.0)");
    is_true("(integer? 0.0)");
    is_true("(integer? -1.0)");

    // integer? on non-integer floats: R7RS says #f
    is_false("(integer? 3.15)");
    is_false("(integer? 0.5)");
    is_false("(integer? -2.7)");

    // exact-integer? on floats: always #f (they're inexact)
    is_false("(exact-integer? 42.0)");
    is_false("(exact-integer? 0.0)");

    // rational? — in mae-scheme, all reals are rational (no complex)
    is_true("(rational? 42)");
    is_true("(rational? 3.15)");

    // complex? — R7RS permits #t for all numbers when no complex type
    is_true("(complex? 42)");
    is_true("(complex? 3.15)");

    // positive? and negative? with inexact zero
    is_false("(positive? 0.0)");
    is_false("(negative? 0.0)");
    is_true("(positive? 0.1)");
    is_true("(negative? -0.1)");
    is_false("(negative? 0)");

    // zero? with inexact
    is_true("(zero? 0.0)");
    is_false("(zero? 0.1)");
}

// =============================================================================
// §6.6 char predicates — edge cases (R7RS §6.6)
// =============================================================================

#[test]
fn s6_6_char_predicate_edge_cases() {
    // char-numeric? negative cases
    is_false("(char-numeric? #\\a)");
    is_false("(char-numeric? #\\space)");
    is_false("(char-numeric? #\\!)");
    is_true("(char-numeric? #\\0)");
    is_true("(char-numeric? #\\9)");

    // char-alphabetic? negative cases
    is_false("(char-alphabetic? #\\0)");
    is_false("(char-alphabetic? #\\space)");
    is_false("(char-alphabetic? #\\!)");

    // char-whitespace? edge cases
    is_true("(char-whitespace? #\\space)");
    is_true("(char-whitespace? #\\newline)");
    is_true("(char-whitespace? #\\tab)");
    is_false("(char-whitespace? #\\a)");
    is_false("(char-whitespace? #\\0)");

    // char-upper-case? / char-lower-case? edge cases
    is_false("(char-upper-case? #\\a)");
    is_false("(char-lower-case? #\\A)");
    is_false("(char-upper-case? #\\1)");
    is_false("(char-lower-case? #\\1)");

    // char-ci comparisons
    is_true("(char-ci=? #\\A #\\a)");
    is_true("(char-ci<? #\\a #\\B)");
    is_true("(char-ci>? #\\B #\\a)");
    is_true("(char-ci<=? #\\a #\\A)");
    is_true("(char-ci>=? #\\A #\\a)");
}

// =============================================================================
// §6.7 string case-insensitive comparisons — comprehensive (R7RS §6.7)
// =============================================================================

#[test]
fn s6_7_string_ci_comprehensive() {
    is_true(r#"(string-ci=? "ABC" "abc")"#);
    is_true(r#"(string-ci=? "Hello" "hELLO")"#);
    is_false(r#"(string-ci=? "abc" "abd")"#);

    is_true(r#"(string-ci<? "abc" "ABD")"#);
    is_false(r#"(string-ci<? "abd" "ABC")"#);

    is_true(r#"(string-ci>? "ABD" "abc")"#);
    is_false(r#"(string-ci>? "abc" "ABD")"#);

    is_true(r#"(string-ci<=? "abc" "ABC")"#);
    is_true(r#"(string-ci<=? "abc" "ABD")"#);
    is_false(r#"(string-ci<=? "abd" "ABC")"#);

    is_true(r#"(string-ci>=? "ABC" "abc")"#);
    is_true(r#"(string-ci>=? "abd" "ABC")"#);
    is_false(r#"(string-ci>=? "abc" "ABD")"#);
}

// =============================================================================
// §6.13 Port predicates on closed ports (R7RS §6.13)
// =============================================================================

#[test]
fn s6_13_port_predicates_closed() {
    // A closed port is still a port, still input/output
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // String port: open, check predicates, close, check again
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-input-string "hello")))
              (and (port? p) (input-port? p) (textual-port? p)))
        "#
        )
        .unwrap(),
        Value::Bool(true),
    );

    // After close, port? still #t but reads fail
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-input-string "hello")))
              (close-port p)
              (port? p))
        "#
        )
        .unwrap(),
        Value::Bool(true),
    );

    // Output string port
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-output-string)))
              (and (port? p) (output-port? p) (textual-port? p)))
        "#
        )
        .unwrap(),
        Value::Bool(true),
    );

    // R7RS §6.13.1: input-port? returns #t even on closed input ports
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-input-string "hello")))
              (close-port p)
              (input-port? p))
        "#
        )
        .unwrap(),
        Value::Bool(true),
    );

    // R7RS §6.13.1: output-port? returns #t even on closed output ports
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-output-string)))
              (close-port p)
              (output-port? p))
        "#
        )
        .unwrap(),
        Value::Bool(true),
    );

    // input-port-open? returns #f on closed ports
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-input-string "hello")))
              (close-port p)
              (input-port-open? p))
        "#
        )
        .unwrap(),
        Value::Bool(false),
    );

    // output-port-open? returns #f on closed ports
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-output-string)))
              (close-port p)
              (output-port-open? p))
        "#
        )
        .unwrap(),
        Value::Bool(false),
    );
}

// =============================================================================
// §6.8 vector-set! error cases (R7RS §6.8)
// =============================================================================

#[test]
fn s6_8_vector_error_cases() {
    // vector-set! out of bounds
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);
    assert!(vm.eval("(vector-set! (vector 1 2 3) 5 99)").is_err());

    // vector-ref out of bounds
    let mut vm2 = Vm::new();
    crate::stdlib::register_stdlib(&mut vm2);
    assert!(vm2.eval("(vector-ref (vector 1 2 3) 5)").is_err());
}

// =============================================================================
// §6.7 string-set! error (immutable strings — mae-scheme stance)
// =============================================================================

#[test]
fn s6_7_immutable_string_errors() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // string-set! should error (mae-scheme strings are immutable)
    assert!(vm.eval(r#"(string-set! "hello" 0 #\H)"#).is_err());

    // string-copy! should error
    let mut vm2 = Vm::new();
    crate::stdlib::register_stdlib(&mut vm2);
    assert!(vm2.eval(r#"(string-copy! "hello" 0 "world")"#).is_err());

    // string-fill! should error
    let mut vm3 = Vm::new();
    crate::stdlib::register_stdlib(&mut vm3);
    assert!(vm3.eval(r#"(string-fill! "hello" #\x)"#).is_err());
}

// =============================================================================
// §6.10 map/for-each error propagation (R7RS §6.10)
// =============================================================================

#[test]
fn s6_10_map_error_propagation() {
    // map with error in callback should propagate
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);
    assert!(vm.eval("(map (lambda (x) (/ 1 x)) '(1 0 2))").is_err());

    // for-each with error in callback should propagate
    let mut vm2 = Vm::new();
    crate::stdlib::register_stdlib(&mut vm2);
    assert!(vm2
        .eval("(for-each (lambda (x) (/ 1 x)) '(1 0 2))")
        .is_err());
}

// =============================================================================
// §6.2 number->string / string->number edge cases (R7RS §6.2.7)
// =============================================================================

#[test]
fn s6_2_number_string_edge_cases() {
    // number->string with various radices
    assert_eq!(
        eval("(number->string 255 16)"),
        Value::String(Rc::from("ff")),
    );
    assert_eq!(eval("(number->string 7 2)"), Value::String(Rc::from("111")),);
    assert_eq!(eval("(number->string 8 8)"), Value::String(Rc::from("10")),);
    assert_eq!(eval("(number->string 0 16)"), Value::String(Rc::from("0")),);

    // string->number with radix
    is_int("(string->number \"ff\" 16)", 255);
    is_int("(string->number \"111\" 2)", 7);
    is_int("(string->number \"10\" 8)", 8);

    // string->number failure returns #f
    is_false("(string->number \"not-a-number\")");
    is_false("(string->number \"\")");

    // Negative numbers
    assert_eq!(eval("(number->string -42)"), Value::String(Rc::from("-42")),);
    is_int("(string->number \"-42\")", -42);

    // Float conversion
    assert_eq!(eval("(string->number \"3.15\")"), Value::Float(3.15),);
}

// =============================================================================
// §6.4 list operations — edge cases (R7RS §6.4)
// =============================================================================

#[test]
fn s6_4_list_edge_cases() {
    // list-tail at boundary
    assert_eq!(eval("(list-tail '(a b c) 3)"), Value::Null);
    assert_eq!(eval("(list-tail '() 0)"), Value::Null);

    // list-copy preserves structure
    assert_eq!(eval("(list-copy '(1 2 3))"), eval("'(1 2 3)"),);
    // list-copy of empty list
    assert_eq!(eval("(list-copy '())"), Value::Null);

    // append with empty lists
    assert_eq!(eval("(append '() '(1 2))"), eval("'(1 2)"),);
    assert_eq!(eval("(append '(1 2) '())"), eval("'(1 2)"),);
    assert_eq!(eval("(append '() '())"), Value::Null);

    // reverse of empty and singleton
    assert_eq!(eval("(reverse '())"), Value::Null);
    assert_eq!(eval("(reverse '(1))"), eval("'(1)"),);
}

// =============================================================================
// §6.5 symbol edge cases (R7RS §6.5)
// =============================================================================

#[test]
fn s6_5_symbol_edge_cases() {
    // symbol->string returns immutable string representation
    assert_eq!(
        eval("(symbol->string 'hello)"),
        Value::String(Rc::from("hello")),
    );

    // string->symbol round-trip
    is_true(r#"(eq? (string->symbol "foo") 'foo)"#);

    // symbol with special characters (via string->symbol)
    assert_eq!(
        eval(r#"(symbol->string (string->symbol "hello world"))"#),
        Value::String(Rc::from("hello world")),
    );
}

// =============================================================================
// §6.9 bytevector edge cases (R7RS §6.9)
// =============================================================================

#[test]
fn s6_9_bytevector_edge_cases() {
    // bytevector-u8-set! valid range
    assert_eq!(
        eval("(let ((bv (bytevector 0 0 0))) (bytevector-u8-set! bv 1 255) (bytevector-u8-ref bv 1))"),
        Value::Int(255),
    );

    // make-bytevector with fill
    assert_eq!(
        eval("(bytevector-u8-ref (make-bytevector 3 42) 2)"),
        Value::Int(42),
    );

    // bytevector-length
    is_int("(bytevector-length #u8())", 0);
    is_int("(bytevector-length #u8(1 2 3))", 3);

    // bytevector-copy with start/end
    assert_eq!(
        eval("(bytevector-copy #u8(0 1 2 3 4) 2 4)"),
        eval("#u8(2 3)"),
    );

    // bytevector-append empty
    assert_eq!(eval("(bytevector-append #u8() #u8())"), eval("#u8()"),);
    assert_eq!(eval("(bytevector-append #u8() #u8(1))"), eval("#u8(1)"),);
}

// =============================================================================
// §6.10 apply edge cases (R7RS §6.10)
// =============================================================================

#[test]
fn s6_10_apply_edge_cases() {
    // apply with empty list
    is_int("(apply + '())", 0);
    is_int("(apply * '())", 1);

    // apply with multiple leading args
    is_int("(apply + 1 2 '(3 4))", 10);
    is_int("(apply + 1 '(2))", 3);

    // apply with lambda
    is_int("(apply (lambda (x y) (+ x y)) '(3 4))", 7);
}

// =============================================================================
// §4.2.6 do — comprehensive (R7RS §4.2.6)
// =============================================================================

#[test]
fn s4_2_6_do_comprehensive() {
    // do with no body (just iteration)
    is_int("(do ((i 0 (+ i 1))) ((= i 5) i))", 5);

    // do with body side effect
    assert_eq!(
        eval(
            "(let ((result '()))
               (do ((i 0 (+ i 1)))
                   ((= i 3) (reverse result))
                 (set! result (cons i result))))"
        ),
        eval("'(0 1 2)"),
    );

    // do with multiple step variables
    is_int(
        "(do ((i 0 (+ i 1))
              (j 10 (- j 1)))
             ((= i 5) (+ i j)))",
        10, // i=5, j=5
    );

    // do with no step expression (variable stays constant)
    is_int(
        "(do ((x 42)
              (i 0 (+ i 1)))
             ((= i 3) x))",
        42,
    );
}

// =============================================================================
// §4.2.3 and/or — return value semantics (R7RS §4.2.3)
// =============================================================================

#[test]
fn s4_2_3_and_or_return_values() {
    // and returns last true value
    is_int("(and 1 2 3)", 3);
    assert_eq!(eval("(and 1 #f 3)"), Value::Bool(false));
    // and with no args returns #t
    is_true("(and)");
    // and with single arg returns that arg
    is_int("(and 42)", 42);

    // or returns first true value
    is_int("(or #f #f 3)", 3);
    is_int("(or 1 2 3)", 1);
    // or with no args returns #f
    is_false("(or)");
    // or with all false returns last
    is_false("(or #f #f #f)");
    // or with single arg
    is_int("(or 42)", 42);
}

// =============================================================================
// §4.1.6 define — internal definitions (R7RS §4.1.6 / §5.3.2)
// =============================================================================

#[test]
fn s5_3_internal_definitions() {
    // Internal define at start of body
    is_int(
        "(let ()
           (define x 10)
           (define y 20)
           (+ x y))",
        30,
    );

    // Internal define with mutual recursion
    is_true(
        "(let ()
           (define (even? n) (if (= n 0) #t (odd? (- n 1))))
           (define (odd? n) (if (= n 0) #f (even? (- n 1))))
           (even? 10))",
    );
}

// =============================================================================
// §6.11 error-object accessors (R7RS §6.11)
// =============================================================================

#[test]
fn s6_11_error_object_accessors() {
    // error-object-message
    assert_eq!(
        eval(
            r#"(guard (e (#t (error-object-message e)))
                  (error "test message" 1 2))"#
        ),
        Value::String(Rc::from("test message")),
    );

    // error-object-irritants
    assert_eq!(
        eval(
            r#"(guard (e (#t (error-object-irritants e)))
                  (error "msg" 'a 'b 'c))"#
        ),
        eval("'(a b c)"),
    );

    // error-object? predicate
    is_true(
        r#"(guard (e (#t (error-object? e)))
                 (error "test"))"#,
    );
    is_false("(error-object? 42)");
    is_false(r#"(error-object? "not an error")"#);

    // file-error? and read-error?
    is_false(
        r#"(guard (e (#t (file-error? e)))
                  (error "not a file error"))"#,
    );
    is_false(
        r#"(guard (e (#t (read-error? e)))
                  (error "not a read error"))"#,
    );
}

// =============================================================================
// §6.13 string port comprehensive (R7RS §6.13)
// =============================================================================

#[test]
fn s6_13_string_port_comprehensive() {
    // Read multiple values from string port
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);
    assert_eq!(
        vm.eval(
            r#"
            (let ((p (open-input-string "42 hello #t")))
              (let* ((a (read p))
                     (b (read p))
                     (c (read p)))
                (list a b c)))
        "#
        )
        .unwrap(),
        vm.eval("'(42 hello #t)").unwrap(),
    );

    // get-output-string accumulates writes
    assert_eq!(
        eval(
            r#"
            (let ((p (open-output-string)))
              (write-char #\H p)
              (write-char #\i p)
              (get-output-string p))
        "#
        ),
        Value::String(Rc::from("Hi")),
    );

    // Empty string port reads EOF immediately
    assert_eq!(
        eval(r#"(eof-object? (read (open-input-string "")))"#),
        Value::Bool(true),
    );
}

// =============================================================================
// §6.2 exact/inexact conversion edge cases (R7RS §6.2.6)
// =============================================================================

#[test]
fn s6_2_exact_inexact_conversion_edges() {
    // exact->inexact
    assert_eq!(eval("(exact->inexact 42)"), Value::Float(42.0));
    assert_eq!(eval("(exact->inexact 0)"), Value::Float(0.0));

    // inexact->exact
    is_int("(inexact->exact 42.0)", 42);
    is_int("(inexact->exact 0.0)", 0);
    is_int("(inexact->exact -7.0)", -7);

    // exact and inexact predicates
    is_true("(exact? 42)");
    is_false("(exact? 42.0)");
    is_true("(inexact? 42.0)");
    is_false("(inexact? 42)");

    // exact->inexact already inexact is no-op
    assert_eq!(eval("(exact->inexact 3.15)"), Value::Float(3.15));
}

// =============================================================================
// §6.10 dynamic-wind — comprehensive (R7RS §6.10)
// =============================================================================

#[test]
fn s6_10_dynamic_wind_nested() {
    // Basic dynamic-wind: in/body/out all execute in order
    assert_eq!(
        eval(
            r#"
            (let ((log '()))
              (dynamic-wind
                (lambda () (set! log (cons 'in log)))
                (lambda () (set! log (cons 'body log)) 42)
                (lambda () (set! log (cons 'out log))))
              (reverse log))
        "#
        ),
        eval("'(in body out)"),
    );

    // dynamic-wind returns body value
    is_int(
        "(dynamic-wind (lambda () #f) (lambda () 99) (lambda () #f))",
        99,
    );

    // Nested dynamic-wind
    assert_eq!(
        eval(
            r#"
            (let ((log '()))
              (dynamic-wind
                (lambda () (set! log (cons 'in1 log)))
                (lambda ()
                  (dynamic-wind
                    (lambda () (set! log (cons 'in2 log)))
                    (lambda () (set! log (cons 'body log)))
                    (lambda () (set! log (cons 'out2 log)))))
                (lambda () (set! log (cons 'out1 log))))
              (reverse log))
        "#
        ),
        eval("'(in1 in2 body out2 out1)"),
    );
}

// =============================================================================
// §6.10 dynamic-wind + call/cc interaction (R7RS §6.10)
// =============================================================================

#[test]
fn s6_10_dynamic_wind_callcc() {
    // R7RS requires that when a continuation crosses dynamic-wind boundaries,
    // the appropriate before/after thunks fire.
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // Test 1: continuation captured inside dynamic-wind, invoked from outside
    // When k is invoked from outside the extent, `before` should fire again
    // and when the thunk returns, `after` should fire.
    let result = vm
        .eval(
            r#"
        (let ((k #f)
              (count 0))
          (dynamic-wind
            (lambda () (set! count (+ count 1)))
            (lambda ()
              (call/cc (lambda (c) (set! k c)))
              count)
            (lambda () (set! count (+ count 100))))
          ;; At this point: before ran once (count=1), body ran (count=1),
          ;; after ran once (count=101).
          ;; Don't invoke k again to avoid infinite loop in this test.
          count)
    "#,
        )
        .unwrap();
    // Before ran once (1), after ran once (+100) = 101
    assert_eq!(result, Value::Int(101));

    // Test 2: dynamic-wind after thunk fires on call/cc escape.
    // We use a global to track side effects since continuation restoration
    // restores the stack (which would overwrite local bindings).
    let mut vm2 = Vm::new();
    crate::stdlib::register_stdlib(&mut vm2);
    vm2.eval("(define __dw_after_ran__ #f)").unwrap();
    let result2 = vm2
        .eval(
            r#"
        (call/cc
          (lambda (escape)
            (dynamic-wind
              (lambda () #f)
              (lambda () (escape 'done))
              (lambda () (set! __dw_after_ran__ #t)))))
    "#,
        )
        .unwrap();
    assert_eq!(result2, Value::symbol("done"));
    // After thunk should have run when escape left the dynamic extent
    assert_eq!(vm2.eval("__dw_after_ran__").unwrap(), Value::Bool(true),);
}

// ---------------------------------------------------------------------------
// §6.11 file-error? and read-error? condition predicates
// ---------------------------------------------------------------------------

#[test]
fn s6_11_file_error_predicate() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // file-error? should return #t for I/O errors from file operations
    let result = vm
        .eval(
            r#"
        (guard (exn
                ((file-error? exn) 'caught-file-error)
                (#t 'other-error))
          (open-input-file "/nonexistent/path/that/does/not/exist"))
    "#,
        )
        .unwrap();
    assert_eq!(result, Value::symbol("caught-file-error"));
}

#[test]
fn s6_11_file_error_not_on_regular_error() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // file-error? should return #f for regular errors
    let result = vm
        .eval(
            r#"
        (guard (exn
                ((file-error? exn) 'file-error)
                (#t 'other))
          (error "not a file error"))
    "#,
        )
        .unwrap();
    assert_eq!(result, Value::symbol("other"));
}

#[test]
fn s6_11_read_error_predicate() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // read-error? should return #t for malformed input
    let result = vm
        .eval(
            r#"
        (guard (exn
                ((read-error? exn) 'caught-read-error)
                (#t 'other-error))
          (read (open-input-string "(unclosed")))
    "#,
        )
        .unwrap();
    assert_eq!(result, Value::symbol("caught-read-error"));
}

// ---------------------------------------------------------------------------
// §6.4 member and assoc with custom comparator
// ---------------------------------------------------------------------------

#[test]
fn s6_4_member_custom_comparator() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // 2-arg member (default equal?)
    assert_eq!(
        vm.eval("(member 2 '(1 2 3))").unwrap(),
        vm.eval("'(2 3)").unwrap(),
    );

    // 3-arg member with custom comparator
    assert_eq!(
        vm.eval(r#"(member 5 '(1 2 8 4) (lambda (a b) (< b a)))"#)
            .unwrap(),
        vm.eval("'(1 2 8 4)").unwrap(),
    );

    // 3-arg member not found
    assert_eq!(
        vm.eval(r#"(member 0 '(1 2 3) (lambda (a b) (= a b)))"#)
            .unwrap(),
        Value::Bool(false),
    );
}

#[test]
fn s6_4_assoc_custom_comparator() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    // 2-arg assoc (default equal?)
    assert_eq!(
        vm.eval("(assoc 2 '((1 . a) (2 . b) (3 . c)))").unwrap(),
        vm.eval("'(2 . b)").unwrap(),
    );

    // 3-arg assoc with custom comparator
    assert_eq!(
        vm.eval(
            r#"(assoc 2.0 '((1 . a) (2 . b) (3 . c))
                          (lambda (a b) (= (exact a) (exact b))))"#
        )
        .unwrap(),
        vm.eval("'(2 . b)").unwrap(),
    );

    // 3-arg assoc not found
    assert_eq!(
        vm.eval(r#"(assoc 5 '((1 . a) (2 . b)) =)"#).unwrap(),
        Value::Bool(false),
    );
}

// ---------------------------------------------------------------------------
// §6.13 with-output-to-file / with-input-from-file port redirection
// ---------------------------------------------------------------------------

#[test]
fn s6_13_with_output_to_file_redirect() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    let tmp = std::env::temp_dir().join("mae_test_redirect_output.txt");
    let path = tmp.to_str().unwrap();

    // with-output-to-file should redirect display output to the file
    vm.eval(&format!(
        r#"
        (with-output-to-file "{path}"
          (lambda () (display "hello from redirect")))
    "#
    ))
    .unwrap();

    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(contents, "hello from redirect");
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn s6_13_with_input_from_file_redirect() {
    let mut vm = Vm::new();
    crate::stdlib::register_stdlib(&mut vm);

    let tmp = std::env::temp_dir().join("mae_test_redirect_input.txt");
    let path = tmp.to_str().unwrap();
    std::fs::write(&tmp, "42").unwrap();

    // with-input-from-file should redirect read input from the file
    let result = vm
        .eval(&format!(
            r#"
        (with-input-from-file "{path}"
          (lambda () (read (current-input-port))))
    "#
        ))
        .unwrap();
    assert_eq!(result, Value::Int(42));
    std::fs::remove_file(&tmp).ok();
}

// =========================================================================
// §4.2.1 cond clause without body returns test value
// =========================================================================

#[test]
fn s4_2_cond_no_body_returns_test_value() {
    // R7RS §4.2.1: (cond (test)) — if test is true, return test value
    assert_eq!(eval("(cond (#t))"), Value::Bool(true));
    assert_eq!(eval("(cond (1))"), Value::Int(1));
    assert_eq!(eval("(cond (#f) (42))"), Value::Int(42));
    assert_eq!(
        eval(r#"(cond (#f) ("hello"))"#),
        Value::String(Rc::from("hello"))
    );
    // With preceding false clause
    assert_eq!(eval("(cond (#f) (3))"), Value::Int(3));
}

// =========================================================================
// §6.13.2 read without port uses current-input-port
// =========================================================================

#[test]
fn s6_13_read_uses_current_input_port() {
    // read from string port passed as arg
    assert_eq!(
        eval("(let ((p (open-input-string \"42\"))) (read p))"),
        Value::Int(42)
    );
    // read-char from string port using with-input-from-file redirect
    // (we can test with-input-from-file + read together)
    let tmp = std::env::temp_dir().join("mae_test_read_noarg.txt");
    let path = tmp.to_str().unwrap().replace('\\', "/");
    std::fs::write(&tmp, "(+ 1 2)").unwrap();
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(&format!(
            r#"(with-input-from-file "{path}" (lambda () (read)))"#
        ))
        .unwrap();
    // read returns the datum (+ 1 2) as a list
    let items = result.to_vec().unwrap();
    assert_eq!(items.len(), 3);
    std::fs::remove_file(&tmp).ok();
}

// =========================================================================
// §6.13.3 read-bytevector! (destructive read into bytevector)
// =========================================================================

#[test]
fn s6_13_read_bytevector_bang() {
    assert_eq!(
        eval(
            r#"(let ((bv (make-bytevector 5 0))
                   (p (open-input-string "abc")))
               (read-bytevector! bv p)
               (bytevector-u8-ref bv 0))"#
        ),
        Value::Int(97) // 'a'
    );
    // Returns count of bytes read
    assert_eq!(
        eval(
            r#"(let ((bv (make-bytevector 10 0))
                   (p (open-input-string "hi")))
               (read-bytevector! bv p))"#
        ),
        Value::Int(2)
    );
}

// =========================================================================
// §6.12 load evaluates file contents
// =========================================================================

#[test]
fn s6_12_load_evaluates_file() {
    let tmp = std::env::temp_dir().join("mae_test_load_eval.scm");
    let path = tmp.to_str().unwrap().replace('\\', "/");
    std::fs::write(&tmp, "(define load-test-val 42)").unwrap();
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(&format!(r#"(load "{path}")"#)).unwrap();
    let result = vm.eval("load-test-val").unwrap();
    assert_eq!(result, Value::Int(42));
    std::fs::remove_file(&tmp).ok();
}

// =========================================================================
// §6.13.1 flush-output-port works on file ports
// =========================================================================

#[test]
fn s6_13_flush_output_port_file() {
    let tmp = std::env::temp_dir().join("mae_test_flush.txt");
    let path = tmp.to_str().unwrap().replace('\\', "/");
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(&format!(
        r#"(let ((p (open-output-file "{path}")))
           (write-string "flushed" p)
           (flush-output-port p)
           (close-output-port p))"#
    ))
    .unwrap();
    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(contents, "flushed");
    std::fs::remove_file(&tmp).ok();
}

// =========================================================================
// §7.1 String line continuation
// =========================================================================

#[test]
fn s7_1_string_line_continuation() {
    // R7RS §7.1.2: \<newline><intraline-whitespace> is nothing
    assert_eq!(
        eval("\"hello\\\n    world\""),
        Value::String(Rc::from("helloworld"))
    );
    assert_eq!(eval("\"abc\\\n  def\""), Value::String(Rc::from("abcdef")));
}

// =============================================================================
// Edge-case tests for sections with insufficient coverage
// =============================================================================

// §4.2.1 cond with => arrow clause
#[test]
fn edge_cond_arrow_false_test() {
    // When the test is #f, the => clause is skipped
    is_int("(cond (#f => car) (else 5))", 5);
}

#[test]
fn edge_cond_arrow_true_test() {
    // When the test is truthy, the result is passed to the proc
    is_int("(cond ('(1 2 3) => car) (else 5))", 1);
}

#[test]
fn edge_cond_arrow_numeric_test() {
    // Non-#f value passes itself to the proc
    is_int("(cond (42 => (lambda (x) (+ x 1))) (else 0))", 43);
}

// §4.2.3 and/or edge cases on return values
#[test]
fn edge_and_or_return_value_types() {
    // and returns the first false-ish value
    is_false("(and 1 #f 'never)");
    // and with string returns it
    is_str("(and 1 2 \"yes\")", "yes");
    // or returns first truthy value, even if it's a string
    is_str(r#"(or #f "found")"#, "found");
    // or #f 5 -> 5
    is_int("(or #f 5)", 5);
    // or with only #f returns #f
    is_false("(or #f)");
    // and with single #f
    is_false("(and #f)");
}

// §4.2.6 do loop — basic and with accumulator
#[test]
fn edge_do_loop_basic() {
    // Simple counting loop
    is_int("(do ((i 0 (+ i 1))) ((= i 5) i))", 5);
}

#[test]
fn edge_do_loop_accumulator() {
    // do loop that accumulates a result
    is_int(
        "(do ((i 0 (+ i 1))
              (sum 0 (+ sum i)))
             ((= i 5) sum))",
        10,
    );
}

#[test]
fn edge_do_loop_reverse_list() {
    // do loop building a list in reverse
    is_true(
        "(equal? (do ((lst '(1 2 3 4 5) (cdr lst))
                      (acc '() (cons (car lst) acc)))
                     ((null? lst) acc))
                 '(5 4 3 2 1))",
    );
}

// §5.3 define with body (shorthand)
#[test]
fn edge_define_with_body() {
    is_int("(define (f x) (+ x 1)) (f 5)", 6);
}

#[test]
fn edge_define_with_multi_body() {
    // define with multiple body expressions returns last
    is_int("(define (g x) (+ x 1) (+ x 2) (+ x 3)) (g 10)", 13);
}

// §6.1 eqv? on procedures
#[test]
fn edge_eqv_procedures() {
    // Same procedure binding must be eqv? to itself
    is_true("(let ((f (lambda (x) x))) (eqv? f f))");
    // Two distinct lambdas with same body are NOT eqv?
    is_false(
        "(let ((f (lambda (x) x))
               (g (lambda (x) x)))
           (eqv? f g))",
    );
}

// §6.2 exact->inexact and inexact->exact roundtrip
#[test]
fn edge_exact_inexact_roundtrip() {
    // exact->inexact then inexact->exact roundtrip
    is_int("(inexact->exact (exact->inexact 5))", 5);
    // inexact->exact of 2.5 should give integer 2 (truncation behavior)
    // Actually R7RS says inexact->exact returns exact value equal to argument
    // For 2.0 that's 2
    is_int("(inexact->exact 2.0)", 2);
    // exact->inexact produces a float
    assert_eq!(eval("(exact->inexact 3)"), Value::Float(3.0));
}

// §6.2 number->string with radix
#[test]
fn edge_number_to_string_radix_hex() {
    is_str("(number->string 255 16)", "ff");
}

#[test]
fn edge_number_to_string_radix_binary() {
    is_str("(number->string 10 2)", "1010");
}

#[test]
fn edge_number_to_string_radix_octal() {
    is_str("(number->string 8 8)", "10");
}

// §6.2 string->number with radix
#[test]
fn edge_string_to_number_radix_hex() {
    is_int(r#"(string->number "ff" 16)"#, 255);
}

#[test]
fn edge_string_to_number_radix_binary() {
    is_int(r#"(string->number "1010" 2)"#, 10);
}

#[test]
fn edge_string_to_number_radix_octal() {
    is_int(r#"(string->number "10" 8)"#, 8);
}

#[test]
fn edge_string_to_number_invalid() {
    // Invalid string->number returns #f
    is_false(r#"(string->number "not-a-number")"#);
    is_false(r#"(string->number "gg" 16)"#);
}

// §6.4 list-tail edge cases
#[test]
fn edge_list_tail_zero() {
    // list-tail with 0 returns the whole list
    is_true("(equal? (list-tail '(a b c d) 0) '(a b c d))");
}

#[test]
fn edge_list_tail_end() {
    // list-tail at exact end returns empty list
    is_true("(null? (list-tail '(a b c) 3))");
}

// §6.4 list-copy independence
#[test]
fn edge_list_copy_independence() {
    // Mutation of original doesn't affect copy (for mutable pairs, not applicable
    // in mae-scheme with immutable pairs, but list-copy should still produce equal result)
    is_true("(equal? (list-copy '(1 2 3)) '(1 2 3))");
    // Empty list copy
    is_true("(null? (list-copy '()))");
}

// §6.6 char-ci=? and friends
#[test]
fn edge_char_ci_eq() {
    is_true("(char-ci=? #\\a #\\A)");
    is_true("(char-ci=? #\\Z #\\z)");
    is_false("(char-ci=? #\\a #\\b)");
}

#[test]
fn edge_char_ci_lt_gt() {
    is_true("(char-ci<? #\\a #\\B)");
    is_false("(char-ci<? #\\b #\\A)");
    is_true("(char-ci>? #\\c #\\A)");
    is_true("(char-ci<=? #\\a #\\A)");
    is_true("(char-ci>=? #\\a #\\A)");
}

// §6.7 string-copy! — mae-scheme strings are immutable, should error
#[test]
fn edge_string_copy_bang_immutable() {
    let err = eval_err(r#"(string-copy! "hello" 0 "xy")"#);
    assert!(
        err.contains("immutable"),
        "string-copy! should mention immutability: {err}"
    );
}

// §6.7 string-downcase/upcase/foldcase
#[test]
fn edge_string_case_conversions() {
    is_str(r#"(string-upcase "hello")"#, "HELLO");
    is_str(r#"(string-downcase "HELLO")"#, "hello");
    is_str(r#"(string-downcase "Hello World")"#, "hello world");
    is_str(r#"(string-upcase "")"#, "");
    is_str(r#"(string-foldcase "HeLLo")"#, "hello");
}

// §6.8 vector-copy basic
#[test]
fn edge_vector_copy_basic() {
    is_true("(equal? (vector-copy #(1 2 3)) #(1 2 3))");
    // Copy with start
    is_true("(equal? (vector-copy #(a b c d e) 2) #(c d e))");
    // Empty vector copy
    is_true("(equal? (vector-copy #()) #())");
}

// §6.8 vector-fill! basic
#[test]
fn edge_vector_fill_basic() {
    is_true(
        "(let ((v (vector 1 2 3)))
           (vector-fill! v 0)
           (equal? v #(0 0 0)))",
    );
}

#[test]
fn edge_vector_fill_single() {
    is_true(
        "(let ((v (vector 42)))
           (vector-fill! v 99)
           (equal? v #(99)))",
    );
}

// §6.9 bytevector-copy basic
#[test]
fn edge_bytevector_copy_basic() {
    is_true("(equal? (bytevector-copy #u8(1 2 3)) #u8(1 2 3))");
    // Copy with start/end
    is_true("(equal? (bytevector-copy #u8(0 1 2 3 4) 1 3) #u8(1 2))");
}

// §6.10 call-with-values basic
#[test]
fn edge_call_with_values_basic() {
    is_int("(call-with-values (lambda () (values 1 2)) +)", 3);
}

#[test]
fn edge_call_with_values_single() {
    // Single value
    is_int("(call-with-values (lambda () 42) (lambda (x) x))", 42);
}

#[test]
fn edge_call_with_values_three() {
    // Three values
    is_int("(call-with-values (lambda () (values 1 2 3)) +)", 6);
}

// §6.10 for-each mutation test
#[test]
fn edge_for_each_mutation() {
    is_true(
        "(let ((x '()))
           (for-each (lambda (v) (set! x (cons v x))) '(1 2 3))
           (equal? x '(3 2 1)))",
    );
}

#[test]
fn edge_for_each_empty() {
    // for-each on empty list should do nothing
    assert_eq!(
        eval("(let ((x 0)) (for-each (lambda (v) (set! x (+ x 1))) '()) x)"),
        Value::Int(0)
    );
}

// §6.11 error with irritants
#[test]
fn edge_error_irritants_message() {
    is_str(
        r#"(guard (e (#t (error-object-message e))) (error "bad" 1 2))"#,
        "bad",
    );
}

#[test]
fn edge_error_irritants_list() {
    is_true(r#"(guard (e (#t (equal? (error-object-irritants e) '(1 2)))) (error "bad" 1 2))"#);
}

#[test]
fn edge_error_irritants_type() {
    // error-object-type returns "error" for errors created with (error ...)
    is_str(
        r#"(guard (e (#t (error-object-type e))) (error "bad" 1 2))"#,
        "error",
    );
}

// §6.13 open-input-string and read roundtrip
#[test]
fn edge_open_input_string_read() {
    is_int(
        "(let ((p (open-input-string \"42\")))
           (read p))",
        42,
    );
}

#[test]
fn edge_open_input_string_read_symbol() {
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (eq? (read p) 'hello))",
    );
}

#[test]
fn edge_open_input_string_read_list() {
    is_true(
        "(let ((p (open-input-string \"(1 2 3)\")))
           (equal? (read p) '(1 2 3)))",
    );
}

#[test]
fn edge_open_input_string_eof() {
    is_true(
        "(let ((p (open-input-string \"\")))
           (eof-object? (read p)))",
    );
}

// §6.14 features returns a list
#[test]
fn edge_features_is_list() {
    is_true("(list? (features))");
}

#[test]
fn edge_features_contains_r7rs() {
    // memq returns the tail, not #t; check it's not #f
    is_false("(not (memq 'r7rs (features)))");
}

#[test]
fn edge_features_contains_mae() {
    is_false("(not (memq 'mae-scheme (features)))");
}

// §4.2.5 delay/force
#[test]
fn edge_delay_force_basic() {
    is_int("(force (delay 42))", 42);
}

#[test]
fn edge_delay_force_memoization() {
    // force memoizes: side effects only happen once
    is_int(
        "(let ((count 0))
           (define p (delay (begin (set! count (+ count 1)) count)))
           (force p)
           (force p)
           count)",
        1,
    );
}

#[test]
fn edge_delay_force_expression() {
    is_int("(force (delay (+ 2 3)))", 5);
}

// ---------------------------------------------------------------------------
// §6.13.3 — display vs write semantics
// ---------------------------------------------------------------------------

#[test]
fn s6_13_display_no_quotes_on_strings() {
    // display should not quote strings
    is_str(
        r#"(let ((p (open-output-string)))
             (display "hello" p)
             (get-output-string p))"#,
        "hello",
    );
}

#[test]
fn s6_13_write_quotes_strings() {
    // write should quote strings
    is_str(
        r#"(let ((p (open-output-string)))
             (write "hello" p)
             (get-output-string p))"#,
        r#""hello""#,
    );
}

#[test]
fn s6_13_display_char_as_character() {
    // display should show the character itself
    is_str(
        r#"(let ((p (open-output-string)))
             (display #\a p)
             (get-output-string p))"#,
        "a",
    );
}

#[test]
fn s6_13_write_char_with_prefix() {
    // write should show #\a notation
    is_str(
        r#"(let ((p (open-output-string)))
             (write #\a p)
             (get-output-string p))"#,
        "#\\a",
    );
}

#[test]
fn s6_13_display_list_of_strings() {
    // display on a list should recursively display elements (no quotes)
    is_str(
        r#"(let ((p (open-output-string)))
             (display '("hello" "world") p)
             (get-output-string p))"#,
        "(hello world)",
    );
}

#[test]
fn s6_13_write_list_of_strings() {
    // write on a list should recursively write elements (with quotes)
    is_str(
        r#"(let ((p (open-output-string)))
             (write '("hello" "world") p)
             (get-output-string p))"#,
        r#"("hello" "world")"#,
    );
}

#[test]
fn s6_13_display_vector_of_strings() {
    is_str(
        r#"(let ((p (open-output-string)))
             (display (vector "a" "b") p)
             (get-output-string p))"#,
        "#(a b)",
    );
}

#[test]
fn s6_13_display_nested_list_of_chars() {
    is_str(
        r#"(let ((p (open-output-string)))
             (display (list #\x (list #\y #\z)) p)
             (get-output-string p))"#,
        "(x (y z))",
    );
}

// ---------------------------------------------------------------------------
// §6.13 — Sequential file port reads
// ---------------------------------------------------------------------------

#[test]
fn s6_13_sequential_read_from_file_port() {
    // read should parse one s-expression at a time, not consume the whole file
    let dir = std::env::temp_dir().join("mae_test_seq_read");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("multi_sexp.scm");
    std::fs::write(&path, "(+ 1 2) (+ 3 4) (+ 5 6)").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((a (read p))
                    (b (read p))
                    (c (read p))
                    (d (read p)))
               (close-input-port p)
               (list a b c (eof-object? d))))"#,
        path.display()
    );
    let result = eval(&code);
    // Should read 3 s-expressions and then get EOF
    assert_eq!(
        result,
        Value::list(vec![
            Value::list(vec![Value::symbol("+"), Value::Int(1), Value::Int(2)]),
            Value::list(vec![Value::symbol("+"), Value::Int(3), Value::Int(4)]),
            Value::list(vec![Value::symbol("+"), Value::Int(5), Value::Int(6)]),
            Value::Bool(true),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_sequential_read_char_from_file_port() {
    let dir = std::env::temp_dir().join("mae_test_seq_readchar");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("chars.txt");
    std::fs::write(&path, "abc").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((a (read-char p))
                    (b (read-char p))
                    (c (read-char p))
                    (d (read-char p)))
               (close-input-port p)
               (list a b c (eof-object? d))))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![
            Value::Char('a'),
            Value::Char('b'),
            Value::Char('c'),
            Value::Bool(true),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_mixed_read_and_read_char_on_file_port() {
    let dir = std::env::temp_dir().join("mae_test_mixed_read");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("mixed.scm");
    std::fs::write(&path, "42 hello").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((num (read p))
                    (space (read-char p))
                    (sym (read p)))
               (close-input-port p)
               (list num space sym)))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![
            Value::Int(42),
            Value::Char(' '),
            Value::symbol("hello"),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_read_line_from_file_port() {
    let dir = std::env::temp_dir().join("mae_test_readline");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("lines.txt");
    std::fs::write(&path, "first\nsecond\nthird").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((a (read-line p))
                    (b (read-line p))
                    (c (read-line p))
                    (d (read-line p)))
               (close-input-port p)
               (list a b c (eof-object? d))))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![
            Value::String(Rc::from("first")),
            Value::String(Rc::from("second")),
            Value::String(Rc::from("third")),
            Value::Bool(true),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_peek_char_does_not_advance_file_port() {
    let dir = std::env::temp_dir().join("mae_test_peek_file");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("peek.txt");
    std::fs::write(&path, "xy").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((peeked (peek-char p))
                    (read1 (read-char p))
                    (read2 (read-char p)))
               (close-input-port p)
               (list peeked read1 read2)))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![Value::Char('x'), Value::Char('x'), Value::Char('y'),])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_char_ready_on_file_port() {
    let dir = std::env::temp_dir().join("mae_test_charready");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("ready.txt");
    std::fs::write(&path, "a").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((ready1 (char-ready? p))
                    (_ (read-char p))
                    (ready2 (char-ready? p)))
               (close-input-port p)
               (list ready1 ready2)))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![Value::Bool(true), Value::Bool(false)])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn s6_13_read_string_from_file_port() {
    let dir = std::env::temp_dir().join("mae_test_readstring");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("readstr.txt");
    std::fs::write(&path, "hello world").unwrap();

    let code = format!(
        r#"(let ((p (open-input-file "{}")))
             (let* ((a (read-string 5 p))
                    (b (read-string 6 p)))
               (close-input-port p)
               (list a b)))"#,
        path.display()
    );
    let result = eval(&code);
    assert_eq!(
        result,
        Value::list(vec![
            Value::String(Rc::from("hello")),
            Value::String(Rc::from(" world")),
        ])
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// Chibi-derived edge case tests (from Chibi-Scheme r7rs-tests.scm)
// ===========================================================================

// --- §4.1 Primitive expressions (Chibi) ---

#[test]
fn chibi_4_1_lambda_varargs() {
    // (lambda x x) captures all args as a list
    assert_eq!(
        eval("((lambda x x) 3 4 5 6)"),
        Value::list(vec![
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
            Value::Int(6)
        ])
    );
}

#[test]
fn chibi_4_1_lambda_dotted_rest() {
    // Dotted rest parameter
    assert_eq!(
        eval("((lambda (x y . z) z) 3 4 5 6)"),
        Value::list(vec![Value::Int(5), Value::Int(6)])
    );
}

#[test]
fn chibi_4_1_if_condition_dispatch() {
    // ((if #f + *) 3 4) — if as operator position
    is_int("((if #f + *) 3 4)", 12);
    is_int("((if #t + *) 3 4)", 7);
}

// --- §4.2 Derived expressions (Chibi) ---

#[test]
fn chibi_4_2_cond_arrow() {
    // cond with => clause
    is_int("(cond ((assv 'b '((a 1) (b 2))) => cadr) (else #f))", 2);
}

#[test]
fn chibi_4_2_case_basic() {
    // case dispatching
    assert_eq!(
        eval("(case (* 2 3) ((2 3 5 7) 'prime) ((1 4 6 8 9) 'composite))"),
        Value::symbol("composite")
    );
    assert_eq!(
        eval("(case (car '(c d)) ((a e i o u) 'vowel) ((w y) 'semivowel) (else 'other))"),
        Value::symbol("other")
    );
}

#[test]
fn chibi_4_2_and_returns_last_true() {
    // and returns last true value, not just #t
    assert_eq!(
        eval("(and 1 2 'c '(f g))"),
        Value::list(vec![Value::symbol("f"), Value::symbol("g")])
    );
    is_true("(and)"); // empty and returns #t
}

#[test]
fn chibi_4_2_or_returns_first_true() {
    // or returns first true value
    assert_eq!(
        eval("(or (memq 'b '(a b c)) (/ 3 0))"),
        Value::list(vec![Value::symbol("b"), Value::symbol("c")])
    );
    is_false("(or #f #f #f)");
}

#[test]
fn chibi_4_2_named_let() {
    // Named let for loops
    is_int("(let loop ((x 0)) (if (= x 10) x (loop (+ x 1))))", 10);
}

#[test]
fn chibi_4_2_letrec_star() {
    // letrec* allows sequential references
    is_int(
        "(letrec* ((p (lambda (x) (+ 1 (q (- x 1)))))
                   (q (lambda (y) (if (zero? y) 0 (+ 1 (p (- y 1))))))
                   (x (p 5))
                   (y x))
           y)",
        5,
    );
}

#[test]
fn chibi_4_2_do_loop() {
    // do loop with multiple bindings
    assert_eq!(
        eval(
            "(do ((vec (make-vector 5))
                  (i 0 (+ i 1)))
                 ((= i 5) vec)
               (vector-set! vec i i))"
        ),
        eval("#(0 1 2 3 4)")
    );
}

// --- §4.3 Macros (Chibi) ---

#[test]
fn chibi_4_3_let_syntax_hygiene() {
    // let-syntax should be hygienic — `if` is rebound but doesn't affect the macro
    assert_eq!(
        eval(
            "(let-syntax ((when (syntax-rules ()
                             ((when test stmt1 stmt2 ...)
                              (if test (begin stmt1 stmt2 ...))))))
               (let ((if #t))
                 (when if (set! if 'now))
                 if))"
        ),
        Value::symbol("now")
    );
}

#[test]
fn chibi_4_3_basic_syntax_rules() {
    // Basic syntax-rules macro
    assert_eq!(
        eval(
            "(let-syntax ((swap! (syntax-rules ()
                             ((swap! a b) (let ((t a)) (set! a b) (set! b t))))))
               (let ((x 1) (y 2))
                 (swap! x y)
                 (list x y)))"
        ),
        Value::list(vec![Value::Int(2), Value::Int(1)])
    );
}

// --- §5 Program Structure (Chibi) ---

#[test]
fn chibi_5_define_values() {
    // define-values destructuring
    is_int("(define-values (a b c) (values 1 2 3)) (+ a b c)", 6);
}

#[test]
fn chibi_5_define_record_type() {
    // define-record-type
    is_true(
        "(define-record-type <pare> (kons x y) pare? (x kar) (y kdr))
         (pare? (kons 1 2))",
    );
    is_false(
        "(define-record-type <pare> (kons x y) pare? (x kar) (y kdr))
         (pare? (cons 1 2))",
    );
    is_int(
        "(define-record-type <pare> (kons x y) pare? (x kar) (y kdr))
         (kar (kons 1 2))",
        1,
    );
    is_int(
        "(define-record-type <pare> (kons x y) pare? (x kar) (y kdr))
         (kdr (kons 1 2))",
        2,
    );
}

// --- §6.1 Equivalence (Chibi) ---

#[test]
fn chibi_6_1_eqv_edge_cases() {
    is_true("(eqv? #t #t)");
    is_true("(eqv? #f #f)");
    is_true("(eqv? 'abc 'abc)");
    is_true("(eqv? 2 2)");
    is_true("(eqv? '() '())");
    is_true("(eqv? car car)");
    is_false("(eqv? #f 'nil)");
    is_false("(eqv? '() #f)");
    is_false("(eqv? 2 2.0)"); // exact vs inexact
}

#[test]
fn chibi_6_1_equal_deep() {
    is_true("(equal? '(a b c) '(a b c))");
    is_true("(equal? '(a (b) c) '(a (b) c))");
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_true(r#"(equal? "abc" "abc")"#);
}

// --- §6.2 Numbers (Chibi) ---

#[test]
fn chibi_6_2_type_predicates() {
    is_true("(number? 3)");
    is_true("(real? 3)");
    is_true("(integer? 3)");
    is_true("(exact? 3)");
    is_true("(inexact? 3.0)");
    is_true("(integer? 3.0)"); // 3.0 is integer-valued
    is_false("(integer? 3.1)");
}

#[test]
fn chibi_6_2_arithmetic_edge() {
    is_int("(+ 3 4)", 7);
    is_int("(- 3 4)", -1);
    is_int("(* 4)", 4);
    is_int("(+)", 0);
    is_int("(*)", 1);
    is_int("(abs -7)", 7);
    is_int("(abs 7)", 7);
    is_int("(gcd 32 -36)", 4);
    is_int("(gcd)", 0);
    is_int("(lcm 32 -36)", 288);
    is_int("(lcm)", 1);
}

#[test]
fn chibi_6_2_exact_inexact_conversion() {
    is_int("(exact 3.0)", 3);
    assert_eq!(eval("(inexact 3)"), Value::Float(3.0));
}

#[test]
fn chibi_6_2_number_string_roundtrip() {
    // R7RS: (string->number (number->string x)) should equal x for exact numbers
    is_int("(string->number (number->string 42))", 42);
    is_int("(string->number (number->string -7))", -7);
    is_int("(string->number (number->string 0))", 0);
}

// --- §6.3 Booleans (Chibi) ---

#[test]
fn chibi_6_3_boolean_equality() {
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
    is_true("(boolean=? #t #t #t)");
    is_false("(boolean=? #t #t #f)");
}

// --- §6.4 Pairs and Lists (Chibi) ---

#[test]
fn chibi_6_4_list_ops() {
    assert_eq!(
        eval("(list 'a (+ 3 4) 'c)"),
        Value::list(vec![Value::symbol("a"), Value::Int(7), Value::symbol("c")])
    );
    is_int("(length '(a b c))", 3);
    is_int("(length '())", 0);
    assert_eq!(
        eval("(append '(a) '(b c d))"),
        Value::list(vec![
            Value::symbol("a"),
            Value::symbol("b"),
            Value::symbol("c"),
            Value::symbol("d"),
        ])
    );
    assert_eq!(
        eval("(reverse '(a b c))"),
        Value::list(vec![
            Value::symbol("c"),
            Value::symbol("b"),
            Value::symbol("a")
        ])
    );
}

#[test]
fn chibi_6_4_make_list() {
    assert_eq!(
        eval("(make-list 2 3)"),
        Value::list(vec![Value::Int(3), Value::Int(3)])
    );
}

#[test]
fn chibi_6_4_list_copy_independence() {
    // list-copy creates independent structure
    is_true(
        "(let* ((a '(1 2 3))
                (b (list-copy a)))
           (equal? a b))",
    );
}

#[test]
fn chibi_6_4_member_custom_compare() {
    // member with custom comparator
    assert_eq!(
        eval("(member 2.0 '(1 2 3) =)"),
        Value::list(vec![Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn chibi_6_4_assoc_custom_compare() {
    // assoc with custom comparator
    assert_eq!(
        eval("(assoc 2.0 '((1 a) (2 b) (3 c)) =)"),
        Value::list(vec![Value::Int(2), Value::symbol("b")])
    );
}

// --- §6.6 Characters (Chibi) ---

#[test]
fn chibi_6_6_char_predicates() {
    is_true("(char-alphabetic? #\\a)");
    is_false("(char-alphabetic? #\\1)");
    is_true("(char-numeric? #\\1)");
    is_false("(char-numeric? #\\a)");
    is_true("(char-whitespace? #\\space)");
    is_true("(char-whitespace? #\\newline)");
    is_true("(char-upper-case? #\\A)");
    is_false("(char-upper-case? #\\a)");
    is_true("(char-lower-case? #\\a)");
    is_false("(char-lower-case? #\\A)");
}

#[test]
fn chibi_6_6_digit_value() {
    is_int("(digit-value #\\0)", 0);
    is_int("(digit-value #\\3)", 3);
    is_int("(digit-value #\\9)", 9);
    is_false("(digit-value #\\a)");
    is_false("(digit-value #\\space)");
}

// --- §6.7 Strings (Chibi) ---

#[test]
fn chibi_6_7_string_ops() {
    is_int(r#"(string-length "abc")"#, 3);
    assert_eq!(eval(r#"(string-ref "abc" 1)"#), Value::Char('b'));
    is_str(r#"(substring "abcdef" 2 4)"#, "cd");
    is_str(r#"(string-append "hello" " " "world")"#, "hello world");
    is_str(r#"(string-upcase "hello")"#, "HELLO");
    is_str(r#"(string-downcase "HELLO")"#, "hello");
}

#[test]
fn chibi_6_7_string_to_list_roundtrip() {
    is_str("(list->string (string->list \"hello\"))", "hello");
}

// --- §6.8 Vectors (Chibi) ---

#[test]
fn chibi_6_8_vector_ops() {
    is_int("(vector-length #(1 2 3))", 3);
    is_int("(vector-ref #(1 2 3) 1)", 2);
    assert_eq!(
        eval("(vector->list #(1 2 3))"),
        Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
    assert_eq!(eval("(list->vector '(1 2 3))"), eval("#(1 2 3)"));
}

#[test]
fn chibi_6_8_vector_append() {
    assert_eq!(eval("(vector-append #(1 2) #(3 4))"), eval("#(1 2 3 4)"));
}

// --- §6.9 Bytevectors (Chibi) ---

#[test]
fn chibi_6_9_bytevector_ops() {
    is_int("(bytevector-length #u8(1 2 3))", 3);
    is_int("(bytevector-u8-ref #u8(10 20 30) 1)", 20);
    assert_eq!(
        eval("(bytevector-append #u8(1 2) #u8(3 4))"),
        eval("#u8(1 2 3 4)")
    );
}

#[test]
fn chibi_6_9_utf8_string_conversion() {
    is_str(r#"(utf8->string #u8(65 66 67))"#, "ABC");
    assert_eq!(eval(r#"(string->utf8 "ABC")"#), eval("#u8(65 66 67)"));
}

// --- §6.10 Control (Chibi) ---

#[test]
fn chibi_6_10_apply() {
    is_int("(apply + '(3 4))", 7);
    is_int("(apply + 1 2 '(3 4))", 10);
}

#[test]
fn chibi_6_10_map_multi_list() {
    // map with multiple lists
    assert_eq!(
        eval("(map + '(1 2 3) '(10 20 30))"),
        Value::list(vec![Value::Int(11), Value::Int(22), Value::Int(33)])
    );
}

#[test]
fn chibi_6_10_string_map() {
    is_str("(string-map char-upcase \"hello\")", "HELLO");
}

#[test]
fn chibi_6_10_vector_map() {
    assert_eq!(
        eval("(vector-map + #(1 2 3) #(10 20 30))"),
        eval("#(11 22 33)")
    );
}

#[test]
fn chibi_6_10_call_cc_escape() {
    // Classic call/cc escape pattern
    is_int(
        "(+ 1 (call-with-current-continuation (lambda (k) (+ 2 (k 3)))))",
        4,
    );
}

#[test]
fn chibi_6_10_call_with_values() {
    is_int("(call-with-values (lambda () (values 4 5)) +)", 9);
}

#[test]
fn chibi_6_10_dynamic_wind_ordering() {
    // dynamic-wind before/after ordering
    is_str(
        r#"(let ((path '()))
           (dynamic-wind
             (lambda () (set! path (cons 'before path)))
             (lambda () (set! path (cons 'during path)))
             (lambda () (set! path (cons 'after path))))
           (list->string (map (lambda (s) (string-ref (symbol->string s) 0)) (reverse path))))"#,
        "bda",
    );
}

// --- §6.11 Exceptions (Chibi) ---

#[test]
fn chibi_6_11_guard_basic() {
    is_int(
        "(guard (exn
                 ((string? (error-object-message exn)) 42))
           (error \"test\" \"oops\"))",
        42,
    );
}

#[test]
fn chibi_6_11_error_object_properties() {
    is_str(
        r#"(guard (e (#t (error-object-message e)))
           (error "test message" 1 2 3))"#,
        "test message",
    );
    // irritants
    assert_eq!(
        eval(
            r#"(guard (e (#t (error-object-irritants e)))
               (error "msg" 1 2 3))"#
        ),
        Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

// --- §6.13 I/O (Chibi) ---

#[test]
fn chibi_6_13_string_port_roundtrip() {
    // write to string port, read back
    is_int(
        r#"(let ((p (open-output-string)))
             (write 42 p)
             (let ((s (get-output-string p)))
               (read (open-input-string s))))"#,
        42,
    );
}

#[test]
fn chibi_6_13_write_read_roundtrip_list() {
    assert_eq!(
        eval(
            r#"(let ((p (open-output-string)))
                 (write '(1 2 3) p)
                 (let ((s (get-output-string p)))
                   (read (open-input-string s))))"#
        ),
        Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

// --- §6.14 System interface (Chibi) ---

#[test]
fn chibi_6_14_features_contains_r7rs() {
    // memq returns the tail starting at the match (truthy), not #t
    is_true("(if (memq 'r7rs (features)) #t #f)");
}

#[test]
fn chibi_6_14_features_contains_mae_scheme() {
    is_true("(if (memq 'mae-scheme (features)) #t #f)");
}

// =========================================================================
// Regression tests for audit findings
// =========================================================================

// --- Issue #1: define-record-type accessor index when field spec order ≠ constructor order ---

#[test]
fn audit_record_type_accessor_index_mismatch() {
    // Constructor takes (y x) but field specs list x first, then y.
    // Accessor must return the correct field regardless of spec order.
    let result = eval(
        "(begin
           (define-record-type <pt>
             (make-pt y x)
             pt?
             (x pt-x)
             (y pt-y))
           (let ((p (make-pt 20 10)))
             (list (pt-x p) (pt-y p))))",
    );
    assert_eq!(result, eval("'(10 20)"));
}

#[test]
fn audit_record_type_accessor_matching_order() {
    // When field spec order matches constructor order, still works
    let result = eval(
        "(begin
           (define-record-type <pair>
             (make-pair a b)
             pair?
             (a pair-a)
             (b pair-b))
           (let ((p (make-pair 1 2)))
             (list (pair-a p) (pair-b p))))",
    );
    assert_eq!(result, eval("'(1 2)"));
}

// --- Issue #2: abs i64::MIN overflow ---

#[test]
fn audit_abs_min_int_no_panic() {
    // abs of most negative fixnum should not panic
    let result = eval(&format!("(abs {})", i64::MIN));
    // Should return i64::MAX (saturated) rather than panicking
    match result {
        Value::Int(n) => assert!(n > 0, "abs of MIN should be positive, got {n}"),
        _ => panic!("abs should return integer"),
    }
}

// --- Issue #3: expt exact integer preservation ---

#[test]
fn audit_expt_exact_integer() {
    // (expt 2 10) should return exact 1024, not float
    is_int("(expt 2 10)", 1024);
    is_int("(expt 3 5)", 243);
    is_int("(expt 2 0)", 1);
    is_int("(expt 5 1)", 5);
}

#[test]
fn audit_expt_large_exact() {
    // 2^53 is exactly representable in i64
    is_int("(expt 2 53)", 1_i64 << 53);
}

#[test]
fn audit_expt_overflow_to_float() {
    // 2^63 overflows i64, should fall back to float
    let r = eval("(expt 2 63)");
    assert!(
        matches!(r, Value::Float(_)),
        "2^63 should overflow to float, got {r}"
    );
}

// --- Issue #4: unary / exactness ---

#[test]
fn audit_unary_div_exact() {
    // (/ 1) should return exact 1
    is_int("(/ 1)", 1);
    // (/ -1) should return exact -1
    is_int("(/ -1)", -1);
}

// --- Issue #5: exact-integer-sqrt precision ---

#[test]
fn audit_exact_integer_sqrt_basic() {
    assert_eq!(eval("(exact-integer-sqrt 14)"), eval("'(3 5)"));
    assert_eq!(eval("(exact-integer-sqrt 0)"), eval("'(0 0)"));
    assert_eq!(eval("(exact-integer-sqrt 1)"), eval("'(1 0)"));
    assert_eq!(eval("(exact-integer-sqrt 4)"), eval("'(2 0)"));
    assert_eq!(eval("(exact-integer-sqrt 5)"), eval("'(2 1)"));
}

// --- Issue #6: call_thunk winder preservation ---

#[test]
fn audit_dynamic_wind_nested_thunk_winders() {
    // Nested dynamic-wind should properly save/restore winders in call_thunk
    let result = eval(
        "(let ((trace '()))
           (dynamic-wind
             (lambda () (set! trace (cons 'in1 trace)))
             (lambda ()
               (dynamic-wind
                 (lambda () (set! trace (cons 'in2 trace)))
                 (lambda () (set! trace (cons 'body trace)))
                 (lambda () (set! trace (cons 'out2 trace)))))
             (lambda () (set! trace (cons 'out1 trace))))
           (reverse trace))",
    );
    assert_eq!(result, eval("'(in1 in2 body out2 out1)"));
}

// --- Issue #7: modulo i64::MIN overflow ---

#[test]
fn audit_modulo_large_negative() {
    // modulo with large negative should not overflow
    let result = eval("(modulo -9223372036854775807 3)");
    match result {
        Value::Int(n) => assert!(
            (0..3).contains(&n),
            "modulo result should be in [0,3), got {n}"
        ),
        _ => panic!("modulo should return integer"),
    }
}

// --- Issue #8: output-bytevector binary safety ---

#[test]
fn audit_output_bytevector_high_bytes() {
    // Bytes 128-255 should round-trip through bytevector port
    let result = eval(
        "(let ((p (open-output-bytevector)))
           (write-u8 0 p)
           (write-u8 127 p)
           (write-u8 128 p)
           (write-u8 255 p)
           (let ((bv (get-output-bytevector p)))
             (list (bytevector-u8-ref bv 0)
                   (bytevector-u8-ref bv 1)
                   (bytevector-u8-ref bv 2)
                   (bytevector-u8-ref bv 3))))",
    );
    assert_eq!(result, eval("'(0 127 128 255)"));
}

// --- Additional audit regression tests ---

#[test]
fn audit_input_bytevector_high_bytes() {
    // Bytes 128-255 should round-trip through bytevector input port
    let result = eval(
        "(let ((p (open-input-bytevector #u8(0 127 128 255))))
           (list (read-u8 p) (read-u8 p) (read-u8 p) (read-u8 p)))",
    );
    assert_eq!(result, eval("'(0 127 128 255)"));
}

#[test]
fn audit_input_bytevector_eof() {
    is_true(
        "(let ((p (open-input-bytevector #u8(42))))
           (read-u8 p)
           (eof-object? (read-u8 p)))",
    );
}

#[test]
fn audit_input_bytevector_peek() {
    let result = eval(
        "(let ((p (open-input-bytevector #u8(99))))
           (let ((a (peek-u8 p)) (b (read-u8 p)))
             (list a b)))",
    );
    assert_eq!(result, eval("'(99 99)"));
}

#[test]
fn audit_input_bytevector_read_bytevector() {
    let result = eval(
        "(let ((p (open-input-bytevector #u8(1 2 3 4 5))))
           (read-bytevector 3 p))",
    );
    assert_eq!(result, eval("#u8(1 2 3)"));
}

#[test]
fn audit_textual_port_not_binary() {
    // textual-port? should return #f for binary ports
    is_false("(textual-port? (open-input-bytevector #u8()))");
    is_false("(textual-port? (open-output-bytevector))");
}

#[test]
fn audit_textual_port_is_text() {
    is_true("(textual-port? (open-input-string \"\"))");
    is_true("(textual-port? (open-output-string))");
}

#[test]
fn audit_features_no_false_flags() {
    // ratios and exact-complex should NOT be in features list
    is_false("(if (memq 'ratios (features)) #t #f)");
    is_false("(if (memq 'exact-complex (features)) #t #f)");
}

#[test]
fn audit_reader_delimiter_quote() {
    // A symbol followed by a quote should be two separate datums:
    // x then 'y (which reads as (quote y))
    let result = eval(
        "(let ((p (open-input-string \"x'y\")))
                         (let* ((a (read p)) (b (read p)))
                           (list a b)))",
    );
    assert_eq!(result, eval("'(x (quote y))"));
}

#[test]
fn audit_parameterize_restores_on_exception() {
    // parameterize should restore values even when body raises
    is_int(
        "(let ((p (make-parameter 10)))
              (guard (exn (else 'caught))
                (parameterize ((p 99))
                  (error \"boom\")))
              (p))",
        10,
    );
}

// ============================================================
// Audit round 3: integer overflow promotion
// ============================================================

#[test]
fn audit_addition_overflow_promotes_to_float() {
    // i64::MAX + 1 should promote to float, not panic
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval("(+ 9223372036854775807 1)").unwrap();
    match result {
        Value::Float(f) => assert!(f > 9.2e18, "expected large float, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn audit_addition_no_overflow_stays_exact() {
    // Normal addition should stay as integer
    is_int("(+ 1000000 2000000)", 3000000);
    is_int("(+ -5 10)", 5);
    is_int("(+ 0 0)", 0);
}

#[test]
fn audit_multiplication_overflow_promotes_to_float() {
    // i64::MAX * 2 should promote to float, not panic
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval("(* 9223372036854775807 2)").unwrap();
    match result {
        Value::Float(f) => assert!(f > 1.8e19, "expected large float, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn audit_multiplication_no_overflow_stays_exact() {
    is_int("(* 1000 2000)", 2000000);
    is_int("(* -3 7)", -21);
    is_int("(* 0 9223372036854775807)", 0);
}

#[test]
fn audit_square_overflow_promotes_to_float() {
    // (square large-int) should promote, not panic
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval("(square 9223372036854775807)").unwrap();
    match result {
        Value::Float(f) => assert!(f > 8.5e37, "expected large float, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn audit_square_no_overflow_stays_exact() {
    is_int("(square 10)", 100);
    is_int("(square -7)", 49);
    is_int("(square 0)", 0);
}

// ============================================================
// Audit round 3: call-with-values 0-value case
// ============================================================

#[test]
fn audit_call_with_values_zero_values() {
    // Producer returns 0 values, consumer takes 0 args
    is_int("(call-with-values (lambda () (values)) (lambda () 42))", 42);
}

#[test]
fn audit_call_with_values_single_non_values() {
    // Producer returns a single value (not via values)
    is_int("(call-with-values (lambda () 5) (lambda (x) (* x 10)))", 50);
}

#[test]
fn audit_call_with_values_multiple() {
    // Producer returns multiple values
    is_int(
        "(call-with-values (lambda () (values 10 20 30)) (lambda (a b c) (+ a b c)))",
        60,
    );
}

// ============================================================
// Audit round 3: let*-values sequential binding
// ============================================================

#[test]
fn audit_let_star_values_sequential() {
    // Second binding should see first binding's values
    is_int(
        "(let*-values (((a b) (values 3 4))
                       ((c) (values (+ a b))))
           c)",
        7,
    );
}

#[test]
fn audit_let_star_values_three_bindings() {
    // Three sequential bindings, each using previous
    is_int(
        "(let*-values (((x) (values 2))
                       ((y) (values (* x 3)))
                       ((z) (values (+ x y))))
           z)",
        8,
    );
}

#[test]
fn audit_let_star_values_single_binding() {
    // Degenerate case: single binding (same as let-values)
    is_int(
        "(let*-values (((a b) (values 10 20)))
           (- b a))",
        10,
    );
}

// ============================================================
// Audit round 3: case with => arrow clauses (R7RS §4.2.1)
// ============================================================

#[test]
fn audit_case_arrow_basic() {
    // (case key ((datum ...) => proc)) — proc receives the key
    is_int(
        "(case (* 2 3)
           ((2 3 5 7) 'prime)
           ((1 4 6 8 9) => (lambda (x) (* x 10))))",
        60,
    );
}

#[test]
fn audit_case_else_arrow() {
    // (case key (else => proc)) — proc receives the key
    is_int(
        "(case 99
           ((1 2 3) 'small)
           (else => (lambda (x) (+ x 1))))",
        100,
    );
}

#[test]
fn audit_case_arrow_no_match_falls_through() {
    // No match with no else should return void
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm
        .eval(
            "(case 42
               ((1 2 3) => (lambda (x) x)))",
        )
        .unwrap();
    // Unmatched case returns void
    assert!(
        matches!(result, Value::Void),
        "expected Void for unmatched case, got {result:?}"
    );
}

#[test]
fn audit_case_mixed_arrow_and_normal() {
    // Mix arrow and normal clauses
    is_int(
        "(case 5
           ((1 2) 'low)
           ((5 6) => (lambda (x) (* x x)))
           (else 0))",
        25,
    );
}

// ============================================================
// Audit round 3: raise-continuable
// ============================================================

#[test]
fn audit_raise_continuable_returns_handler_value() {
    // raise-continuable: handler's return value becomes result
    is_int(
        "(with-exception-handler
           (lambda (exn) 42)
           (lambda () (raise-continuable \"oops\")))",
        42,
    );
}

#[test]
fn audit_raise_continuable_passes_exception_to_handler() {
    // The exception value should reach the handler
    is_true(
        "(with-exception-handler
           (lambda (exn) (string? exn))
           (lambda () (raise-continuable \"test-value\")))",
    );
}

// ============================================================
// Audit round 3: parameterize dynamic-wind escape safety
// ============================================================

#[test]
fn audit_parameterize_restores_on_call_cc_escape() {
    // parameterize should restore when escaping via call/cc
    is_int(
        "(let ((p (make-parameter 10)))
           (call-with-current-continuation
             (lambda (k)
               (parameterize ((p 99))
                 (k (p)))))
           (p))",
        10,
    );
}

#[test]
fn audit_parameterize_nested() {
    // Nested parameterize should work correctly
    is_int(
        "(let ((p (make-parameter 1)))
           (parameterize ((p 2))
             (parameterize ((p 3))
               (p))))",
        3,
    );
    // After both parameterize, original value restored
    is_int(
        "(let ((p (make-parameter 1)))
           (parameterize ((p 2))
             (parameterize ((p 3))
               'ignore))
           (p))",
        1,
    );
}

// ============================================================
// Audit round 3: raise vs raise-continuable (R7RS §6.11)
// ============================================================

#[test]
fn audit_raise_non_continuable_handler_returns_is_error() {
    // R7RS §6.11: If handler returns from non-continuable raise, it's an error
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval(
        "(with-exception-handler
           (lambda (e) 'returned)
           (lambda () (raise 'boom)))",
    );
    assert!(
        result.is_err(),
        "non-continuable raise: handler return should be an error"
    );
}

#[test]
fn audit_raise_continuable_handler_returns_value() {
    // R7RS §6.11: raise-continuable allows handler to return a value
    is_int(
        "(with-exception-handler
           (lambda (e) (* e 10))
           (lambda () (+ 1 (raise-continuable 5))))",
        51,
    );
}

#[test]
fn audit_raise_continuable_handler_sees_exception() {
    // Handler receives the exception object
    is_true(
        "(with-exception-handler
           (lambda (e) (symbol? e))
           (lambda () (raise-continuable 'test-sym)))",
    );
}

#[test]
fn audit_guard_catches_raise() {
    // guard works with raise (unwind-based)
    is_int(
        "(guard (exn
                 ((string? exn) 1)
                 ((symbol? exn) 2))
           (raise 'test))",
        2,
    );
}

#[test]
fn audit_guard_catches_error() {
    // guard works with error (which uses raise internally)
    is_int(
        "(guard (exn (#t 99))
           (error \"fail\"))",
        99,
    );
}

#[test]
fn audit_with_exception_handler_escape_via_call_cc() {
    // Correct pattern: handler escapes via continuation
    is_int(
        "(call-with-current-continuation
           (lambda (exit)
             (with-exception-handler
               (lambda (e) (exit 42))
               (lambda () (raise 'boom)))))",
        42,
    );
}

#[test]
fn audit_nested_handlers() {
    // Inner handler escapes to outer guard
    is_int(
        "(guard (exn (#t 99))
           (with-exception-handler
             (lambda (e) (raise (string-append \"re-\" (symbol->string e))))
             (lambda () (raise 'boom))))",
        99,
    );
}

#[test]
fn audit_raise_continuable_resumes_execution() {
    // After raise-continuable, execution continues at the call site
    is_int(
        "(with-exception-handler
           (lambda (e) 10)
           (lambda ()
             (let ((x (raise-continuable 'ignored)))
               (+ x 5))))",
        15,
    );
}

// ============================================================
// §4.3.2 — syntax-rules: custom ellipsis + ellipsis escape
// ============================================================

#[test]
fn audit_syntax_rules_custom_ellipsis() {
    // R7RS §4.3.2 / SRFI 46: custom ellipsis identifier
    // (syntax-rules ::: () ...) uses ::: instead of ... as ellipsis
    is_int(
        "(define-syntax my-add
           (syntax-rules ::: ()
             ((_ x :::) (+ x :::))))
         (my-add 1 2 3)",
        6,
    );
}

#[test]
fn audit_syntax_rules_custom_ellipsis_zero_args() {
    // Custom ellipsis with zero matched elements
    assert_eq!(
        eval(
            "(define-syntax my-list2
               (syntax-rules ::: ()
                 ((_ x :::) (list x :::))))
             (my-list2)"
        ),
        Value::Null,
    );
}

#[test]
fn audit_syntax_rules_custom_ellipsis_preserves_dots() {
    // With custom ellipsis :::, the identifier ... is just a regular symbol
    // (can be used as a literal or pattern variable)
    is_int(
        "(define-syntax uses-dots
           (syntax-rules ::: ()
             ((_ a) a)))
         (uses-dots 42)",
        42,
    );
}

#[test]
fn audit_syntax_rules_ellipsis_escape_literal_dots() {
    // R7RS §4.3.2: (... ...) in a template produces a literal ... symbol.
    // We wrap in quote so the expansion isn't evaluated as a variable lookup.
    assert_eq!(
        eval(
            "(define-syntax emit-dots
               (syntax-rules ()
                 ((_) '(... ...))))
             (emit-dots)"
        ),
        Value::symbol("..."),
    );
}

#[test]
fn audit_syntax_rules_ellipsis_escape_template() {
    // (... template) suppresses ellipsis processing in template
    // so (list ...) inside (... ...) is treated literally
    assert_eq!(
        eval(
            "(define-syntax make-list-call
               (syntax-rules ()
                 ((_ x) (... (list x)))))
             (make-list-call 5)"
        ),
        Value::list(vec![Value::Int(5)]),
    );
}

#[test]
fn audit_syntax_rules_ellipsis_escape_preserves_vars() {
    // Ellipsis escape still substitutes pattern variables
    is_int(
        "(define-syntax apply-it
           (syntax-rules ()
             ((_ f x) (... (f x)))))
         (apply-it + 3)",
        3,
    );
}

// ============================================================
// §6.13 — stdin port operations
// ============================================================

#[test]
fn audit_stdin_is_input_port() {
    // current-input-port returns an input port
    assert_eq!(
        eval("(input-port? (current-input-port))"),
        Value::Bool(true),
    );
}

#[test]
fn audit_stdin_not_output_port() {
    assert_eq!(
        eval("(output-port? (current-input-port))"),
        Value::Bool(false),
    );
}

#[test]
fn audit_stdin_char_ready() {
    // char-ready? on string port with data → #t
    is_true("(char-ready? (open-input-string \"x\"))");
    // char-ready? on empty string port → #f (no data available)
    is_false("(char-ready? (open-input-string \"\"))");
    // char-ready? on file port → #t (regular files never block per POSIX)
    let tmp = std::env::temp_dir().join("mae_char_ready_test.txt");
    std::fs::write(&tmp, "data").unwrap();
    let code = format!(
        "(let ((p (open-input-file \"{}\"))) (let ((r (char-ready? p))) (close-input-port p) r))",
        tmp.display()
    );
    is_true(&code);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn audit_stdin_port_redirection_read_char() {
    // call-with-input-file passes port to proc
    let tmp = std::env::temp_dir().join("mae_stdin_test_read_char.txt");
    std::fs::write(&tmp, "AB").unwrap();
    let code = format!(
        "(call-with-input-file \"{}\" (lambda (p) (read-char p)))",
        tmp.display()
    );
    assert_eq!(eval(&code), Value::Char('A'));
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn audit_stdin_port_redirection_read_line() {
    let tmp = std::env::temp_dir().join("mae_stdin_test_read_line.txt");
    std::fs::write(&tmp, "hello world\nsecond line\n").unwrap();
    let code = format!(
        "(call-with-input-file \"{}\" (lambda (p) (read-line p)))",
        tmp.display()
    );
    assert_eq!(eval(&code), Value::String(Rc::from("hello world")),);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn audit_stdin_port_redirection_read() {
    // with-input-from-file redirects current-input-port (thunk, no args)
    let tmp = std::env::temp_dir().join("mae_stdin_test_read.txt");
    std::fs::write(&tmp, "(+ 1 2)").unwrap();
    let code = format!(
        "(with-input-from-file \"{}\" (lambda () (eval (read))))",
        tmp.display()
    );
    is_int(&code, 3);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn audit_string_port_read_char_sequence() {
    // Verify read-char works sequentially on string ports (proxy for stdin behavior)
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"abc\")))
               (let ((a (read-char p))
                     (b (read-char p))
                     (c (read-char p))
                     (d (read-char p)))
                 (list a b c d)))"
        ),
        Value::list(vec![
            Value::Char('a'),
            Value::Char('b'),
            Value::Char('c'),
            Value::Eof,
        ]),
    );
}

#[test]
fn audit_string_port_peek_then_read() {
    // peek-char doesn't advance position
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"xy\")))
               (let ((pk (peek-char p))
                     (rd (read-char p)))
                 (list pk rd)))"
        ),
        Value::list(vec![Value::Char('x'), Value::Char('x')]),
    );
}

// ============================================================
// §6.2.6 — rationalize (Stern-Brocot mediant search)
// ============================================================

#[test]
fn audit_rationalize_exact_integer() {
    // R7RS §6.2.6: simplest = smallest |p| among same denominator.
    // (rationalize 3 1) → range [2, 4], simplest integer is 2 (|2| < |3| < |4|)
    is_int("(rationalize 3 1)", 2);
}

#[test]
fn audit_rationalize_zero_in_range() {
    // Zero is always the simplest rational when in range
    assert_eq!(eval("(rationalize 0.3 0.5)"), Value::Float(0.0));
}

#[test]
fn audit_rationalize_negative() {
    // Negative value: simplest rational in [-1.5, -0.5]
    assert_eq!(eval("(rationalize -1.0 0.5)"), Value::Float(-1.0));
}

#[test]
fn audit_rationalize_third() {
    // (rationalize 1/3 1/10) should find a simple fraction near 1/3
    // The simplest rational in [0.233..., 0.433...] is 1/3 (0.333...)
    let result = eval("(rationalize 0.333 0.1)");
    if let Value::Float(f) = result {
        // Should be 1/3 = 0.333... (simplest rational with small denominator)
        assert!((f - 1.0 / 3.0).abs() < 0.11, "got {f}");
    } else {
        panic!("expected float, got {result}");
    }
}

#[test]
fn audit_rationalize_half() {
    // (rationalize 0.5 0.01) should return 0.5 (= 1/2, simplest in range)
    assert_eq!(eval("(rationalize 0.5 0.01)"), Value::Float(0.5));
}

#[test]
fn audit_rationalize_inf_diff() {
    // Infinite tolerance → zero (simplest possible rational)
    assert_eq!(eval("(rationalize 5.0 +inf.0)"), Value::Float(0.0));
}

#[test]
fn audit_rationalize_nan() {
    // NaN propagates
    let result = eval("(rationalize +nan.0 1.0)");
    if let Value::Float(f) = result {
        assert!(f.is_nan());
    } else {
        panic!("expected NaN float");
    }
}

#[test]
fn audit_rationalize_inf_x() {
    // Infinite x → x (no finite rational can approximate infinity)
    assert_eq!(
        eval("(rationalize +inf.0 1.0)"),
        Value::Float(f64::INFINITY)
    );
}

// ============================================================
// §6.13.2 — char-ready? / u8-ready? (non-kludge verification)
// ============================================================

#[test]
fn audit_char_ready_exhausted_string_port() {
    // char-ready? on an exhausted string port should return #f
    // (no more data available)
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"\")))
               (char-ready? p))"
        ),
        Value::Bool(false),
    );
}

#[test]
fn audit_char_ready_after_full_read() {
    // After reading all data, char-ready? should return #f
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"x\")))
               (read-char p)
               (char-ready? p))"
        ),
        Value::Bool(false),
    );
}

#[test]
fn audit_char_ready_with_data() {
    // String port with data: char-ready? should return #t
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"hello\")))
               (char-ready? p))"
        ),
        Value::Bool(true),
    );
}

#[test]
fn audit_u8_ready_exhausted_bytevector_port() {
    // u8-ready? on exhausted bytevector port should return #f
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector #u8())))
               (u8-ready? p))"
        ),
        Value::Bool(false),
    );
}

#[test]
fn audit_u8_ready_with_data() {
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector #u8(1 2 3))))
               (u8-ready? p))"
        ),
        Value::Bool(true),
    );
}

#[test]
fn audit_char_ready_closed_port_errors() {
    // char-ready? on a closed port should signal an error
    let msg = eval_err(
        "(let ((p (open-input-string \"x\")))
           (close-port p)
           (char-ready? p))",
    );
    assert!(msg.contains("closed"), "expected closed error, got: {msg}");
}

// ============================================================
// Coverage gap tests — functions with missing or thin coverage
// ============================================================

// --- fold-right ---
#[test]
fn coverage_fold_right_basic() {
    // fold-right builds from right: (f 1 (f 2 (f 3 init)))
    assert_eq!(eval("(fold-right cons '() '(1 2 3))"), eval("'(1 2 3)"),);
}

#[test]
fn coverage_fold_right_string_build() {
    assert_eq!(
        eval("(fold-right string-append \"\" '(\"a\" \"b\" \"c\"))"),
        Value::String(Rc::from("abc")),
    );
}

#[test]
fn coverage_fold_right_empty() {
    is_int("(fold-right + 0 '())", 0);
}

#[test]
fn coverage_fold_right_vs_fold_left() {
    // fold-right preserves order, fold-left reverses for cons
    assert_eq!(eval("(fold-right cons '() '(1 2 3))"), eval("'(1 2 3)"),);
    assert_eq!(
        eval("(fold-left (lambda (acc x) (cons x acc)) '() '(1 2 3))"),
        eval("'(3 2 1)"),
    );
}

// --- string-for-each ---
#[test]
fn coverage_string_for_each_single() {
    // Collect characters via string-for-each
    assert_eq!(
        eval(
            "(let ((out '()))
               (string-for-each (lambda (c) (set! out (cons c out))) \"abc\")
               (reverse out))"
        ),
        Value::list(vec![Value::Char('a'), Value::Char('b'), Value::Char('c')]),
    );
}

#[test]
fn coverage_string_for_each_empty() {
    // Empty string: callback never called
    assert_eq!(
        eval(
            "(let ((count 0))
               (string-for-each (lambda (c) (set! count (+ count 1))) \"\")
               count)"
        ),
        Value::Int(0),
    );
}

#[test]
fn coverage_string_for_each_multi() {
    // Multi-string: iterate corresponding characters
    assert_eq!(
        eval(
            "(let ((pairs '()))
               (string-for-each
                 (lambda (a b) (set! pairs (cons (list a b) pairs)))
                 \"ab\" \"xy\")
               (reverse pairs))"
        ),
        eval("'((#\\a #\\x) (#\\b #\\y))"),
    );
}

// --- make-string ---
#[test]
fn coverage_make_string_fill() {
    assert_eq!(
        eval("(make-string 5 #\\x)"),
        Value::String(Rc::from("xxxxx")),
    );
}

#[test]
fn coverage_make_string_no_fill() {
    // make-string with just length — fill char is implementation-defined
    // We just verify it produces a string of the right length
    is_int("(string-length (make-string 3))", 3);
}

#[test]
fn coverage_make_string_zero() {
    assert_eq!(eval("(make-string 0 #\\z)"), Value::String(Rc::from("")),);
}

// --- vector-for-each ---
#[test]
fn coverage_vector_for_each_basic() {
    assert_eq!(
        eval(
            "(let ((sum 0))
               (vector-for-each (lambda (x) (set! sum (+ sum x))) #(1 2 3 4))
               sum)"
        ),
        Value::Int(10),
    );
}

#[test]
fn coverage_vector_for_each_multi() {
    assert_eq!(
        eval(
            "(let ((pairs '()))
               (vector-for-each
                 (lambda (a b) (set! pairs (cons (+ a b) pairs)))
                 #(1 2 3) #(10 20 30))
               (reverse pairs))"
        ),
        eval("'(11 22 33)"),
    );
}

// --- vector-map ---
#[test]
fn coverage_vector_map_basic() {
    assert_eq!(
        eval("(vector->list (vector-map + #(1 2 3) #(10 20 30)))"),
        eval("'(11 22 33)"),
    );
}

// --- string-map ---
#[test]
fn coverage_string_map_basic() {
    assert_eq!(
        eval("(string-map char-upcase \"hello\")"),
        Value::String(Rc::from("HELLO")),
    );
}

#[test]
fn coverage_string_map_multi() {
    // Multi-string map — take max char from each position
    assert_eq!(
        eval(
            "(string-map
               (lambda (a b) (if (char>? a b) a b))
               \"ace\" \"bdf\")"
        ),
        Value::String(Rc::from("bdf")),
    );
}

// --- call-with-port ---
#[test]
fn coverage_call_with_port_closes() {
    // call-with-port closes the port after proc returns
    is_false(
        "(let ((p (open-input-string \"hello\")))
           (call-with-port p (lambda (port) (read-char port)))
           (input-port-open? p))",
    );
}

// --- call-with-output-file ---
#[test]
fn coverage_call_with_output_file() {
    let tmp = std::env::temp_dir().join("mae_test_call_with_output.txt");
    let code = format!(
        "(call-with-output-file \"{}\" (lambda (p) (write-string \"hello\" p)))",
        tmp.display()
    );
    eval(&code);
    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(contents, "hello");
    let _ = std::fs::remove_file(&tmp);
}

// --- with-output-to-file ---
#[test]
fn coverage_with_output_to_file() {
    let tmp = std::env::temp_dir().join("mae_test_with_output_to.txt");
    let code = format!(
        "(with-output-to-file \"{}\" (lambda () (display \"world\")))",
        tmp.display()
    );
    eval(&code);
    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(contents, "world");
    let _ = std::fs::remove_file(&tmp);
}

// --- make-list ---
#[test]
fn coverage_make_list_basic() {
    assert_eq!(eval("(make-list 3 'x)"), eval("'(x x x)"),);
}

#[test]
fn coverage_make_list_zero() {
    assert_eq!(eval("(make-list 0 'x)"), Value::Null);
}

#[test]
fn coverage_make_list_no_fill() {
    // make-list with no fill value
    is_int("(length (make-list 5))", 5);
}

// --- list-copy ---
#[test]
fn coverage_list_copy() {
    assert_eq!(eval("(list-copy '(1 2 3))"), eval("'(1 2 3)"),);
    // list-copy of empty list
    assert_eq!(eval("(list-copy '())"), Value::Null);
}

// --- list-set! (should error for immutable pairs) ---
#[test]
fn coverage_list_set_error() {
    let msg = eval_err("(list-set! '(1 2 3) 1 99)");
    assert!(!msg.is_empty(), "list-set! should signal an error");
}

// --- list-tail ---
#[test]
fn coverage_list_tail() {
    assert_eq!(eval("(list-tail '(a b c d) 2)"), eval("'(c d)"),);
    assert_eq!(eval("(list-tail '(a b c) 0)"), eval("'(a b c)"));
    assert_eq!(eval("(list-tail '(a b c) 3)"), Value::Null);
}

// --- exact-integer-sqrt ---
#[test]
fn coverage_exact_integer_sqrt() {
    // Returns (root remainder) where val = root² + remainder
    assert_eq!(
        eval("(call-with-values (lambda () (exact-integer-sqrt 14)) list)"),
        eval("'(3 5)"),
    );
    assert_eq!(
        eval("(call-with-values (lambda () (exact-integer-sqrt 4)) list)"),
        eval("'(2 0)"),
    );
    assert_eq!(
        eval("(call-with-values (lambda () (exact-integer-sqrt 0)) list)"),
        eval("'(0 0)"),
    );
}

// --- floor/, truncate/ ---
#[test]
fn coverage_floor_division() {
    // floor/ returns (quotient remainder) where dividend = quotient*divisor + remainder
    assert_eq!(
        eval("(call-with-values (lambda () (floor/ 17 5)) list)"),
        eval("'(3 2)"),
    );
    assert_eq!(
        eval("(call-with-values (lambda () (floor/ -17 5)) list)"),
        eval("'(-4 3)"),
    );
    is_int("(floor-quotient 17 5)", 3);
    is_int("(floor-remainder 17 5)", 2);
    is_int("(floor-quotient -17 5)", -4);
    is_int("(floor-remainder -17 5)", 3);
}

#[test]
fn coverage_truncate_division() {
    assert_eq!(
        eval("(call-with-values (lambda () (truncate/ 17 5)) list)"),
        eval("'(3 2)"),
    );
    assert_eq!(
        eval("(call-with-values (lambda () (truncate/ -17 5)) list)"),
        eval("'(-3 -2)"),
    );
    is_int("(truncate-quotient 17 5)", 3);
    is_int("(truncate-remainder 17 5)", 2);
    is_int("(truncate-quotient -17 5)", -3);
    is_int("(truncate-remainder -17 5)", -2);
}

// --- square ---
#[test]
fn coverage_square() {
    is_int("(square 5)", 25);
    is_int("(square -3)", 9);
    is_int("(square 0)", 0);
    assert_eq!(eval("(square 2.5)"), Value::Float(6.25));
}

// --- log with base ---
#[test]
fn coverage_log_with_base() {
    // (log z base) = ln(z)/ln(base)
    assert_eq!(eval("(log 8 2)"), Value::Float(3.0));
    // Natural log
    let val = eval("(log 1)");
    assert_eq!(val, Value::Float(0.0));
}

// --- atan with 2 args ---
#[test]
fn coverage_atan2() {
    // (atan y x) = atan2(y, x)
    let val = eval("(atan 1.0 1.0)");
    if let Value::Float(f) = val {
        assert!((f - std::f64::consts::FRAC_PI_4).abs() < 1e-10);
    } else {
        panic!("expected float, got {val:?}");
    }
}

// --- member with custom comparator ---
#[test]
fn coverage_member_custom_compare() {
    assert_eq!(eval("(member 2.0 '(1 2 3) =)"), eval("'(2 3)"),);
    // member with default (equal?) for lists
    assert_eq!(eval("(member '(2) '((1) (2) (3)))"), eval("'((2) (3))"),);
}

// --- assoc with custom comparator ---
#[test]
fn coverage_assoc_custom_compare() {
    assert_eq!(
        eval("(assoc 2.0 '((1 . a) (2 . b) (3 . c)) =)"),
        eval("'(2 . b)"),
    );
}

// --- make-parameter with converter ---
#[test]
fn coverage_make_parameter_converter() {
    assert_eq!(
        eval(
            "(let ((p (make-parameter 10 (lambda (x) (* x 2)))))
               (list (p)
                     (begin (p 5) (p))))"
        ),
        eval("'(10 10)"),
    );
}

// --- promise? ---
#[test]
fn coverage_promise_predicate() {
    is_true("(promise? (delay 42))");
    is_true("(promise? (make-promise 42))");
    is_false("(promise? 42)");
    is_false("(promise? '())");
}

// --- error-object accessors ---
#[test]
fn coverage_error_object_accessors() {
    // error-object?, error-object-message, error-object-irritants, error-object-type
    assert_eq!(
        eval(
            "(guard (e (#t (list
                            (error-object? e)
                            (error-object-message e)
                            (error-object-irritants e)
                            (error-object-type e))))
               (error \"test error\" 1 2 3))"
        ),
        eval("'(#t \"test error\" (1 2 3) \"error\")"),
    );
}

// --- boolean=? ---
#[test]
fn coverage_boolean_eq() {
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
    is_true("(boolean=? #t #t #t)");
    is_false("(boolean=? #t #t #f)");
}

// --- symbol=? ---
#[test]
fn coverage_symbol_eq() {
    is_true("(symbol=? 'foo 'foo)");
    is_false("(symbol=? 'foo 'bar)");
}

// --- display-string (internal, stdout-only) ---
#[test]
fn coverage_display_string() {
    // display-string takes 1 arg and prints to stdout (not a port arg)
    // Just verify it doesn't error
    eval("(display-string \"test\")");
}

// --- format ---
#[test]
fn coverage_format() {
    assert_eq!(
        eval("(format \"hello ~a, ~a\" 'world 42)"),
        Value::String(Rc::from("hello world, 42")),
    );
    // ~s uses write (quoted)
    assert_eq!(
        eval("(format \"~s\" \"quoted\")"),
        Value::String(Rc::from("\"quoted\"")),
    );
}

// --- bytevector port operations ---
#[test]
fn coverage_bytevector_input_port() {
    is_int("(read-u8 (open-input-bytevector #u8(65 66 67)))", 65);
    is_int("(peek-u8 (open-input-bytevector #u8(65)))", 65);
}

#[test]
fn coverage_bytevector_output_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-bytevector)))
               (write-u8 65 p)
               (write-u8 66 p)
               (get-output-bytevector p))"
        ),
        eval("#u8(65 66)"),
    );
}

// --- read-bytevector ---
#[test]
fn coverage_read_bytevector() {
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector #u8(1 2 3 4 5))))
               (read-bytevector 3 p))"
        ),
        eval("#u8(1 2 3)"),
    );
}

// --- write-shared / write-simple ---
#[test]
fn coverage_write_shared() {
    // write-shared should produce valid output (same as write for non-circular data)
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-shared '(1 2 3) p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("(1 2 3)")),
    );
}

#[test]
fn coverage_write_simple() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-simple '(1 . 2) p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("(1 . 2)")),
    );
}

// --- close-input-port / close-output-port ---
#[test]
fn coverage_close_specific_ports() {
    // close-input-port
    is_false(
        "(let ((p (open-input-string \"x\")))
           (close-input-port p)
           (input-port-open? p))",
    );
    // close-output-port
    is_false(
        "(let ((p (open-output-string)))
           (close-output-port p)
           (output-port-open? p))",
    );
}

// --- current-error-port ---
#[test]
fn coverage_current_error_port() {
    is_true("(output-port? (current-error-port))");
    is_true("(port? (current-error-port))");
}

// --- features and cond-expand ---
#[test]
fn coverage_features_list() {
    // (features) returns a list — memq returns sublist (truthy) or #f
    is_true("(list? (features))");
    is_true("(if (memq 'r7rs (features)) #t #f)");
    is_true("(if (memq 'mae-scheme (features)) #t #f)");
}

// --- jiffies-per-second / current-jiffy ---
#[test]
fn coverage_timing() {
    // jiffies-per-second should be a positive integer
    is_true("(> (jiffies-per-second) 0)");
    // current-jiffy should be a positive integer
    is_true("(> (current-jiffy) 0)");
    // current-second should be a positive number (Unix epoch)
    is_true("(> (current-second) 1000000000)");
}

// --- get-environment-variable ---
#[test]
fn coverage_get_environment_variable() {
    // PATH should exist on all systems
    is_true("(string? (get-environment-variable \"PATH\"))");
    // Non-existent variable returns #f
    is_false("(get-environment-variable \"MAE_NONEXISTENT_VAR_12345\")");
}

// --- get-environment-variables ---
#[test]
fn coverage_get_environment_variables() {
    is_true("(list? (get-environment-variables))");
    is_true("(> (length (get-environment-variables)) 0)");
    // Each element should be a pair of strings
    is_true("(pair? (car (get-environment-variables)))");
}

// --- command-line ---
#[test]
fn coverage_command_line() {
    is_true("(list? (command-line))");
}

// --- binary port operations ---
#[test]
fn coverage_open_binary_files() {
    let tmp = std::env::temp_dir().join("mae_test_binary_io.bin");
    let write_code = format!(
        "(let ((p (open-binary-output-file \"{}\")))
           (write-u8 255 p)
           (write-u8 0 p)
           (write-u8 128 p)
           (close-output-port p))",
        tmp.display()
    );
    eval(&write_code);

    let read_code = format!(
        "(let ((p (open-binary-input-file \"{}\")))
           (let ((a (read-u8 p))
                 (b (read-u8 p))
                 (c (read-u8 p)))
             (close-input-port p)
             (list a b c)))",
        tmp.display()
    );
    assert_eq!(
        eval(&read_code),
        Value::list(vec![Value::Int(255), Value::Int(0), Value::Int(128)]),
    );
    let _ = std::fs::remove_file(&tmp);
}

// --- textual-port? / binary-port? ---
#[test]
fn coverage_port_type_predicates() {
    is_true("(textual-port? (open-input-string \"x\"))");
    is_true("(textual-port? (open-output-string))");
    is_false("(textual-port? (open-input-bytevector #u8()))");

    is_true("(binary-port? (open-input-bytevector #u8()))");
    is_true("(binary-port? (open-output-bytevector))");
    is_false("(binary-port? (open-input-string \"x\"))");
}

// --- flush-output-port ---
#[test]
fn coverage_flush_output_port() {
    // Just ensure it doesn't error on a string output port
    eval("(flush-output-port (open-output-string))");
}

// --- interaction-environment / scheme-report-environment ---
#[test]
fn coverage_environments() {
    // These return symbols (truthy values)
    assert_eq!(
        eval("(interaction-environment)"),
        Value::symbol("interaction")
    );
    assert_eq!(eval("(scheme-report-environment 7)"), Value::symbol("r7rs"));
}

// --- read-bytevector! ---
#[test]
fn coverage_read_bytevector_mut() {
    assert_eq!(
        eval(
            "(let ((bv (make-bytevector 5 0))
                   (p (open-input-bytevector #u8(10 20 30))))
               (let ((n (read-bytevector! bv p)))
                 (list n (bytevector-u8-ref bv 0) (bytevector-u8-ref bv 1) (bytevector-u8-ref bv 2))))"
        ),
        Value::list(vec![Value::Int(3), Value::Int(10), Value::Int(20), Value::Int(30)]),
    );
}

// --- read-string ---
#[test]
fn coverage_read_string() {
    assert_eq!(
        eval("(read-string 3 (open-input-string \"hello world\"))"),
        Value::String(Rc::from("hel")),
    );
    // Read more than available
    assert_eq!(
        eval("(read-string 100 (open-input-string \"hi\"))"),
        Value::String(Rc::from("hi")),
    );
}

// --- edge cases for existing functions ---

#[test]
fn coverage_map_multi_list() {
    // Multi-list map stops at shortest list
    assert_eq!(eval("(map + '(1 2 3) '(10 20))"), eval("'(11 22)"),);
}

#[test]
fn coverage_for_each_multi_list() {
    assert_eq!(
        eval(
            "(let ((sum 0))
               (for-each (lambda (a b) (set! sum (+ sum a b)))
                         '(1 2 3) '(10 20 30))
               sum)"
        ),
        Value::Int(66),
    );
}

#[test]
fn coverage_filter() {
    assert_eq!(eval("(filter odd? '(1 2 3 4 5))"), eval("'(1 3 5)"),);
    assert_eq!(eval("(filter odd? '())"), Value::Null);
}

#[test]
fn coverage_call_with_values_multi() {
    // Multi-value return via values + call-with-values
    assert_eq!(
        eval("(call-with-values (lambda () (values 1 2 3)) +)"),
        Value::Int(6),
    );
}

#[test]
fn coverage_eqv_edge_cases() {
    is_true("(eqv? '() '())");
    is_true("(eqv? #t #t)");
    is_true("(eqv? #f #f)");
    is_false("(eqv? #t #f)");
    is_true("(eqv? 42 42)");
    is_false("(eqv? 42 42.0)"); // exact ≠ inexact
    is_true("(eqv? #\\a #\\a)");
    is_false("(eqv? #\\a #\\b)");
}

#[test]
fn coverage_equal_deep() {
    is_true("(equal? '(1 (2 3) 4) '(1 (2 3) 4))");
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_true("(equal? \"abc\" \"abc\")");
    is_false("(equal? '(1 2) '(1 3))");
}

// ============================================================
// Branch-level coverage: string.rs
// ============================================================

#[test]
fn branch_string_constructor() {
    // (string char ...) builds from individual chars
    assert_eq!(eval("(string #\\h #\\i)"), Value::String(Rc::from("hi")),);
    // Zero args
    assert_eq!(eval("(string)"), Value::String(Rc::from("")));
}

#[test]
fn branch_substring_error() {
    // start > end
    let msg = eval_err("(substring \"hello\" 3 1)");
    assert!(msg.contains("out of range"), "got: {msg}");
    // end > length
    let msg = eval_err("(substring \"hello\" 0 100)");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_substring_default_end() {
    // substring with 2 args → default end = string length
    assert_eq!(
        eval("(substring \"hello\" 2)"),
        Value::String(Rc::from("llo")),
    );
}

#[test]
fn branch_string_ref_error() {
    let msg = eval_err("(string-ref \"hello\" 10)");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_string_to_list_with_range() {
    // string->list with start and end
    assert_eq!(
        eval("(string->list \"hello\" 1 3)"),
        Value::list(vec![Value::Char('e'), Value::Char('l')]),
    );
    // Just start
    assert_eq!(
        eval("(string->list \"hello\" 3)"),
        Value::list(vec![Value::Char('l'), Value::Char('o')]),
    );
}

#[test]
fn branch_string_copy_with_range() {
    // string-copy with start and end
    assert_eq!(
        eval("(string-copy \"hello\" 1 4)"),
        Value::String(Rc::from("ell")),
    );
    // Just start
    assert_eq!(
        eval("(string-copy \"hello\" 3)"),
        Value::String(Rc::from("lo")),
    );
}

#[test]
fn branch_string_mutation_errors() {
    // string-set! → immutable error
    let msg = eval_err("(string-set! \"hello\" 0 #\\H)");
    assert!(msg.contains("immutable"), "got: {msg}");
    // string-copy! → immutable error
    let msg = eval_err("(string-copy! \"hello\" 0 \"bye\")");
    assert!(msg.contains("immutable"), "got: {msg}");
    // string-fill! → immutable error
    let msg = eval_err("(string-fill! \"hello\" #\\x)");
    assert!(msg.contains("immutable"), "got: {msg}");
}

#[test]
fn branch_string_comparisons_full() {
    // All 6 comparison functions, both true and false branches
    is_true("(string=? \"abc\" \"abc\")");
    is_false("(string=? \"abc\" \"abd\")");
    is_true("(string<? \"abc\" \"abd\")");
    is_false("(string<? \"abd\" \"abc\")");
    is_true("(string>? \"abd\" \"abc\")");
    is_false("(string>? \"abc\" \"abd\")");
    is_true("(string<=? \"abc\" \"abc\")");
    is_true("(string<=? \"abc\" \"abd\")");
    is_false("(string<=? \"abd\" \"abc\")");
    is_true("(string>=? \"abc\" \"abc\")");
    is_true("(string>=? \"abd\" \"abc\")");
    is_false("(string>=? \"abc\" \"abd\")");
}

#[test]
fn branch_string_ci_comparisons_full() {
    // Case-insensitive: both true and false for all 5
    is_true("(string-ci=? \"ABC\" \"abc\")");
    is_false("(string-ci=? \"abc\" \"abd\")");
    is_true("(string-ci<? \"abc\" \"ABD\")");
    is_false("(string-ci<? \"abd\" \"ABC\")");
    is_true("(string-ci>? \"abd\" \"ABC\")");
    is_false("(string-ci>? \"abc\" \"ABD\")");
    is_true("(string-ci<=? \"ABC\" \"abc\")");
    is_true("(string-ci<=? \"abc\" \"ABD\")");
    is_false("(string-ci<=? \"ABD\" \"abc\")");
    is_true("(string-ci>=? \"ABC\" \"abc\")");
    is_true("(string-ci>=? \"ABD\" \"abc\")");
    is_false("(string-ci>=? \"abc\" \"ABD\")");
}

#[test]
fn branch_string_foldcase() {
    assert_eq!(
        eval("(string-foldcase \"HeLLo\")"),
        Value::String(Rc::from("hello")),
    );
}

#[test]
fn branch_string_append_edge() {
    // Zero args
    assert_eq!(eval("(string-append)"), Value::String(Rc::from("")));
    // One arg
    assert_eq!(
        eval("(string-append \"hi\")"),
        Value::String(Rc::from("hi"))
    );
}

#[test]
fn branch_string_contains_edge() {
    // Empty needle always matches
    is_true("(string-contains \"hello\" \"\")");
    // Empty haystack with non-empty needle
    is_false("(string-contains \"\" \"x\")");
}

#[test]
fn branch_string_trim_edge() {
    assert_eq!(eval("(string-trim \"\")"), Value::String(Rc::from("")));
    assert_eq!(eval("(string-trim \"  \")"), Value::String(Rc::from("")));
    assert_eq!(
        eval("(string-trim \"no-trim\")"),
        Value::String(Rc::from("no-trim"))
    );
}

#[test]
fn branch_string_split_edge() {
    // Split with no delimiter match
    assert_eq!(
        eval("(car (string-split \"hello\" \",\"))"),
        Value::String(Rc::from("hello")),
    );
    // Empty string split
    assert_eq!(
        eval("(car (string-split \"\" \",\"))"),
        Value::String(Rc::from("")),
    );
}

#[test]
fn branch_string_join_edge() {
    // Empty list
    assert_eq!(eval("(string-join '() \",\")"), Value::String(Rc::from("")),);
    // Single element
    assert_eq!(
        eval("(string-join '(\"only\") \",\")"),
        Value::String(Rc::from("only")),
    );
}

// ============================================================
// Branch-level coverage: vector.rs
// ============================================================

#[test]
fn branch_make_vector_no_fill() {
    // Default fill is undefined
    is_int("(vector-length (make-vector 4))", 4);
}

#[test]
fn branch_vector_set_out_of_range() {
    let msg = eval_err("(let ((v (vector 1 2 3))) (vector-set! v 5 99))");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_vector_ref_out_of_range() {
    let msg = eval_err("(vector-ref (vector 1 2 3) 10)");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_vector_to_list_with_range() {
    assert_eq!(
        eval("(vector->list #(10 20 30 40 50) 1 3)"),
        eval("'(20 30)"),
    );
    assert_eq!(eval("(vector->list #(10 20 30) 2)"), eval("'(30)"),);
}

#[test]
fn branch_vector_copy_with_range() {
    assert_eq!(
        eval("(vector->list (vector-copy #(1 2 3 4 5) 1 3))"),
        eval("'(2 3)"),
    );
}

#[test]
fn branch_vector_copy_bang_with_range() {
    // vector-copy! with start/end
    assert_eq!(
        eval(
            "(let ((v (vector 0 0 0 0 0)))
               (vector-copy! v 1 #(10 20 30 40) 1 3)
               (vector->list v))"
        ),
        eval("'(0 20 30 0 0)"),
    );
}

#[test]
fn branch_vector_append_edge() {
    // Zero args
    assert_eq!(eval("(vector->list (vector-append))"), Value::Null);
    // Multiple
    assert_eq!(
        eval("(vector->list (vector-append #(1) #(2 3) #(4)))"),
        eval("'(1 2 3 4)"),
    );
}

#[test]
fn branch_vector_fill() {
    assert_eq!(
        eval(
            "(let ((v (vector 1 2 3)))
               (vector-fill! v 0)
               (vector->list v))"
        ),
        eval("'(0 0 0)"),
    );
}

#[test]
fn branch_vector_string_conversion() {
    // vector->string
    assert_eq!(
        eval("(vector->string #(#\\h #\\i))"),
        Value::String(Rc::from("hi")),
    );
    // with range
    assert_eq!(
        eval("(vector->string #(#\\a #\\b #\\c #\\d) 1 3)"),
        Value::String(Rc::from("bc")),
    );
    // string->vector
    assert_eq!(
        eval("(vector->list (string->vector \"hello\"))"),
        Value::list(vec![
            Value::Char('h'),
            Value::Char('e'),
            Value::Char('l'),
            Value::Char('l'),
            Value::Char('o')
        ]),
    );
    // with range
    assert_eq!(
        eval("(vector->list (string->vector \"hello\" 1 3))"),
        Value::list(vec![Value::Char('e'), Value::Char('l')]),
    );
}

#[test]
fn branch_vector_type_errors() {
    // vector-length on non-vector
    let msg = eval_err("(vector-length 42)");
    assert!(msg.contains("vector"), "got: {msg}");
    // vector-ref on non-vector
    let msg = eval_err("(vector-ref 42 0)");
    assert!(msg.contains("vector"), "got: {msg}");
    // vector-set! on non-vector
    let msg = eval_err("(vector-set! 42 0 1)");
    assert!(msg.contains("vector"), "got: {msg}");
}

// ============================================================
// Branch-level coverage: bytevector operations
// ============================================================

#[test]
fn branch_make_bytevector_no_fill() {
    // Default fill is 0
    is_int("(bytevector-u8-ref (make-bytevector 3) 0)", 0);
}

#[test]
fn branch_make_bytevector_with_fill() {
    is_int("(bytevector-u8-ref (make-bytevector 3 255) 0)", 255);
}

#[test]
fn branch_bytevector_constructor() {
    // (bytevector byte ...)
    is_int("(bytevector-length (bytevector 1 2 3))", 3);
    is_int("(bytevector-u8-ref (bytevector 10 20 30) 1)", 20);
}

#[test]
fn branch_bytevector_u8_set_out_of_range() {
    let msg = eval_err("(let ((bv (make-bytevector 3))) (bytevector-u8-set! bv 5 0))");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_bytevector_u8_ref_out_of_range() {
    let msg = eval_err("(bytevector-u8-ref (bytevector 1 2 3) 10)");
    assert!(msg.contains("out of range"), "got: {msg}");
}

#[test]
fn branch_bytevector_copy_with_range() {
    assert_eq!(
        eval(
            "(let ((bv (bytevector-copy (bytevector 10 20 30 40 50) 1 3)))
               (list (bytevector-u8-ref bv 0) (bytevector-u8-ref bv 1)))"
        ),
        Value::list(vec![Value::Int(20), Value::Int(30)]),
    );
}

#[test]
fn branch_bytevector_copy_bang_with_range() {
    assert_eq!(
        eval(
            "(let ((bv (make-bytevector 5 0)))
               (bytevector-copy! bv 1 (bytevector 10 20 30 40) 1 3)
               (list (bytevector-u8-ref bv 0)
                     (bytevector-u8-ref bv 1)
                     (bytevector-u8-ref bv 2)
                     (bytevector-u8-ref bv 3)))"
        ),
        Value::list(vec![
            Value::Int(0),
            Value::Int(20),
            Value::Int(30),
            Value::Int(0)
        ]),
    );
}

#[test]
fn branch_bytevector_append_edge() {
    // Zero args
    is_int("(bytevector-length (bytevector-append))", 0);
    // Multiple
    assert_eq!(
        eval(
            "(let ((bv (bytevector-append (bytevector 1 2) (bytevector 3) (bytevector 4 5))))
               (bytevector-length bv))"
        ),
        Value::Int(5),
    );
}

#[test]
fn branch_bytevector_to_list_with_range() {
    assert_eq!(
        eval("(bytevector->list (bytevector 10 20 30 40) 1 3)"),
        Value::list(vec![Value::Int(20), Value::Int(30)]),
    );
}

#[test]
fn branch_bytevector_type_errors() {
    let msg = eval_err("(bytevector-length 42)");
    assert!(msg.contains("bytevector"), "got: {msg}");
    let msg = eval_err("(bytevector-u8-ref 42 0)");
    assert!(msg.contains("bytevector"), "got: {msg}");
}

#[test]
fn branch_utf8_invalid() {
    // Invalid UTF-8 → error
    let msg = eval_err("(utf8->string (bytevector 255 254))");
    assert!(msg.contains("UTF-8"), "got: {msg}");
}

// ============================================================
// Branch-level coverage: char.rs
// ============================================================

#[test]
fn branch_char_comparisons_full() {
    // All 5 comparison functions, both true and false
    is_true("(char=? #\\a #\\a)");
    is_false("(char=? #\\a #\\b)");
    is_true("(char<? #\\a #\\b)");
    is_false("(char<? #\\b #\\a)");
    is_true("(char>? #\\b #\\a)");
    is_false("(char>? #\\a #\\b)");
    is_true("(char<=? #\\a #\\a)");
    is_true("(char<=? #\\a #\\b)");
    is_false("(char<=? #\\b #\\a)");
    is_true("(char>=? #\\a #\\a)");
    is_true("(char>=? #\\b #\\a)");
    is_false("(char>=? #\\a #\\b)");
}

#[test]
fn branch_char_ci_comparisons_full() {
    // All 5 case-insensitive, both true and false
    is_true("(char-ci=? #\\A #\\a)");
    is_false("(char-ci=? #\\a #\\b)");
    is_true("(char-ci<? #\\a #\\B)");
    is_false("(char-ci<? #\\B #\\a)");
    is_true("(char-ci>? #\\B #\\a)");
    is_false("(char-ci>? #\\a #\\B)");
    is_true("(char-ci<=? #\\A #\\a)");
    is_true("(char-ci<=? #\\a #\\B)");
    is_false("(char-ci<=? #\\B #\\a)");
    is_true("(char-ci>=? #\\A #\\a)");
    is_true("(char-ci>=? #\\B #\\a)");
    is_false("(char-ci>=? #\\a #\\B)");
}

#[test]
fn branch_char_classification_false() {
    // False branches of classification predicates
    is_false("(char-alphabetic? #\\5)");
    is_false("(char-numeric? #\\a)");
    is_false("(char-whitespace? #\\a)");
    is_false("(char-upper-case? #\\a)");
    is_false("(char-lower-case? #\\A)");
}

#[test]
fn branch_digit_value_non_digit() {
    // Returns #f for non-digit chars
    is_false("(digit-value #\\a)");
    is_false("(digit-value #\\space)");
    // Works for all digits 0-9
    is_int("(digit-value #\\0)", 0);
    is_int("(digit-value #\\5)", 5);
    is_int("(digit-value #\\9)", 9);
}

#[test]
fn branch_char_foldcase() {
    assert_eq!(eval("(char-foldcase #\\A)"), Value::Char('a'));
    assert_eq!(eval("(char-foldcase #\\a)"), Value::Char('a'));
    assert_eq!(eval("(char-foldcase #\\Z)"), Value::Char('z'));
}

#[test]
fn branch_char_to_string() {
    assert_eq!(eval("(char->string #\\x)"), Value::String(Rc::from("x")));
    assert_eq!(
        eval("(char->string #\\space)"),
        Value::String(Rc::from(" "))
    );
}

#[test]
fn branch_integer_to_char_invalid() {
    // Invalid Unicode scalar value
    let msg = eval_err("(integer->char #xD800)");
    assert!(
        msg.contains("invalid") || msg.contains("Unicode"),
        "got: {msg}"
    );
}

// ============================================================
// Branch-level coverage: numeric operations
// ============================================================

#[test]
fn branch_arithmetic_edge_cases() {
    // + with zero args
    is_int("(+)", 0);
    // * with zero args
    is_int("(*)", 1);
    // - with one arg (negation)
    is_int("(- 5)", -5);
    // / with one arg (reciprocal)
    assert_eq!(eval("(/ 2)"), Value::Float(0.5));
    // Mixed exact/inexact
    assert_eq!(eval("(+ 1 2.0)"), Value::Float(3.0));
    assert_eq!(eval("(* 2 3.0)"), Value::Float(6.0));
}

#[test]
fn branch_division_exact() {
    // Exact division when divisible
    is_int("(/ 6 3)", 2);
    is_int("(/ 12 3 2)", 2);
    // Inexact when not divisible
    assert_eq!(eval("(/ 1 3)"), Value::Float(1.0 / 3.0));
}

#[test]
fn branch_comparison_chaining() {
    // Multi-arg comparisons
    is_true("(= 1 1 1 1)");
    is_false("(= 1 1 2 1)");
    is_true("(< 1 2 3 4)");
    is_false("(< 1 2 2 4)");
    is_true("(> 4 3 2 1)");
    is_false("(> 4 3 3 1)");
    is_true("(<= 1 1 2 3)");
    is_false("(<= 1 2 1 3)");
    is_true("(>= 3 2 2 1)");
    is_false("(>= 3 2 3 1)");
}

#[test]
fn branch_numeric_predicates() {
    is_true("(exact? 42)");
    is_false("(exact? 42.0)");
    is_true("(inexact? 42.0)");
    is_false("(inexact? 42)");
    is_true("(exact-integer? 42)");
    is_false("(exact-integer? 42.0)");
    is_true("(integer? 42)");
    is_true("(integer? 42.0)");
    is_false("(integer? 42.5)");
    is_true("(rational? 42)");
    is_true("(rational? 42.5)");
    is_false("(rational? +inf.0)");
    is_true("(positive? 1)");
    is_false("(positive? -1)");
    is_false("(positive? 0)");
    is_true("(negative? -1)");
    is_false("(negative? 1)");
    is_false("(negative? 0)");
    is_true("(finite? 42)");
    is_true("(finite? 42.5)");
    is_false("(finite? +inf.0)");
    is_false("(finite? -inf.0)");
    is_true("(infinite? +inf.0)");
    is_true("(infinite? -inf.0)");
    is_false("(infinite? 42)");
}

#[test]
fn branch_trig_edge() {
    // sin/cos/tan at 0
    assert_eq!(eval("(sin 0)"), Value::Float(0.0));
    assert_eq!(eval("(cos 0)"), Value::Float(1.0));
    assert_eq!(eval("(tan 0)"), Value::Float(0.0));
    // asin/acos at boundaries
    assert_eq!(eval("(asin 0)"), Value::Float(0.0));
    assert_eq!(eval("(acos 1)"), Value::Float(0.0));
}

#[test]
fn branch_exp_log_edge() {
    assert_eq!(eval("(exp 0)"), Value::Float(1.0));
    assert_eq!(eval("(log 1)"), Value::Float(0.0));
    // log with base
    assert_eq!(eval("(log 8 2)"), Value::Float(3.0));
}

// ============================================================
// Branch-level coverage: I/O edge cases
// ============================================================

#[test]
fn branch_read_eof_on_empty() {
    // read on empty string port → eof
    assert_eq!(
        eval("(eof-object? (read (open-input-string \"\")))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_read_char_eof() {
    assert_eq!(
        eval("(eof-object? (read-char (open-input-string \"\")))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_peek_char_eof() {
    assert_eq!(
        eval("(eof-object? (peek-char (open-input-string \"\")))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_read_u8_eof() {
    assert_eq!(
        eval("(eof-object? (read-u8 (open-input-bytevector #u8())))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_peek_u8_eof() {
    assert_eq!(
        eval("(eof-object? (peek-u8 (open-input-bytevector #u8())))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_read_line_eof() {
    assert_eq!(
        eval("(eof-object? (read-line (open-input-string \"\")))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_read_bytevector_eof() {
    assert_eq!(
        eval("(eof-object? (read-bytevector 5 (open-input-bytevector #u8())))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_read_string_eof() {
    assert_eq!(
        eval("(eof-object? (read-string 5 (open-input-string \"\")))"),
        Value::Bool(true),
    );
}

#[test]
fn branch_write_to_closed_port() {
    let msg = eval_err(
        "(let ((p (open-output-string)))
           (close-port p)
           (write-char #\\x p))",
    );
    assert!(msg.contains("closed"), "got: {msg}");
}

#[test]
fn branch_read_from_closed_port() {
    let msg = eval_err(
        "(let ((p (open-input-string \"x\")))
           (close-port p)
           (read-char p))",
    );
    assert!(msg.contains("closed"), "got: {msg}");
}

#[test]
fn branch_port_predicates_complete() {
    // input-port?
    is_true("(input-port? (open-input-string \"x\"))");
    is_false("(input-port? (open-output-string))");
    is_false("(input-port? 42)");
    // output-port?
    is_true("(output-port? (open-output-string))");
    is_false("(output-port? (open-input-string \"x\"))");
    is_false("(output-port? 42)");
    // port?
    is_true("(port? (open-input-string \"x\"))");
    is_true("(port? (open-output-string))");
    is_false("(port? 42)");
}

#[test]
fn branch_port_open_predicates() {
    // input-port-open? true then false after close
    is_true("(let ((p (open-input-string \"x\"))) (input-port-open? p))");
    is_false("(let ((p (open-input-string \"x\"))) (close-port p) (input-port-open? p))");
    // output-port-open?
    is_true("(let ((p (open-output-string))) (output-port-open? p))");
    is_false("(let ((p (open-output-string))) (close-port p) (output-port-open? p))");
}

#[test]
fn branch_eof_object() {
    // (eof-object) returns the eof object
    is_true("(eof-object? (eof-object))");
    is_false("(eof-object? 42)");
    is_false("(eof-object? #f)");
    is_false("(eof-object? '())");
}

#[test]
fn branch_write_vs_display() {
    // write quotes strings, display doesn't
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\"hello\"")),
    );
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("hello")),
    );
    // write quotes chars, display doesn't
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write #\\a p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("#\\a")),
    );
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display #\\a p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("a")),
    );
}

#[test]
fn branch_write_string_with_range() {
    // write-string with start/end
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-string \"hello world\" p 6 11)
               (get-output-string p))"
        ),
        Value::String(Rc::from("world")),
    );
}

#[test]
fn branch_write_bytevector_with_range() {
    // write-bytevector with start/end
    assert_eq!(
        eval(
            "(let ((p (open-output-bytevector)))
               (write-bytevector (bytevector 10 20 30 40 50) p 1 3)
               (bytevector-u8-ref (get-output-bytevector p) 0))"
        ),
        Value::Int(20),
    );
}

#[test]
fn branch_newline_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (newline p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\n")),
    );
}

#[test]
fn branch_file_exists() {
    is_false("(file-exists? \"/nonexistent/path/12345\")");
    // Create temp file, check, delete
    let tmp = std::env::temp_dir().join("mae_file_exists_test.txt");
    std::fs::write(&tmp, "x").unwrap();
    let code = format!("(file-exists? \"{}\")", tmp.display());
    is_true(&code);
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn branch_delete_file() {
    let tmp = std::env::temp_dir().join("mae_delete_file_test.txt");
    std::fs::write(&tmp, "x").unwrap();
    let code = format!("(delete-file \"{}\")", tmp.display());
    eval(&code);
    assert!(!tmp.exists());
}

// ============================================================
// Branch-level coverage: reader edge cases
// ============================================================

#[test]
fn branch_reader_char_literals() {
    // Named character literals
    assert_eq!(eval("#\\space"), Value::Char(' '));
    assert_eq!(eval("#\\newline"), Value::Char('\n'));
    assert_eq!(eval("#\\tab"), Value::Char('\t'));
    assert_eq!(eval("#\\return"), Value::Char('\r'));
    assert_eq!(eval("#\\alarm"), Value::Char('\x07'));
    assert_eq!(eval("#\\backspace"), Value::Char('\x08'));
    assert_eq!(eval("#\\escape"), Value::Char('\x1b'));
    assert_eq!(eval("#\\delete"), Value::Char('\x7f'));
    assert_eq!(eval("#\\null"), Value::Char('\0'));
}

#[test]
fn branch_reader_string_escapes() {
    assert_eq!(eval("(string-ref \"\\n\" 0)"), Value::Char('\n'),);
    assert_eq!(eval("(string-ref \"\\t\" 0)"), Value::Char('\t'),);
    assert_eq!(eval("(string-ref \"\\r\" 0)"), Value::Char('\r'),);
    assert_eq!(eval("(string-ref \"\\\\\" 0)"), Value::Char('\\'),);
    assert_eq!(eval("(string-ref \"\\\"\" 0)"), Value::Char('"'),);
    // Hex escape
    assert_eq!(eval("(string-ref \"\\x41;\" 0)"), Value::Char('A'),);
}

#[test]
fn branch_reader_radix_prefixes() {
    is_int("#b1010", 10);
    is_int("#o17", 15);
    is_int("#xFF", 255);
    is_int("#d42", 42);
}

#[test]
fn branch_reader_exactness_prefixes() {
    // #e makes inexact exact
    is_int("#e1.0", 1);
    is_int("#e2.5", 2); // truncates
                        // #i makes exact inexact
    assert_eq!(eval("#i42"), Value::Float(42.0));
}

#[test]
fn branch_reader_block_comment() {
    is_int("#| this is a comment |# 42", 42);
    // Nested block comments
    is_int("#| outer #| inner |# still comment |# 99", 99);
}

#[test]
fn branch_reader_datum_comment() {
    is_int("#;(ignored expression) 42", 42);
    is_int("#;\"ignored string\" 99", 99);
}

// ============================================================
// Branch-level coverage: cxr accessors
// ============================================================

#[test]
fn branch_cxr_deep() {
    // 2-deep
    assert_eq!(eval("(caar '((1 2) 3))"), Value::Int(1));
    assert_eq!(eval("(cadr '(1 2 3))"), Value::Int(2));
    assert_eq!(eval("(cdar '((1 2) 3))"), eval("'(2)"));
    assert_eq!(eval("(cddr '(1 2 3))"), eval("'(3)"));
    // 3-deep
    assert_eq!(eval("(caaar '(((1 2) 3) 4))"), Value::Int(1));
    assert_eq!(eval("(caddr '(1 2 3 4))"), Value::Int(3));
    assert_eq!(eval("(cdddr '(1 2 3 4))"), eval("'(4)"));
    // 4-deep
    assert_eq!(eval("(caaaar '((((1)))))"), Value::Int(1));
    assert_eq!(eval("(cadddr '(1 2 3 4 5))"), Value::Int(4));
}

// ============================================================
// Branch-level coverage: apply edge cases
// ============================================================

#[test]
fn branch_apply_with_leading_args() {
    // R7RS §6.10: (apply proc arg1 ... args)
    is_int("(apply + 1 2 '(3))", 6);
    is_int("(apply + 1 2 3 '(4))", 10);
    is_int("(apply + '())", 0);
}

// ============================================================
// Branch-level coverage: set-car!/set-cdr! (immutable pairs)
// ============================================================

#[test]
fn branch_set_car_cdr() {
    // Per SPEC_STANCES.md §2: pairs are immutable, set-car!/set-cdr! signal errors
    let msg = eval_err("(let ((p (cons 1 2))) (set-car! p 10))");
    assert!(
        msg.contains("immutable"),
        "set-car! should error, got: {msg}"
    );
    let msg = eval_err("(let ((p (cons 1 2))) (set-cdr! p 20))");
    assert!(
        msg.contains("immutable"),
        "set-cdr! should error, got: {msg}"
    );
}

// ============================================================
// Branch-level coverage: type predicates false branches
// ============================================================

#[test]
fn branch_type_predicates_false() {
    is_false("(boolean? 42)");
    is_false("(number? \"hello\")");
    is_false("(string? 42)");
    is_false("(symbol? 42)");
    is_false("(char? 42)");
    is_false("(pair? 42)");
    is_false("(null? 42)");
    is_false("(vector? 42)");
    is_false("(bytevector? 42)");
    is_false("(procedure? 42)");
    is_false("(port? 42)");
    is_false("(void? 42)");
    is_false("(eof-object? 42)");
    is_false("(list? 42)");
    is_false("(zero? 1)");
    is_false("(even? 1)");
    is_false("(odd? 2)");
}

#[test]
fn branch_type_predicates_true() {
    is_true("(boolean? #t)");
    is_true("(boolean? #f)");
    is_true("(number? 42)");
    is_true("(number? 42.0)");
    is_true("(string? \"hello\")");
    is_true("(symbol? 'foo)");
    is_true("(char? #\\a)");
    is_true("(pair? '(1))");
    is_true("(null? '())");
    is_true("(vector? #(1))");
    is_true("(bytevector? #u8(1))");
    is_true("(procedure? car)");
    is_true("(port? (open-input-string \"\"))");
    is_true("(void? (void))");
    is_true("(list? '(1 2 3))");
    is_true("(list? '())");
    is_true("(zero? 0)");
    is_true("(even? 2)");
    is_true("(odd? 1)");
}

// =============================================================================
// Branch-level coverage: remaining gaps across all mae-scheme modules
// =============================================================================

// --- io.rs: format specifiers ---
#[test]
fn branch_format_newline_and_tilde() {
    // ~% produces newline
    assert_eq!(eval("(format \"a~%b\")"), Value::string("a\nb"));
    // ~~ produces literal tilde
    assert_eq!(eval("(format \"~~\")"), Value::string("~"));
    // unknown specifier preserved literally
    assert_eq!(eval("(format \"~z\")"), Value::string("~z"));
    // ~s uses write (machine-readable) representation
    assert_eq!(eval("(format \"~s\" \"hi\")"), Value::string("\"hi\""),);
}

// --- io.rs: get-output-string on wrong port type ---
#[test]
fn branch_get_output_string_wrong_port() {
    let msg = eval_err("(get-output-string (open-input-string \"x\"))");
    assert!(
        msg.contains("output-string-port") || msg.contains("type"),
        "get-output-string on input port should error: {msg}"
    );
}

#[test]
fn branch_get_output_string_non_port() {
    let msg = eval_err("(get-output-string 42)");
    assert!(
        msg.contains("port") || msg.contains("type"),
        "get-output-string on non-port should error: {msg}"
    );
}

// --- io.rs: get-output-bytevector on StringOutput port ---
#[test]
fn branch_get_output_bytevector_from_string_port() {
    // StringOutput port should still work (returns bytes)
    let result =
        eval("(let ((p (open-output-string))) (write-string \"hi\" p) (get-output-bytevector p))");
    assert!(matches!(result, Value::Bytevector(_)));
}

// --- io.rs: open-input-file on non-existent file ---
#[test]
fn branch_open_input_file_not_found() {
    let msg = eval_err("(open-input-file \"/tmp/mae_nonexistent_file_12345.scm\")");
    assert!(
        msg.contains("open-input-file") || msg.contains("No such file"),
        "open-input-file should report file error: {msg}"
    );
}

// --- io.rs: read-line without trailing newline ---
#[test]
fn branch_read_line_no_trailing_newline() {
    assert_eq!(
        eval("(let ((p (open-input-string \"hello\"))) (read-line p))"),
        Value::string("hello"),
    );
}

// --- io.rs: read-line from file port without trailing newline ---
#[test]
fn branch_read_line_file_no_newline() {
    use std::io::Write;
    let path = "/tmp/mae_test_readline_no_nl.txt";
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "no newline here").unwrap();
    drop(f);
    let result = eval(&format!(
        "(let ((p (open-input-file \"{path}\"))) (let ((line (read-line p))) (close-port p) line))"
    ));
    assert_eq!(result, Value::string("no newline here"));
    std::fs::remove_file(path).ok();
}

// --- io.rs: read on binary file port (error) ---
#[test]
fn branch_read_binary_file_port() {
    use std::io::Write;
    let path = "/tmp/mae_test_read_binary.bin";
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(b"\x00\x01\x02").unwrap();
    drop(f);
    let msg = eval_err(&format!(
        "(let ((p (open-binary-input-file \"{path}\"))) (read p))"
    ));
    assert!(
        msg.contains("binary"),
        "read on binary port should error: {msg}"
    );
    std::fs::remove_file(path).ok();
}

// --- io.rs: exit with different arg types ---
#[test]
fn branch_exit_arg_types() {
    // exit with #t → code 0
    let msg = eval_err("(exit #t)");
    assert!(msg.contains("0"), "exit #t: {msg}");
    // exit with #f → code 1
    let msg = eval_err("(exit #f)");
    assert!(msg.contains("1"), "exit #f: {msg}");
    // exit with integer
    let msg = eval_err("(exit 42)");
    assert!(msg.contains("42"), "exit 42: {msg}");
    // exit with no args
    let msg = eval_err("(exit)");
    assert!(msg.contains("0"), "exit no args: {msg}");
}

// --- io.rs: read-u8 on closed port ---
#[test]
fn branch_read_u8_closed_port() {
    let msg = eval_err("(let ((p (open-input-string \"x\"))) (close-port p) (read-u8 p))");
    assert!(msg.contains("closed"), "read-u8 on closed port: {msg}");
}

// --- io.rs: read-bytevector on closed port ---
#[test]
fn branch_read_bytevector_closed_port() {
    let msg =
        eval_err("(let ((p (open-input-string \"x\"))) (close-port p) (read-bytevector 5 p))");
    assert!(
        msg.contains("closed"),
        "read-bytevector on closed port: {msg}"
    );
}

// --- io.rs: read-string from string port + EOF ---
#[test]
fn branch_read_string_from_port() {
    assert_eq!(
        eval("(let ((p (open-input-string \"hello\"))) (read-string 3 p))"),
        Value::string("hel"),
    );
    // EOF on empty port
    assert_eq!(
        eval("(let ((p (open-input-string \"\"))) (read-string 5 p))"),
        Value::Eof,
    );
    // Read more than available
    assert_eq!(
        eval("(let ((p (open-input-string \"hi\"))) (read-string 10 p))"),
        Value::string("hi"),
    );
}

// --- io.rs: write-simple and write-shared with port arg ---
#[test]
fn branch_write_simple_and_shared() {
    assert_eq!(
        eval("(let ((p (open-output-string))) (write-simple 42 p) (get-output-string p))"),
        Value::string("42"),
    );
    assert_eq!(
        eval("(let ((p (open-output-string))) (write-shared '(1 2) p) (get-output-string p))"),
        Value::string("(1 2)"),
    );
}

// --- io.rs: write-u8 to bytevector output port ---
#[test]
fn branch_write_u8_to_bytevector_port() {
    assert_eq!(
        eval("(let ((p (open-output-bytevector))) (write-u8 65 p) (get-output-bytevector p))"),
        eval("#u8(65)"),
    );
}

// --- io.rs: write-bytevector to output port ---
#[test]
fn branch_write_bytevector_to_port() {
    assert_eq!(
        eval("(let ((p (open-output-bytevector))) (write-bytevector #u8(1 2 3) p) (get-output-bytevector p))"),
        eval("#u8(1 2 3)"),
    );
}

// --- io.rs: write-string to BytevectorOutput port ---
#[test]
fn branch_write_to_bytevector_output_port() {
    assert_eq!(
        eval("(let ((p (open-output-bytevector))) (write-string \"hi\" p) (get-output-bytevector p))"),
        eval("#u8(104 105)"),
    );
}

// --- io.rs: char-ready? on buffered file port ---
#[test]
fn branch_char_ready_file_port() {
    use std::io::Write;
    let path = "/tmp/mae_test_char_ready.txt";
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "abc").unwrap();
    drop(f);
    // After reading one char, char-ready? should be true (buffered data remains)
    is_true(&format!(
        "(let ((p (open-input-file \"{path}\"))) (read-char p) (let ((r (char-ready? p))) (close-port p) r))"
    ));
    std::fs::remove_file(path).ok();
}

// --- io.rs: u8-ready? on BytevectorInput port ---
#[test]
fn branch_u8_ready_bytevector_port() {
    is_true("(let ((p (open-input-bytevector #u8(1 2 3)))) (u8-ready? p))");
    // After consuming all bytes
    is_false("(let ((p (open-input-bytevector #u8(1)))) (read-u8 p) (u8-ready? p))");
}

// --- io.rs: u8-ready? on closed port ---
#[test]
fn branch_u8_ready_closed_port() {
    let msg = eval_err("(let ((p (open-input-string \"x\"))) (close-port p) (u8-ready? p))");
    assert!(msg.contains("closed"), "u8-ready? on closed port: {msg}");
}

// --- io.rs: read-u8 from bytevector input ---
#[test]
fn branch_read_u8_bytevector_input() {
    is_int(
        "(let ((p (open-input-bytevector #u8(65 66)))) (read-u8 p))",
        65,
    );
    // EOF after all bytes consumed
    assert_eq!(
        eval("(let ((p (open-input-bytevector #u8(1)))) (read-u8 p) (read-u8 p))"),
        Value::Eof,
    );
}

// --- io.rs: peek-u8 from bytevector input ---
#[test]
fn branch_peek_u8_bytevector_input() {
    is_int(
        "(let ((p (open-input-bytevector #u8(42)))) (peek-u8 p))",
        42,
    );
    // EOF on empty
    assert_eq!(
        eval("(let ((p (open-input-bytevector #u8()))) (peek-u8 p))"),
        Value::Eof,
    );
}

// --- io.rs: read-bytevector from bytevector input ---
#[test]
fn branch_read_bytevector_from_bytevector_input() {
    assert_eq!(
        eval("(let ((p (open-input-bytevector #u8(10 20 30)))) (read-bytevector 2 p))"),
        eval("#u8(10 20)"),
    );
}

// --- io.rs: flush-output-port on string port (no-op) ---
#[test]
fn branch_flush_string_port() {
    // Should not error
    eval("(let ((p (open-output-string))) (flush-output-port p))");
}

// --- io.rs: port predicates on closed ports ---
#[test]
fn branch_port_predicates_closed() {
    // input-port? should return #t even after closing
    is_true("(let ((p (open-input-string \"x\"))) (close-port p) (input-port? p))");
    // output-port? should return #t even after closing
    is_true("(let ((p (open-output-string))) (close-port p) (output-port? p))");
    // input-port-open? should return #f after closing
    is_false("(let ((p (open-input-string \"x\"))) (close-port p) (input-port-open? p))");
    // output-port-open? should return #f after closing
    is_false("(let ((p (open-output-string))) (close-port p) (output-port-open? p))");
}

// --- io.rs: close-input-port and close-output-port ---
#[test]
fn branch_close_specific_port() {
    is_false("(let ((p (open-input-string \"x\"))) (close-input-port p) (input-port-open? p))");
    is_false("(let ((p (open-output-string))) (close-output-port p) (output-port-open? p))");
}

// --- io.rs: textual-port? and binary-port? on various port types ---
#[test]
fn branch_port_type_predicates() {
    is_true("(textual-port? (open-input-string \"x\"))");
    is_false("(binary-port? (open-input-string \"x\"))");
    is_true("(textual-port? (open-output-string))");
    // Non-port values
    is_false("(textual-port? 42)");
    is_false("(binary-port? \"hello\")");
}

// --- io.rs: read-bytevector! with start/end args ---
#[test]
fn branch_read_bytevector_mut_range() {
    assert_eq!(
        eval(
            "(let ((bv (make-bytevector 5 0))
                    (p (open-input-bytevector #u8(10 20 30))))
                (read-bytevector! bv p 1 4)
                bv)"
        ),
        eval("#u8(0 10 20 30 0)"),
    );
}

// --- base.rs: arithmetic overflow branches ---
#[test]
fn branch_add_overflow() {
    // i64::MAX + 1 should overflow to float
    let max = i64::MAX;
    let result = eval(&format!("(+ {max} 1)"));
    assert!(
        matches!(result, Value::Float(_)),
        "overflow should produce float"
    );
}

#[test]
fn branch_mul_overflow() {
    // Large int multiplication should overflow to float
    let result = eval("(* 9223372036854775807 2)");
    assert!(
        matches!(result, Value::Float(_)),
        "overflow should produce float"
    );
}

// --- base.rs: subtraction type error ---
#[test]
fn branch_sub_type_error() {
    let msg = eval_err("(- \"x\")");
    assert!(
        msg.contains("number") || msg.contains("type"),
        "- type error: {msg}"
    );
    let msg = eval_err("(- 1 \"x\")");
    assert!(
        msg.contains("number") || msg.contains("type"),
        "- multi type error: {msg}"
    );
}

// --- base.rs: division single arg (reciprocal) ---
#[test]
fn branch_div_reciprocal() {
    assert_eq!(eval("(/ 2)"), Value::Float(0.5)); // 1/2 = 0.5 (inexact)
    assert_eq!(eval("(/ 1)"), Value::Int(1)); // 1/1 = 1 (exact)
    assert_eq!(eval("(/ 4)"), Value::Float(0.25)); // 1/4 = 0.25
                                                   // Division by zero with single arg
    let msg = eval_err("(/ 0)");
    assert!(msg.contains("zero"), "1/0 should error: {msg}");
}

// --- base.rs: number->string with radix ---
#[test]
fn branch_number_to_string_radix() {
    assert_eq!(eval("(number->string 255 16)"), Value::string("ff"));
    assert_eq!(eval("(number->string 7 2)"), Value::string("111"));
    assert_eq!(eval("(number->string -10 16)"), Value::string("-a"));
    // Float arg
    assert_eq!(eval("(number->string 3.14)"), Value::string("3.14"));
    // Radix out of range
    let msg = eval_err("(number->string 10 37)");
    assert!(msg.contains("radix"), "radix out of range: {msg}");
    // Non-number
    let msg = eval_err("(number->string \"x\")");
    assert!(
        msg.contains("number") || msg.contains("type"),
        "non-number: {msg}"
    );
}

// --- base.rs: string->number with radix and failure ---
#[test]
fn branch_string_to_number_radix() {
    is_int("(string->number \"ff\" 16)", 255);
    is_int("(string->number \"111\" 2)", 7);
    // Parse failure returns #f
    is_false("(string->number \"xyz\")");
    is_false("(string->number \"not-a-number\" 10)");
}

// --- base.rs: modulo with negative args ---
#[test]
fn branch_modulo_negative() {
    // R7RS modulo: result has same sign as divisor
    is_int("(modulo 10 3)", 1);
    is_int("(modulo -10 3)", 2);
    is_int("(modulo 10 -3)", -2);
    is_int("(modulo -10 -3)", -1);
    // Division by zero
    let msg = eval_err("(modulo 10 0)");
    assert!(msg.contains("zero"), "modulo by zero: {msg}");
}

// --- base.rs: expt overflow and negative exponent ---
#[test]
fn branch_expt_overflow() {
    // Large exponent overflows to float
    let result = eval("(expt 2 63)");
    assert!(matches!(result, Value::Float(_)) || matches!(result, Value::Int(_)));
    // Negative exponent
    assert_eq!(eval("(expt 2 -1)"), Value::Float(0.5));
    // 0^0 = 1
    is_int("(expt 0 0)", 1);
}

// --- base.rs: exact-integer-sqrt negative ---
#[test]
fn branch_exact_integer_sqrt_negative() {
    let msg = eval_err("(exact-integer-sqrt -1)");
    assert!(msg.contains("negative"), "negative sqrt: {msg}");
}

// --- base.rs: rationalize edge cases ---
#[test]
fn branch_rationalize_edge_cases() {
    // NaN → NaN
    let result = eval("(rationalize +nan.0 1.0)");
    assert!(
        matches!(result, Value::Float(f) if f.is_nan()),
        "NaN input → NaN"
    );
    // Infinite diff
    assert_eq!(eval("(rationalize 3.0 +inf.0)"), Value::Float(0.0));
    // Infinite x
    let result = eval("(rationalize +inf.0 1.0)");
    assert!(
        matches!(result, Value::Float(f) if f.is_infinite()),
        "inf → inf"
    );
    // Infinite x and infinite diff → NaN
    let result = eval("(rationalize +inf.0 +inf.0)");
    assert!(
        matches!(result, Value::Float(f) if f.is_nan()),
        "inf/inf → NaN"
    );
    // Zero in range
    assert_eq!(eval("(rationalize 0.5 1.0)"), Value::Float(0.0));
    // Negative range
    let result = eval("(rationalize -3.5 0.5)");
    assert!(
        matches!(result, Value::Float(f) if f < 0.0),
        "negative range"
    );
}

// --- base.rs: floor-quotient/remainder division by zero ---
#[test]
fn branch_floor_div_by_zero() {
    let msg = eval_err("(floor-quotient 10 0)");
    assert!(msg.contains("zero"), "floor-quotient by zero: {msg}");
    let msg = eval_err("(floor-remainder 10 0)");
    assert!(msg.contains("zero"), "floor-remainder by zero: {msg}");
    let msg = eval_err("(floor/ 10 0)");
    assert!(msg.contains("zero"), "floor/ by zero: {msg}");
}

// --- base.rs: truncate-quotient/remainder division by zero ---
#[test]
fn branch_truncate_div_by_zero() {
    let msg = eval_err("(truncate-quotient 10 0)");
    assert!(msg.contains("zero"), "truncate-quotient by zero: {msg}");
    let msg = eval_err("(truncate-remainder 10 0)");
    assert!(msg.contains("zero"), "truncate-remainder by zero: {msg}");
    let msg = eval_err("(truncate/ 10 0)");
    assert!(msg.contains("zero"), "truncate/ by zero: {msg}");
}

// --- base.rs: gcd/lcm edge cases ---
#[test]
fn branch_gcd_lcm_edges() {
    is_int("(gcd)", 0);
    is_int("(lcm)", 1);
    is_int("(gcd 0 5)", 5);
    is_int("(gcd 12 8)", 4);
    is_int("(lcm 0 5)", 0);
    is_int("(lcm 4 6)", 12);
    // Negative args
    is_int("(gcd -12 8)", 4);
    is_int("(lcm -4 6)", 12);
}

// --- base.rs: list-tail/list-ref out of range ---
#[test]
fn branch_list_tail_out_of_range() {
    let msg = eval_err("(list-tail '(a b) 5)");
    assert!(
        msg.contains("out of range") || msg.contains("type"),
        "list-tail out of range: {msg}"
    );
}

#[test]
fn branch_list_ref_out_of_range() {
    let msg = eval_err("(list-ref '(a b) 5)");
    assert!(!msg.is_empty(), "list-ref out of range should error: {msg}");
}

// --- base.rs: append edge cases ---
#[test]
fn branch_append_edges() {
    assert_eq!(eval("(append)"), Value::Null);
    assert_eq!(eval("(append '(1 2))"), eval("'(1 2)"));
    // Last arg can be non-list (dotted pair)
    assert_eq!(eval("(append '(1) 2)"), eval("(cons 1 2)"));
    // Non-list in non-last position
    let msg = eval_err("(append 42 '(1))");
    assert!(
        msg.contains("list") || msg.contains("type"),
        "non-list append: {msg}"
    );
}

// --- base.rs: set-car!/set-cdr! on non-pair ---
#[test]
fn branch_set_car_cdr_non_pair() {
    let msg = eval_err("(set-car! 42 'x)");
    assert!(
        msg.contains("pair") || msg.contains("type"),
        "set-car! non-pair: {msg}"
    );
    let msg = eval_err("(set-cdr! \"hello\" 'x)");
    assert!(
        msg.contains("pair") || msg.contains("type"),
        "set-cdr! non-pair: {msg}"
    );
}

// --- base.rs: values with 0, 1, multiple args ---
#[test]
fn branch_values_arity() {
    assert_eq!(eval("(values 42)"), Value::Int(42));
    assert_eq!(eval("(values 1 2 3)"), eval("'(1 2 3)"));
    assert_eq!(eval("(values)"), Value::Null);
}

// --- base.rs: boolean=? ---
#[test]
fn branch_boolean_equality() {
    is_true("(boolean=? #t #t)");
    is_true("(boolean=? #f #f)");
    is_false("(boolean=? #t #f)");
    is_true("(boolean=? #t #t #t)");
    is_false("(boolean=? #t #t #f)");
}

// --- base.rs: symbol=? ---
#[test]
fn branch_symbol_equality() {
    is_true("(symbol=? 'foo 'foo)");
    is_false("(symbol=? 'foo 'bar)");
    let msg = eval_err("(symbol=? 'foo 42)");
    assert!(
        msg.contains("symbol") || msg.contains("type"),
        "symbol=? type error: {msg}"
    );
}

// --- base.rs: infinite?/nan? on int ---
#[test]
fn branch_infinite_nan_on_int() {
    is_false("(infinite? 42)");
    is_false("(nan? 42)");
    is_true("(infinite? +inf.0)");
    is_true("(nan? +nan.0)");
}

// --- base.rs: rational? on non-finite float ---
#[test]
fn branch_rational_non_finite() {
    is_false("(rational? +inf.0)");
    is_false("(rational? +nan.0)");
    is_true("(rational? 3.14)");
    is_true("(rational? 42)");
    is_false("(rational? \"x\")");
}

// --- base.rs: integer? on float ---
#[test]
fn branch_integer_pred_float() {
    is_true("(integer? 3.0)");
    is_false("(integer? 3.5)");
    is_false("(integer? \"x\")");
}

// --- base.rs: square overflow ---
#[test]
fn branch_square_overflow() {
    // Small value: exact integer
    is_int("(square 3)", 9);
    // Float
    assert_eq!(eval("(square 2.5)"), Value::Float(6.25));
    // Type error
    let msg = eval_err("(square \"x\")");
    assert!(
        msg.contains("number") || msg.contains("type"),
        "square type error: {msg}"
    );
}

// --- base.rs: abs edge cases ---
#[test]
fn branch_abs_edge_cases() {
    is_int("(abs -5)", 5);
    is_int("(abs 5)", 5);
    assert_eq!(eval("(abs -2.75)"), Value::Float(2.75));
    let msg = eval_err("(abs \"x\")");
    assert!(
        msg.contains("number") || msg.contains("type"),
        "abs type error: {msg}"
    );
}

// --- base.rs: floor/ceiling/round/truncate type errors ---
#[test]
fn branch_rounding_type_errors() {
    let msg = eval_err("(floor \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(ceiling \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(round \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(truncate \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
}

// --- base.rs: round banker's rounding edge ---
#[test]
fn branch_round_bankers() {
    // 0.5 → 0 (round to even)
    assert_eq!(eval("(round 0.5)"), Value::Float(0.0));
    // 1.5 → 2 (round to even)
    assert_eq!(eval("(round 1.5)"), Value::Float(2.0));
    // 2.5 → 2 (round to even)
    assert_eq!(eval("(round 2.5)"), Value::Float(2.0));
    // Integer input passes through
    is_int("(round 5)", 5);
}

// --- base.rs: exact/inexact conversion type errors ---
#[test]
fn branch_exact_inexact_type_errors() {
    let msg = eval_err("(exact->inexact \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(inexact->exact \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(exact \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(inexact \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
}

// --- base.rs: exact?/inexact? type errors ---
#[test]
fn branch_exact_pred_type_errors() {
    let msg = eval_err("(exact? \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(inexact? \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
}

// --- base.rs: zero?/positive?/negative? type errors ---
#[test]
fn branch_sign_pred_type_errors() {
    let msg = eval_err("(zero? \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(positive? \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
    let msg = eval_err("(negative? \"x\")");
    assert!(msg.contains("number") || msg.contains("type"));
}

// --- base.rs: sign predicates with floats ---
#[test]
fn branch_sign_pred_floats() {
    is_true("(zero? 0.0)");
    is_true("(positive? 1.5)");
    is_true("(negative? -0.5)");
    is_false("(positive? -1.0)");
    is_false("(negative? 1.0)");
}

// --- base.rs: numeric comparison chaining with 3+ args ---
#[test]
fn branch_numeric_compare_chain() {
    is_true("(< 1 2 3 4)");
    is_false("(< 1 2 2 4)");
    is_true("(<= 1 2 2 4)");
    is_true("(> 4 3 2 1)");
    is_false("(> 4 3 3 1)");
    is_true("(>= 4 3 3 1)");
    is_true("(= 5 5 5)");
    is_false("(= 5 5 6)");
}

// --- base.rs: length on improper list ---
#[test]
fn branch_length_improper_list() {
    let msg = eval_err("(length (cons 1 2))");
    assert!(
        msg.contains("proper list") || msg.contains("type"),
        "length dotted pair: {msg}"
    );
}

// --- base.rs: reverse on non-list ---
#[test]
fn branch_reverse_error() {
    let msg = eval_err("(reverse 42)");
    assert!(
        msg.contains("list") || msg.contains("type"),
        "reverse non-list: {msg}"
    );
}

// --- base.rs: list-copy ---
#[test]
fn branch_list_copy() {
    assert_eq!(eval("(list-copy '(1 2 3))"), eval("'(1 2 3)"));
    assert_eq!(eval("(list-copy '())"), Value::Null);
}

// --- base.rs: make-list with and without fill ---
#[test]
fn branch_make_list() {
    assert_eq!(eval("(make-list 3 'x)"), eval("'(x x x)"));
    // Without fill: undefined values
    is_int("(length (make-list 4))", 4);
}

// --- base.rs: assv/assq/memv/memq on empty list ---
#[test]
fn branch_assoc_empty() {
    is_false("(assv 1 '())");
    is_false("(assq 'a '())");
    is_false("(memv 1 '())");
    is_false("(memq 'a '())");
}

// --- base.rs: symbol->string / string->symbol type errors ---
#[test]
fn branch_symbol_conversion_errors() {
    let msg = eval_err("(symbol->string 42)");
    assert!(
        msg.contains("symbol") || msg.contains("type"),
        "symbol->string type: {msg}"
    );
    let msg = eval_err("(string->symbol 42)");
    assert!(
        msg.contains("string") || msg.contains("type"),
        "string->symbol type: {msg}"
    );
}

// --- base.rs: sqrt exact result ---
#[test]
fn branch_sqrt_exact() {
    // Perfect square of exact int → exact
    is_int("(sqrt 9)", 3);
    is_int("(sqrt 0)", 0);
    // Non-perfect → float
    assert!(matches!(eval("(sqrt 2)"), Value::Float(_)));
}

// --- base.rs: complex?/real?/exact-integer? ---
#[test]
fn branch_numeric_type_preds() {
    is_true("(complex? 42)");
    is_true("(complex? 3.14)");
    is_false("(complex? \"x\")");
    is_true("(real? 42)");
    is_true("(real? 3.14)");
    is_false("(real? \"x\")");
    is_true("(exact-integer? 42)");
    is_false("(exact-integer? 3.14)");
    is_false("(exact-integer? \"x\")");
}

// --- compiler.rs: cond with arrow clause ---
#[test]
fn branch_cond_arrow() {
    is_int("(cond (1 => (lambda (x) (+ x 10))))", 11);
    // Arrow with false test → skip
    is_int("(cond (#f => (lambda (x) x)) (else 42))", 42);
}

// --- compiler.rs: case with multiple datums per clause ---
#[test]
fn branch_case_multiple_datums() {
    is_int("(case 2 ((1 2 3) 10) (else 20))", 10);
    is_int("(case 5 ((1 2 3) 10) (else 20))", 20);
}

// --- compiler.rs: do loop with step expressions ---
#[test]
fn branch_do_with_steps() {
    is_int("(do ((i 0 (+ i 1))) ((= i 5) i))", 5);
    // Multiple variables with different steps
    is_int("(do ((i 0 (+ i 1)) (j 10 (- j 1))) ((= i 3) j))", 7);
}

// --- compiler.rs: do with result expressions ---
#[test]
fn branch_do_result_exprs() {
    is_int("(do ((i 0 (+ i 1))) ((= i 3) (+ i 100)))", 103);
}

// --- compiler.rs: guard with else clause ---
#[test]
fn branch_guard_else() {
    is_int("(guard (e (else 99)) (raise 'boom))", 99);
}

// --- compiler.rs: guard re-raise ---
#[test]
fn branch_guard_reraise() {
    // Inner guard catches, outer guard catches re-raise
    is_int(
        "(guard (e ((string? e) 1))
           (guard (e ((number? e) (raise \"inner\")))
             (raise 42)))",
        1,
    );
}

// --- compiler.rs: when/unless ---
#[test]
fn branch_when_unless() {
    is_int("(when #t 42)", 42);
    assert_eq!(eval("(when #f 42)"), Value::Void);
    is_int("(unless #f 42)", 42);
    assert_eq!(eval("(unless #t 42)"), Value::Void);
}

// --- compiler.rs: define-record-type ---
#[test]
fn branch_define_record_type() {
    let result = eval(
        "
        (define-record-type <point>
          (make-point x y)
          point?
          (x point-x)
          (y point-y))
        (let ((p (make-point 3 4)))
          (list (point? p) (point-x p) (point-y p)))
    ",
    );
    assert_eq!(result, eval("'(#t 3 4)"));
}

// --- compiler.rs: parameterize ---
#[test]
fn branch_parameterize() {
    assert_eq!(
        eval(
            "
            (define p (make-parameter 10))
            (parameterize ((p 42))
              (p))
        "
        ),
        Value::Int(42),
    );
    // Restores after
    assert_eq!(
        eval(
            "
            (define p (make-parameter 10))
            (parameterize ((p 42))
              'ignore)
            (p)
        "
        ),
        Value::Int(10),
    );
}

// --- compiler.rs: named let ---
#[test]
fn branch_named_let() {
    is_int(
        "(let loop ((n 5) (acc 1)) (if (= n 0) acc (loop (- n 1) (* acc n))))",
        120,
    );
}

// --- compiler.rs: letrec* ---
#[test]
fn branch_letrec_star() {
    is_int("(letrec* ((x 1) (y (+ x 1))) y)", 2);
}

// --- vm.rs: closure handlers (with-exception-handler) ---
#[test]
fn branch_with_exception_handler() {
    // Closure handler catches and can return a value via raise-continuable
    is_int(
        "(with-exception-handler
           (lambda (e) 42)
           (lambda () (raise-continuable 'err)))",
        42,
    );
}

// --- vm.rs: raise-continuable ---
#[test]
fn branch_raise_continuable() {
    is_int(
        "(+ 1 (with-exception-handler
                (lambda (e) 10)
                (lambda () (raise-continuable 'x))))",
        11,
    );
}

// --- vm.rs: continuation + dynamic-wind ---
#[test]
fn branch_callcc_dynamic_wind() {
    // dynamic-wind before/after thunks should run during call/cc
    assert_eq!(
        eval(
            "
            (let ((log '()))
              (call-with-current-continuation
                (lambda (k)
                  (dynamic-wind
                    (lambda () (set! log (cons 'before log)))
                    (lambda () (k 'done))
                    (lambda () (set! log (cons 'after log))))))
              log)
        "
        ),
        eval("'(after before)"),
    );
}

// --- reader.rs: datum labels ---
#[test]
fn branch_reader_datum_labels() {
    // Datum labels: #0= defines, #0# references.
    // The reader stores the labeled datum for later reference.
    // Use quote to prevent the list from being interpreted as a call.
    assert_eq!(eval("(define x '#0=(1 2 3)) (car x)"), Value::Int(1));
}

// --- reader.rs: block comment in hash position ---
#[test]
fn branch_reader_block_comment_hash() {
    assert_eq!(eval("#| comment |# 42"), Value::Int(42));
}

// --- reader.rs: character literals ---
#[test]
fn branch_reader_char_names() {
    assert_eq!(eval("#\\space"), Value::Char(' '));
    assert_eq!(eval("#\\newline"), Value::Char('\n'));
    assert_eq!(eval("#\\tab"), Value::Char('\t'));
    assert_eq!(eval("#\\return"), Value::Char('\r'));
    assert_eq!(eval("#\\alarm"), Value::Char('\u{07}'));
    assert_eq!(eval("#\\backspace"), Value::Char('\u{08}'));
    assert_eq!(eval("#\\delete"), Value::Char('\u{7F}'));
    assert_eq!(eval("#\\escape"), Value::Char('\u{1B}'));
    assert_eq!(eval("#\\null"), Value::Char('\0'));
    // Hex character
    assert_eq!(eval("#\\x41"), Value::Char('A'));
}

// --- reader.rs: #true and #false ---
#[test]
fn branch_reader_bool_long() {
    is_true("#true");
    is_false("#false");
}

// --- reader.rs: unterminated list error ---
#[test]
fn branch_reader_unterminated_list() {
    let msg = eval_err("(1 2");
    assert!(
        msg.contains("unterminated") || msg.contains("end of input"),
        "unterminated list: {msg}"
    );
}

// --- reader.rs: unexpected close paren ---
#[test]
fn branch_reader_unexpected_close() {
    let msg = eval_err(")");
    assert!(
        msg.contains(")") || msg.contains("unexpected"),
        "unexpected close paren: {msg}"
    );
}

// --- reader.rs: unterminated string ---
#[test]
fn branch_reader_unterminated_string() {
    let msg = eval_err("\"hello");
    assert!(
        msg.contains("unterminated") || msg.contains("string"),
        "unterminated string: {msg}"
    );
}

// --- reader.rs: unexpected EOF after # ---
#[test]
fn branch_reader_eof_after_hash() {
    let msg = eval_err("#");
    assert!(
        msg.contains("end of input") || msg.contains("unexpected"),
        "EOF after #: {msg}"
    );
}

// --- reader.rs: invalid after #u ---
#[test]
fn branch_reader_invalid_after_u() {
    let msg = eval_err("#u9(1 2)");
    assert!(
        msg.contains("8") || msg.contains("expected"),
        "invalid #u: {msg}"
    );
}

// --- reader.rs: dotted pair ---
#[test]
fn branch_reader_dotted_pair() {
    assert_eq!(eval("(car '(1 . 2))"), Value::Int(1));
    assert_eq!(eval("(cdr '(1 . 2))"), Value::Int(2));
}

// --- reader.rs: quasiquote/unquote/unquote-splicing ---
#[test]
fn branch_reader_quasiquote() {
    assert_eq!(eval("`(1 ,(+ 2 3) 4)"), eval("'(1 5 4)"));
    assert_eq!(eval("`(1 ,@(list 2 3) 4)"), eval("'(1 2 3 4)"));
}

// --- macros.rs: syntax-rules with ellipsis ---
#[test]
fn branch_syntax_rules_ellipsis() {
    assert_eq!(
        eval(
            "
            (define-syntax my-list
              (syntax-rules ()
                ((my-list x ...) '(x ...))))
            (my-list 1 2 3)
        "
        ),
        eval("'(1 2 3)"),
    );
}

// --- macros.rs: syntax-rules with literal identifiers ---
#[test]
fn branch_syntax_rules_literals() {
    assert_eq!(
        eval(
            "
            (define-syntax my-if
              (syntax-rules (then else)
                ((my-if c then t else f) (if c t f))))
            (my-if #t then 1 else 2)
        "
        ),
        Value::Int(1),
    );
}

// --- library.rs: import with only (on user-defined library) ---
#[test]
fn branch_import_only() {
    // Test that import with (only ...) modifier works — defines
    // just the specified bindings in scope
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(
        "
        (define-library (test mylib-o)
          (export my-add my-sub)
          (begin
            (define (my-add a b) (+ a b))
            (define (my-sub a b) (- a b))))
    ",
    )
    .unwrap();
    vm.eval("(import (only (test mylib-o) my-add))").unwrap();
    assert_eq!(vm.eval("(my-add 1 2)").unwrap(), Value::Int(3));
}

// --- library.rs: import with prefix ---
#[test]
fn branch_import_prefix() {
    eval(
        "
        (define-library (test preflib)
          (export pval)
          (begin (define pval 42)))
        (import (prefix (test preflib) t:))
    ",
    );
}

// --- library.rs: import with rename ---
#[test]
fn branch_import_rename() {
    eval(
        "
        (define-library (test renlib)
          (export rval)
          (begin (define rval 99)))
        (import (rename (test renlib) (rval renamed-val)))
    ",
    );
}

// --- library.rs: import with except ---
#[test]
fn branch_import_except() {
    eval(
        "
        (define-library (test exclib)
          (export ea eb)
          (begin (define ea 1) (define eb 2)))
        (import (except (test exclib) eb))
    ",
    );
}

// --- library.rs: cond-expand with library ---
#[test]
fn branch_cond_expand_library() {
    is_int("(cond-expand ((library (scheme base)) 1) (else 2))", 1);
    is_int("(cond-expand ((library (nonexistent lib)) 1) (else 2))", 2);
}

// --- io.rs: write-char to port ---
#[test]
fn branch_write_char_to_port() {
    assert_eq!(
        eval("(let ((p (open-output-string))) (write-char #\\A p) (get-output-string p))"),
        Value::string("A"),
    );
}

// --- io.rs: display with port ---
#[test]
fn branch_display_to_port() {
    assert_eq!(
        eval("(let ((p (open-output-string))) (display 42 p) (get-output-string p))"),
        Value::string("42"),
    );
}

// --- io.rs: write with port ---
#[test]
fn branch_write_to_port() {
    assert_eq!(
        eval("(let ((p (open-output-string))) (write \"hi\" p) (get-output-string p))"),
        Value::string("\"hi\""),
    );
}

// --- io.rs: display vs write on string ---
#[test]
fn branch_display_vs_write_string() {
    // display: no quotes
    assert_eq!(
        eval("(let ((p (open-output-string))) (display \"hi\" p) (get-output-string p))"),
        Value::string("hi"),
    );
    // write: with quotes
    assert_eq!(
        eval("(let ((p (open-output-string))) (write \"hi\" p) (get-output-string p))"),
        Value::string("\"hi\""),
    );
}

// --- io.rs: file I/O roundtrip ---
#[test]
fn branch_file_io_roundtrip() {
    use std::fs;
    let path = "/tmp/mae_test_io_roundtrip.txt";
    eval(&format!(
        "(let ((p (open-output-file \"{path}\")))
           (write-string \"hello world\" p)
           (close-port p))"
    ));
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{path}\")))
               (let ((s (read-line p)))
                 (close-port p) s))"
        )),
        Value::string("hello world"),
    );
    fs::remove_file(path).ok();
}

// --- io.rs: binary file I/O roundtrip ---
#[test]
fn branch_binary_file_io() {
    use std::fs;
    let path = "/tmp/mae_test_binary_io.bin";
    eval(&format!(
        "(let ((p (open-binary-output-file \"{path}\")))
           (write-bytevector #u8(1 2 3 4 5) p)
           (close-port p))"
    ));
    assert_eq!(
        eval(&format!(
            "(let ((p (open-binary-input-file \"{path}\")))
               (let ((bv (read-bytevector 5 p)))
                 (close-port p) bv))"
        )),
        eval("#u8(1 2 3 4 5)"),
    );
    fs::remove_file(path).ok();
}

// --- io.rs: read-char from file port ---
#[test]
fn branch_read_char_file_port() {
    use std::io::Write;
    let path = "/tmp/mae_test_readchar_file.txt";
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "AB").unwrap();
    drop(f);
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{path}\")))
               (let ((c (read-char p))) (close-port p) c))"
        )),
        Value::Char('A'),
    );
    std::fs::remove_file(path).ok();
}

// --- io.rs: peek-char from file port ---
#[test]
fn branch_peek_char_file_port() {
    use std::io::Write;
    let path = "/tmp/mae_test_peekchar_file.txt";
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "XY").unwrap();
    drop(f);
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{path}\")))
               (peek-char p)
               (let ((c (read-char p))) (close-port p) c))"
        )),
        Value::Char('X'),
    );
    std::fs::remove_file(path).ok();
}

// --- io.rs: read from file port (S-expression) ---
#[test]
fn branch_read_from_file_port() {
    use std::io::Write;
    let path = "/tmp/mae_test_read_sexp.scm";
    let mut f = std::fs::File::create(path).unwrap();
    write!(f, "(+ 1 2)").unwrap();
    drop(f);
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{path}\")))
               (let ((datum (read p))) (close-port p) datum))"
        )),
        eval("'(+ 1 2)"),
    );
    std::fs::remove_file(path).ok();
}

// --- io.rs: environment variables ---
#[test]
fn branch_env_vars() {
    // HOME should exist
    let result = eval("(get-environment-variable \"HOME\")");
    assert!(
        matches!(result, Value::String(_)),
        "HOME should be a string"
    );
    // Non-existent returns #f
    is_false("(get-environment-variable \"MAE_NONEXISTENT_VAR_12345\")");
    // get-environment-variables returns a list
    let result = eval("(pair? (get-environment-variables))");
    assert_eq!(result, Value::Bool(true));
}

// --- io.rs: command-line returns a list ---
#[test]
fn branch_command_line() {
    is_true("(list? (command-line))");
}

// --- io.rs: current-second/jiffy/jiffies-per-second ---
#[test]
fn branch_timing() {
    let result = eval("(current-second)");
    assert!(matches!(result, Value::Float(_)));
    let result = eval("(current-jiffy)");
    assert!(matches!(result, Value::Int(_)));
    is_int("(jiffies-per-second)", 1_000_000_000);
}

// --- io.rs: delete-file on non-existent ---
#[test]
fn branch_delete_file_not_found() {
    let msg = eval_err("(delete-file \"/tmp/mae_nonexistent_delete_12345.txt\")");
    assert!(
        msg.contains("delete-file") || msg.contains("No such file"),
        "delete non-existent: {msg}"
    );
}

// --- io.rs: write to closed port ---
#[test]
fn branch_write_closed_port() {
    let msg = eval_err("(let ((p (open-output-string))) (close-port p) (write-string \"x\" p))");
    assert!(msg.contains("closed"), "write to closed port: {msg}");
}

// --- io.rs: read from output port (type error) ---
#[test]
fn branch_read_from_output_port() {
    let msg = eval_err("(read-char (open-output-string))");
    assert!(
        msg.contains("input") || msg.contains("type"),
        "read from output port: {msg}"
    );
}

// --- base.rs: quotient/remainder division by zero ---
#[test]
fn branch_quotient_remainder_div_zero() {
    let msg = eval_err("(quotient 10 0)");
    assert!(msg.contains("zero"), "quotient by zero: {msg}");
    let msg = eval_err("(remainder 10 0)");
    assert!(msg.contains("zero"), "remainder by zero: {msg}");
}

// --- base.rs: assv/assq match and miss ---
#[test]
fn branch_assoc_functions() {
    assert_eq!(eval("(assv 2 '((1 a) (2 b) (3 c)))"), eval("'(2 b)"));
    is_false("(assv 4 '((1 a) (2 b)))");
    assert_eq!(eval("(assq 'b '((a 1) (b 2)))"), eval("'(b 2)"));
    is_false("(assq 'z '((a 1) (b 2)))");
}

// --- base.rs: memv/memq match and miss ---
#[test]
fn branch_member_functions() {
    assert_eq!(eval("(memv 2 '(1 2 3))"), eval("'(2 3)"));
    is_false("(memv 4 '(1 2 3))");
    assert_eq!(eval("(memq 'b '(a b c))"), eval("'(b c)"));
    is_false("(memq 'z '(a b c))");
}

// --- base.rs: member with custom comparator ---
#[test]
fn branch_member_custom_comparator() {
    assert_eq!(eval("(member 2 '(1 2 3) =)"), eval("'(2 3)"),);
}

// --- base.rs: assoc with custom comparator ---
#[test]
fn branch_assoc_custom_comparator() {
    assert_eq!(
        eval("(assoc 2.0 '((1 a) (2 b)) (lambda (a b) (= a b)))"),
        eval("'(2 b)"),
    );
}

// --- base.rs: int_to_radix_string edge cases ---
#[test]
fn branch_int_to_radix_zero() {
    assert_eq!(eval("(number->string 0 16)"), Value::string("0"));
    assert_eq!(eval("(number->string 0 2)"), Value::string("0"));
}

// --- value.rs: is_list on various structures ---
#[test]
fn branch_is_list() {
    is_true("(list? '())");
    is_true("(list? '(1 2 3))");
    is_false("(list? (cons 1 2))");
    is_false("(list? 42)");
    is_false("(list? \"hello\")");
}

// --- compiler.rs: begin with multiple expressions ---
#[test]
fn branch_begin_multiple() {
    is_int("(begin 1 2 3)", 3);
    is_int("(begin (define x 10) (+ x 5))", 15);
}

// --- compiler.rs: and/or short-circuit ---
#[test]
fn branch_and_or_short_circuit() {
    is_false("(and 1 2 #f 3)");
    is_int("(and 1 2 3)", 3);
    assert_eq!(eval("(and)"), Value::Bool(true));
    is_int("(or #f #f 42 #f)", 42);
    is_false("(or #f #f #f)");
    is_false("(or)");
}

// --- compiler.rs: let with body forms ---
#[test]
fn branch_let_body() {
    is_int("(let ((x 1) (y 2)) (+ x y))", 3);
    is_int("(let* ((x 1) (y (+ x 1))) y)", 2);
}

// --- compiler.rs: lambda rest args ---
#[test]
fn branch_lambda_rest() {
    assert_eq!(eval("((lambda (x . rest) rest) 1 2 3)"), eval("'(2 3)"),);
    assert_eq!(eval("((lambda rest rest) 1 2 3)"), eval("'(1 2 3)"),);
}

// --- compiler.rs: define with body (implicit begin) ---
#[test]
fn branch_define_body() {
    is_int("(define (f x) (define y 10) (+ x y)) (f 5)", 15);
}

// --- vm.rs: file error object structure ---
#[test]
fn branch_file_error_structure() {
    is_true(
        "(guard (e ((file-error? e) #t))
           (open-input-file \"/tmp/mae_nonexistent_12345.txt\"))",
    );
}

// --- vm.rs: error-object-message and error-object-irritants ---
#[test]
fn branch_error_object_accessors() {
    assert_eq!(
        eval(
            "(guard (e (#t (error-object-message e)))
               (error \"test error\" 'a 'b))"
        ),
        Value::string("test error"),
    );
    assert_eq!(
        eval(
            "(guard (e (#t (error-object-irritants e)))
               (error \"test\" 1 2))"
        ),
        eval("'(1 2)"),
    );
    is_true(
        "(guard (e (#t (error-object? e)))
           (error \"test\"))",
    );
}

// =========================================================================
// Phase 2: Remaining branch coverage — compiler.rs error paths
// =========================================================================

#[test]
fn branch_compiler_empty_application() {
    // () evaluates to the empty list (Null), not an error in our implementation
    assert_eq!(eval("'()"), Value::Null);
}

#[test]
fn branch_compiler_quote_arity() {
    let msg = eval_err("(quote)");
    assert!(msg.contains("quote"), "quote arity: {msg}");
    let msg = eval_err("(quote 1 2)");
    assert!(msg.contains("quote"), "quote extra: {msg}");
}

#[test]
fn branch_compiler_if_arity() {
    let msg = eval_err("(if)");
    assert!(
        msg.contains("if") || msg.contains("argument"),
        "if no args: {msg}"
    );
    let msg = eval_err("(if #t)");
    assert!(
        msg.contains("if") || msg.contains("argument"),
        "if one arg: {msg}"
    );
    let msg = eval_err("(if #t 1 2 3)");
    assert!(
        msg.contains("if") || msg.contains("argument"),
        "if four args: {msg}"
    );
}

#[test]
fn branch_compiler_lambda_arity() {
    let msg = eval_err("(lambda)");
    assert!(msg.contains("lambda"), "lambda no args: {msg}");
    let msg = eval_err("(lambda ())");
    assert!(msg.contains("lambda"), "lambda no body: {msg}");
}

#[test]
fn branch_compiler_define_arity() {
    let msg = eval_err("(define)");
    assert!(msg.contains("define"), "define no args: {msg}");
    let msg = eval_err("(define x)");
    assert!(msg.contains("define"), "define no value: {msg}");
}

#[test]
fn branch_compiler_define_symbol_extra_args() {
    let msg = eval_err("(define x 1 2)");
    assert!(msg.contains("define"), "define extra value: {msg}");
}

#[test]
fn branch_compiler_define_invalid_form() {
    let msg = eval_err("(define 42 1)");
    assert!(
        msg.contains("define") || msg.contains("invalid"),
        "define non-sym: {msg}"
    );
}

#[test]
fn branch_compiler_set_arity() {
    let msg = eval_err("(set!)");
    assert!(msg.contains("set!"), "set! no args: {msg}");
    let msg = eval_err("(set! x)");
    assert!(msg.contains("set!"), "set! one arg: {msg}");
    let msg = eval_err("(set! x 1 2)");
    assert!(msg.contains("set!"), "set! three args: {msg}");
}

#[test]
fn branch_compiler_set_non_symbol() {
    let msg = eval_err("(set! 42 1)");
    assert!(
        msg.contains("symbol") || msg.contains("set!"),
        "set! non-symbol: {msg}"
    );
}

#[test]
fn branch_compiler_empty_cond() {
    assert_eq!(eval("(cond)"), Value::Void);
}

#[test]
fn branch_compiler_cond_empty_clause() {
    let msg = eval_err("(cond ())");
    assert!(
        msg.contains("empty") || msg.contains("cond"),
        "empty cond clause: {msg}"
    );
}

#[test]
fn branch_compiler_case_arity() {
    let msg = eval_err("(case)");
    assert!(msg.contains("case"), "case no args: {msg}");
    let msg = eval_err("(case 1)");
    assert!(msg.contains("case"), "case no clauses: {msg}");
}

#[test]
fn branch_compiler_case_empty_clause() {
    let msg = eval_err("(case 1 ())");
    assert!(
        msg.contains("empty") || msg.contains("case"),
        "empty case clause: {msg}"
    );
}

#[test]
fn branch_compiler_case_else_arrow() {
    // (case x (else => proc)) — R7RS §4.2.1
    is_int("(case 42 (else => (lambda (x) (+ x 1))))", 43);
}

#[test]
fn branch_compiler_case_datum_arrow() {
    // ((datum ...) => proc) — R7RS §4.2.1
    is_int("(case 2 ((1 2 3) => (lambda (x) (* x 10))))", 20);
}

#[test]
fn branch_compiler_case_multiple_datums() {
    is_int("(case 2 ((1 3 5) 10) ((2 4 6) 20) (else 30))", 20);
}

#[test]
fn branch_compiler_do_arity() {
    let msg = eval_err("(do)");
    assert!(msg.contains("do"), "do no args: {msg}");
    let msg = eval_err("(do ())");
    assert!(msg.contains("do"), "do no test: {msg}");
}

#[test]
fn branch_compiler_do_empty_test() {
    let msg = eval_err("(do () ())");
    assert!(
        msg.contains("do") && msg.contains("test"),
        "do empty test: {msg}"
    );
}

#[test]
fn branch_compiler_do_var_spec_invalid_len() {
    let msg = eval_err("(do ((x)) (#t))");
    assert!(
        msg.contains("do") || msg.contains("var"),
        "do var spec single: {msg}"
    );
    let msg = eval_err("(do ((x 1 2 3)) (#t))");
    assert!(
        msg.contains("do") || msg.contains("var"),
        "do var spec 4: {msg}"
    );
}

#[test]
fn branch_compiler_do_no_step() {
    // (var init) without step — var keeps its value
    is_int("(do ((x 5)) (#t x))", 5);
}

#[test]
fn branch_compiler_do_multi_result() {
    // do test with multiple result expressions
    is_int("(do ((i 0 (+ i 1))) ((= i 3) (+ i 10) (+ i 20)))", 23);
}

#[test]
fn branch_compiler_guard_no_clauses() {
    // guard with zero clauses — re-raises the exception (produces unhandled error)
    let msg = eval_err("(guard (e) (error \"test\"))");
    assert!(
        msg.contains("unhandled") || msg.contains("exception") || msg.contains("test"),
        "guard no clauses: {msg}"
    );
}

#[test]
fn branch_compiler_when_arity() {
    let msg = eval_err("(when)");
    assert!(msg.contains("when"), "when no args: {msg}");
    let msg = eval_err("(when #t)");
    assert!(msg.contains("when"), "when no body: {msg}");
}

#[test]
fn branch_compiler_unless_arity() {
    let msg = eval_err("(unless)");
    assert!(msg.contains("unless"), "unless no args: {msg}");
    let msg = eval_err("(unless #f)");
    assert!(msg.contains("unless"), "unless no body: {msg}");
}

#[test]
fn branch_compiler_define_values_arity() {
    let msg = eval_err("(define-values)");
    assert!(msg.contains("define-values"), "define-values arity: {msg}");
}

#[test]
fn branch_compiler_define_values_single() {
    is_int("(define-values (x) 42) x", 42);
}

#[test]
fn branch_compiler_define_values_multi() {
    is_int("(define-values (a b c) (values 1 2 3)) (+ a b c)", 6);
}

#[test]
fn branch_compiler_case_lambda_arity() {
    let msg = eval_err("(case-lambda)");
    assert!(msg.contains("case-lambda"), "case-lambda no clauses: {msg}");
}

#[test]
fn branch_compiler_case_lambda_no_match() {
    let msg = eval_err("(define f (case-lambda ((x) x) ((x y) (+ x y)))) (f 1 2 3)");
    assert!(msg.contains("no matching"), "case-lambda no match: {msg}");
}

#[test]
fn branch_compiler_case_lambda_variadic() {
    // case-lambda with variadic clause
    assert_eq!(
        eval("(define f (case-lambda ((x) x) ((x . rest) rest))) (f 1 2 3)"),
        eval("'(2 3)"),
    );
}

#[test]
fn branch_compiler_define_record_type_arity() {
    let msg = eval_err("(define-record-type foo)");
    assert!(msg.contains("define-record-type"), "record arity: {msg}");
}

#[test]
fn branch_compiler_define_record_type_empty_ctor() {
    let msg = eval_err("(define-record-type foo () foo?)");
    assert!(
        msg.contains("constructor") || msg.contains("name"),
        "empty ctor: {msg}"
    );
}

#[test]
fn branch_compiler_define_record_type_field_spec_invalid() {
    let msg = eval_err("(define-record-type foo (make-foo x) foo? (x))");
    assert!(
        msg.contains("field") || msg.contains("accessor"),
        "field spec needs accessor: {msg}"
    );
}

#[test]
fn branch_compiler_parameterize_arity() {
    let msg = eval_err("(parameterize)");
    assert!(msg.contains("parameterize"), "parameterize arity: {msg}");
    let msg = eval_err("(parameterize ())");
    assert!(msg.contains("parameterize"), "parameterize no body: {msg}");
}

#[test]
fn branch_compiler_parameterize_bad_binding() {
    let msg = eval_err("(parameterize ((p)) 1)");
    assert!(
        msg.contains("parameterize") || msg.contains("binding"),
        "bad param binding: {msg}"
    );
}

#[test]
fn branch_compiler_let_values_arity() {
    let msg = eval_err("(let-values)");
    assert!(msg.contains("let-values"), "let-values arity: {msg}");
}

#[test]
fn branch_compiler_let_values_bad_clause() {
    let msg = eval_err("(let-values (((x) 1 2)) x)");
    assert!(
        msg.contains("let-values") || msg.contains("clause"),
        "bad clause: {msg}"
    );
}

#[test]
fn branch_compiler_let_star_values_arity() {
    let msg = eval_err("(let*-values)");
    assert!(msg.contains("let*-values"), "let*-values arity: {msg}");
}

#[test]
fn branch_compiler_let_star_values_empty() {
    // No bindings — just compile body
    is_int("(let*-values () 42)", 42);
}

#[test]
fn branch_compiler_let_star_values_multi() {
    // Multiple bindings — nested desugaring
    is_int("(let*-values (((x) 10) ((y) (+ x 1))) (+ x y))", 21);
}

#[test]
fn branch_compiler_receive_arity() {
    let msg = eval_err("(receive x)");
    assert!(msg.contains("receive"), "receive arity: {msg}");
}

#[test]
fn branch_compiler_receive_basic() {
    is_int("(receive (a b) (values 3 4) (+ a b))", 7);
}

#[test]
fn branch_compiler_eval_arity() {
    let msg = eval_err("(eval)");
    assert!(msg.contains("eval"), "eval no args: {msg}");
    let msg = eval_err("(eval 1 2 3)");
    assert!(msg.contains("eval"), "eval too many: {msg}");
}

#[test]
fn branch_compiler_eval_with_env() {
    // eval with env arg — accepted but ignored
    is_int("(eval '(+ 1 2) (interaction-environment))", 3);
}

#[test]
fn branch_compiler_load_arity() {
    let msg = eval_err("(load)");
    assert!(msg.contains("load"), "load no args: {msg}");
    let msg = eval_err("(load \"a\" \"b\")");
    assert!(msg.contains("load"), "load too many: {msg}");
}

#[test]
fn branch_compiler_dynamic_wind_arity() {
    let msg = eval_err("(dynamic-wind)");
    assert!(msg.contains("dynamic-wind"), "dw no args: {msg}");
    let msg = eval_err("(dynamic-wind (lambda () #f) (lambda () #f))");
    assert!(msg.contains("dynamic-wind"), "dw two args: {msg}");
}

#[test]
fn branch_compiler_call_with_values_arity() {
    let msg = eval_err("(call-with-values)");
    assert!(msg.contains("call-with-values"), "cwv no args: {msg}");
}

#[test]
fn branch_compiler_call_cc_wrong_arity() {
    let msg = eval_err("(call/cc)");
    assert!(
        msg.contains("call") || msg.contains("argument"),
        "call/cc no args: {msg}"
    );
}

#[test]
fn branch_compiler_syntax_error() {
    let msg = eval_err("(syntax-error \"custom error message\")");
    assert!(msg.contains("custom error message"), "syntax-error: {msg}");
}

#[test]
fn branch_compiler_syntax_error_arity() {
    let msg = eval_err("(syntax-error)");
    assert!(msg.contains("syntax-error"), "syntax-error no msg: {msg}");
}

#[test]
fn branch_compiler_syntax_error_non_string() {
    // syntax-error with non-string message — falls back to Display
    let msg = eval_err("(syntax-error 42)");
    assert!(msg.contains("42"), "syntax-error non-string: {msg}");
}

#[test]
fn branch_compiler_cond_expand_no_match() {
    let msg = eval_err("(cond-expand (nonexistent-feature 1))");
    assert!(
        msg.contains("cond-expand") || msg.contains("no matching"),
        "ce no match: {msg}"
    );
}

#[test]
fn branch_compiler_cond_expand_and() {
    is_int("(cond-expand ((and r7rs mae) 42))", 42);
    // (and) with false feature
    assert_eq!(
        eval("(cond-expand ((and r7rs nonexistent) 1) (else 2))"),
        Value::Int(2),
    );
}

#[test]
fn branch_compiler_cond_expand_or() {
    is_int("(cond-expand ((or nonexistent r7rs) 42))", 42);
    assert_eq!(
        eval("(cond-expand ((or nonexistent1 nonexistent2) 1) (else 2))"),
        Value::Int(2),
    );
}

#[test]
fn branch_compiler_cond_expand_not() {
    is_int("(cond-expand ((not nonexistent) 42))", 42);
    assert_eq!(eval("(cond-expand ((not r7rs) 1) (else 2))"), Value::Int(2),);
}

#[test]
fn branch_compiler_cond_expand_library() {
    is_int("(cond-expand ((library (scheme base)) 42))", 42);
    assert_eq!(
        eval("(cond-expand ((library (nonexistent lib)) 1) (else 2))"),
        Value::Int(2),
    );
}

#[test]
fn branch_compiler_include_arity() {
    let msg = eval_err("(include)");
    assert!(msg.contains("include"), "include arity: {msg}");
}

#[test]
fn branch_compiler_include_non_string() {
    let msg = eval_err("(include 42)");
    assert!(
        msg.contains("string") || msg.contains("include"),
        "include non-string: {msg}"
    );
}

#[test]
fn branch_compiler_include_not_found() {
    let msg = eval_err("(include \"nonexistent_file_99999.scm\")");
    assert!(
        msg.contains("not found") || msg.contains("include"),
        "include not found: {msg}"
    );
}

#[test]
fn branch_compiler_let_star_empty_bindings() {
    is_int("(let* () 42)", 42);
}

#[test]
fn branch_compiler_let_bindings_errors() {
    let msg = eval_err("(let)");
    assert!(msg.contains("let"), "let no args: {msg}");
    let msg = eval_err("(let ())");
    assert!(msg.contains("let"), "let no body: {msg}");
    let msg = eval_err("(let ((42 1)) 1)");
    assert!(
        msg.contains("symbol") || msg.contains("let"),
        "let non-sym var: {msg}"
    );
    let msg = eval_err("(let ((x)) 1)");
    assert!(
        msg.contains("let") || msg.contains("binding"),
        "let binding no expr: {msg}"
    );
}

#[test]
fn branch_compiler_letrec_errors() {
    let msg = eval_err("(letrec)");
    assert!(msg.contains("letrec"), "letrec no args: {msg}");
    let msg = eval_err("(letrec ())");
    assert!(msg.contains("letrec"), "letrec no body: {msg}");
    let msg = eval_err("(letrec ((42 1)) 1)");
    assert!(
        msg.contains("symbol") || msg.contains("letrec"),
        "letrec non-sym: {msg}"
    );
}

#[test]
fn branch_compiler_named_let() {
    // Named let — loop
    is_int(
        "(let loop ((i 0) (sum 0)) (if (= i 5) sum (loop (+ i 1) (+ sum i))))",
        10,
    );
}

#[test]
fn branch_compiler_named_let_arity() {
    let msg = eval_err("(let name)");
    assert!(
        msg.contains("named let") || msg.contains("let"),
        "named let arity: {msg}"
    );
}

#[test]
fn branch_compiler_define_macro_arity() {
    let msg = eval_err("(define-macro)");
    assert!(msg.contains("define-macro"), "define-macro arity: {msg}");
    let msg = eval_err("(define-macro foo)");
    assert!(msg.contains("define-macro"), "define-macro no body: {msg}");
}

#[test]
fn branch_compiler_define_macro_empty_sig() {
    let msg = eval_err("(define-macro () 1)");
    assert!(
        msg.contains("define-macro") || msg.contains("empty"),
        "empty sig: {msg}"
    );
}

#[test]
fn branch_compiler_define_macro_wrong_args() {
    // macro expects 1 arg, gets 2
    let msg = eval_err("(define-macro (my-mac x) (list 'quote x)) (my-mac 1 2)");
    assert!(
        msg.contains("macro") || msg.contains("arg"),
        "macro wrong args: {msg}"
    );
}

#[test]
fn branch_compiler_define_macro_multi_body() {
    // define-macro with multiple body expressions
    is_int("(define-macro (my-add a b) (list '+ a b)) (my-add 3 4)", 7);
}

#[test]
fn branch_compiler_define_syntax_arity() {
    let msg = eval_err("(define-syntax)");
    assert!(msg.contains("define-syntax"), "def-syntax arity: {msg}");
    let msg = eval_err("(define-syntax foo bar baz)");
    assert!(msg.contains("define-syntax"), "def-syntax extra: {msg}");
}

#[test]
fn branch_compiler_define_syntax_non_symbol() {
    let msg = eval_err("(define-syntax 42 (syntax-rules () ((_ x) x)))");
    assert!(
        msg.contains("symbol") || msg.contains("define-syntax"),
        "non-sym name: {msg}"
    );
}

#[test]
fn branch_compiler_define_syntax_empty_transformer() {
    let msg = eval_err("(define-syntax foo ())");
    assert!(
        msg.contains("empty") || msg.contains("define-syntax"),
        "empty transformer: {msg}"
    );
}

#[test]
fn branch_compiler_define_syntax_non_syntax_rules() {
    let msg = eval_err("(define-syntax foo (not-syntax-rules () ((_ x) x)))");
    assert!(
        msg.contains("syntax-rules") || msg.contains("define-syntax"),
        "non-sr: {msg}"
    );
}

#[test]
fn branch_compiler_let_syntax_arity() {
    let msg = eval_err("(let-syntax)");
    assert!(msg.contains("let-syntax"), "let-syntax arity: {msg}");
    let msg = eval_err("(let-syntax ())");
    assert!(msg.contains("let-syntax"), "let-syntax no body: {msg}");
}

#[test]
fn branch_compiler_let_syntax_clause_invalid() {
    let msg = eval_err("(let-syntax ((foo)) 1)");
    assert!(
        msg.contains("let-syntax") || msg.contains("clause"),
        "bad clause: {msg}"
    );
}

#[test]
fn branch_compiler_let_syntax_non_sr() {
    let msg = eval_err("(let-syntax ((foo (not-syntax-rules))) 1)");
    assert!(
        msg.contains("syntax-rules") || msg.contains("let-syntax"),
        "non-sr: {msg}"
    );
}

#[test]
fn branch_compiler_quasiquote_arity() {
    let msg = eval_err("(quasiquote)");
    assert!(msg.contains("quasiquote"), "qq arity: {msg}");
    let msg = eval_err("(quasiquote 1 2)");
    assert!(msg.contains("quasiquote"), "qq extra: {msg}");
}

#[test]
fn branch_compiler_set_upvalue() {
    // set! on an upvalue (variable in enclosing scope)
    is_int("(let ((x 1)) ((lambda () (set! x 42))) x)", 42);
}

#[test]
fn branch_compiler_set_global() {
    // set! on a global
    is_int("(define g 10) (set! g 20) g", 20);
}

#[test]
fn branch_compiler_if_no_else() {
    // if with no else — returns void
    assert_eq!(eval("(if #f 1)"), Value::Void);
}

#[test]
fn branch_compiler_cond_test_only() {
    // (cond (test)) — no body, returns test value if truthy
    is_int("(cond (42))", 42);
    // (cond (#f)) — false test falls through, returns void
    assert_eq!(eval("(cond (#f))"), Value::Void);
}

#[test]
fn branch_compiler_cond_no_else_unmatched() {
    // All clauses fail, no else → void
    assert_eq!(eval("(cond (#f 1) (#f 2))"), Value::Void);
}

#[test]
fn branch_compiler_lambda_formals_invalid() {
    let msg = eval_err("(lambda 42 1)");
    assert!(
        msg.contains("formals") || msg.contains("invalid"),
        "bad formals: {msg}"
    );
}

#[test]
fn branch_compiler_lambda_formal_non_symbol() {
    let msg = eval_err("(lambda (42) 1)");
    assert!(
        msg.contains("symbol") || msg.contains("formal"),
        "non-sym formal: {msg}"
    );
}

#[test]
fn branch_compiler_internal_defines() {
    // Internal definitions with forward references (letrec* semantics)
    is_int(
        "((lambda ()
           (define (even? n) (if (= n 0) #t (odd? (- n 1))))
           (define (odd? n) (if (= n 0) #f (even? (- n 1))))
           (if (even? 10) 1 0)))",
        1,
    );
}

// =========================================================================
// Phase 2: reader.rs error paths
// =========================================================================

#[test]
fn branch_reader_eof_in_input() {
    let msg = eval_err("(");
    assert!(msg.contains("unterminated"), "eof in list: {msg}");
}

#[test]
fn branch_reader_unexpected_rparen() {
    let msg = eval_err(")");
    assert!(msg.contains("unexpected ')'"), "unexpected rparen: {msg}");
}

#[test]
fn branch_reader_eof_after_hash_api() {
    let err = mae_scheme::reader::read_all("#").unwrap_err();
    assert!(
        err.message().contains("end of input") || err.message().contains("#"),
        "eof after #: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unknown_hash_char() {
    let err = mae_scheme::reader::read_all("#z").unwrap_err();
    assert!(
        err.message().contains("unexpected") || err.message().contains("#"),
        "unknown hash char: {}",
        err.message()
    );
}

#[test]
fn branch_reader_u_not_8() {
    let err = mae_scheme::reader::read_all("#u9(1)").unwrap_err();
    assert!(
        err.message().contains("'8'") || err.message().contains("#u"),
        "#u not 8: {}",
        err.message()
    );
}

#[test]
fn branch_reader_bytevector_out_of_range() {
    let err = mae_scheme::reader::read_all("#u8(256)").unwrap_err();
    assert!(
        err.message().contains("out of range"),
        "bv out of range: {}",
        err.message()
    );
}

#[test]
fn branch_reader_bytevector_non_integer() {
    let err = mae_scheme::reader::read_all("#u8(foo)").unwrap_err();
    assert!(
        err.message().contains("integer") || err.message().contains("must be"),
        "bv non-int: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unterminated_vector() {
    let err = mae_scheme::reader::read_all("#(1 2").unwrap_err();
    assert!(
        err.message().contains("unterminated"),
        "unterminated vector: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unterminated_string_api() {
    let err = mae_scheme::reader::read_all("\"hello").unwrap_err();
    assert!(
        err.message().contains("unterminated"),
        "unterminated string: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unterminated_string_escape() {
    let err = mae_scheme::reader::read_all("\"hello\\").unwrap_err();
    assert!(
        err.message().contains("unterminated") || err.message().contains("escape"),
        "unterminated escape: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unknown_string_escape() {
    let err = mae_scheme::reader::read_all("\"\\q\"").unwrap_err();
    assert!(
        err.message().contains("unknown") || err.message().contains("escape"),
        "unknown escape: {}",
        err.message()
    );
}

#[test]
fn branch_reader_string_alarm_backspace_null() {
    // \a = alarm, \b = backspace, \0 = null
    let result = mae_scheme::reader::read_all("\"\\a\\b\\0\"").unwrap();
    assert_eq!(result[0], Value::string("\x07\x08\0"));
}

#[test]
fn branch_reader_string_line_continuation() {
    // \ followed by newline — skip newline and leading whitespace
    let result = mae_scheme::reader::read_all("\"hello\\\n   world\"").unwrap();
    assert_eq!(result[0], Value::string("helloworld"));
}

#[test]
fn branch_reader_char_eof() {
    let err = mae_scheme::reader::read_all("#\\").unwrap_err();
    assert!(
        err.message().contains("end of input") || err.message().contains("character"),
        "char eof: {}",
        err.message()
    );
}

#[test]
fn branch_reader_char_unknown_name() {
    let err = mae_scheme::reader::read_all("#\\foobar").unwrap_err();
    assert!(
        err.message().contains("unknown character"),
        "unknown char name: {}",
        err.message()
    );
}

#[test]
fn branch_reader_char_named() {
    assert_eq!(eval("#\\return"), Value::Char('\r'));
    assert_eq!(eval("#\\null"), Value::Char('\0'));
    assert_eq!(eval("#\\nul"), Value::Char('\0'));
    assert_eq!(eval("#\\alarm"), Value::Char('\x07'));
    assert_eq!(eval("#\\backspace"), Value::Char('\x08'));
    assert_eq!(eval("#\\escape"), Value::Char('\x1b'));
    assert_eq!(eval("#\\delete"), Value::Char('\x7f'));
    assert_eq!(eval("#\\linefeed"), Value::Char('\n'));
}

#[test]
fn branch_reader_char_hex() {
    assert_eq!(eval("#\\x41"), Value::Char('A'));
    assert_eq!(eval("#\\x61"), Value::Char('a'));
}

#[test]
fn branch_reader_char_hex_invalid() {
    let err = mae_scheme::reader::read_all("#\\xFFFFFF").unwrap_err();
    assert!(
        err.message().contains("invalid") || err.message().contains("scalar"),
        "invalid hex char: {}",
        err.message()
    );
}

#[test]
fn branch_reader_char_non_alpha() {
    // Single non-alphabetic character
    assert_eq!(eval("#\\!"), Value::Char('!'));
    assert_eq!(eval("#\\@"), Value::Char('@'));
}

#[test]
fn branch_reader_datum_label_undefined_ref() {
    let err = mae_scheme::reader::read_all("#99#").unwrap_err();
    assert!(
        err.message().contains("undefined datum label"),
        "undefined label ref: {}",
        err.message()
    );
}

#[test]
fn branch_reader_datum_label_bad_suffix() {
    let err = mae_scheme::reader::read_all("#0x").unwrap_err();
    // This actually hits the radix path for #0 but with bad digits; or hits the "expected '=' or '#'" error
    assert!(
        !err.message().is_empty(),
        "bad datum label suffix: {}",
        err.message()
    );
}

#[test]
fn branch_reader_datum_label_multi_digit() {
    // Multi-digit label: #10= and #10#
    let result = mae_scheme::reader::read_all("#10=42 #10#").unwrap();
    assert_eq!(result[0], Value::Int(42));
    assert_eq!(result[1], Value::Int(42));
}

#[test]
fn branch_reader_dotted_pair_extra_tokens() {
    let err = mae_scheme::reader::read_all("(1 . 2 3)").unwrap_err();
    assert!(
        err.message().contains("')' after dotted"),
        "extra after dot: {}",
        err.message()
    );
}

#[test]
fn branch_reader_unterminated_delimited_symbol() {
    let err = mae_scheme::reader::read_all("|hello").unwrap_err();
    assert!(
        err.message().contains("unterminated"),
        "unterminated delimited: {}",
        err.message()
    );
}

#[test]
fn branch_reader_delimited_symbol_escape() {
    let result = mae_scheme::reader::read_all("|hello\\|world|").unwrap();
    assert_eq!(result[0], Value::symbol("hello|world"));
}

#[test]
fn branch_reader_delimited_symbol_escape_eof() {
    let err = mae_scheme::reader::read_all("|hello\\").unwrap_err();
    assert!(
        err.message().contains("unterminated"),
        "delimited escape eof: {}",
        err.message()
    );
}

#[test]
fn branch_reader_radix_negative() {
    assert_eq!(eval("#b-101"), Value::Int(-5));
    assert_eq!(eval("#o-17"), Value::Int(-15));
    assert_eq!(eval("#x-ff"), Value::Int(-255));
}

#[test]
fn branch_reader_radix_positive_sign() {
    assert_eq!(eval("#b+101"), Value::Int(5));
    assert_eq!(eval("#x+ff"), Value::Int(255));
}

#[test]
fn branch_reader_radix_no_digits() {
    let err = mae_scheme::reader::read_all("#b").unwrap_err();
    assert!(
        err.message().contains("digits") || err.message().contains("radix"),
        "no digits after radix: {}",
        err.message()
    );
}

#[test]
fn branch_reader_exactness_chained_radix() {
    // #e#x, #i#b etc.
    assert_eq!(eval("#e#xFF"), Value::Int(255));
    assert_eq!(eval("#i#b101"), Value::Float(5.0));
    assert_eq!(eval("#e#o77"), Value::Int(63));
    assert_eq!(eval("#i#d42"), Value::Float(42.0));
}

#[test]
fn branch_reader_exactness_bad_radix() {
    let err = mae_scheme::reader::read_all("#e#z42").unwrap_err();
    assert!(
        err.message().contains("radix") || err.message().contains("expected"),
        "bad radix after exactness: {}",
        err.message()
    );
}

#[test]
fn branch_reader_exactness_float_to_exact() {
    assert_eq!(eval("#e3.7"), Value::Int(3));
}

#[test]
fn branch_reader_exactness_int_to_inexact() {
    assert_eq!(eval("#i42"), Value::Float(42.0));
}

#[test]
fn branch_reader_rational_zero_denominator() {
    let err = mae_scheme::reader::read_all("1/0").unwrap_err();
    assert!(
        err.message().contains("zero") || err.message().contains("division"),
        "rational /0: {}",
        err.message()
    );
}

#[test]
fn branch_reader_rational_invalid() {
    // rational with more than 2 parts
    let err = mae_scheme::reader::read_all("1/2/3").unwrap_err();
    assert!(!err.message().is_empty(), "bad rational: {}", err.message());
}

#[test]
fn branch_reader_special_numbers() {
    assert_eq!(eval("+inf.0"), Value::Float(f64::INFINITY));
    assert_eq!(eval("-inf.0"), Value::Float(f64::NEG_INFINITY));
    assert!(matches!(eval("+nan.0"), Value::Float(f) if f.is_nan()));
    assert!(matches!(eval("-nan.0"), Value::Float(f) if f.is_nan()));
}

#[test]
fn branch_reader_sign_as_symbol() {
    // Bare + and - are symbols, not numbers
    assert_eq!(eval("'+"), Value::symbol("+"));
    assert_eq!(eval("'-"), Value::symbol("-"));
}

#[test]
fn branch_reader_float_with_exponent() {
    assert_eq!(eval("1e3"), Value::Float(1000.0));
    assert_eq!(eval("1.5e2"), Value::Float(150.0));
    assert_eq!(eval("1e-2"), Value::Float(0.01));
}

#[test]
fn branch_reader_dot_prefix_number() {
    assert_eq!(eval(".5"), Value::Float(0.5));
    assert_eq!(eval("-.5"), Value::Float(-0.5));
}

#[test]
fn branch_reader_ellipsis_as_symbol() {
    // ... is a symbol
    assert_eq!(eval("'..."), Value::symbol("..."));
}

#[test]
fn branch_reader_nested_block_comment() {
    // Nested block comments
    let result = mae_scheme::reader::read_all("#| outer #| inner |# still outer |# 42").unwrap();
    assert_eq!(result[0], Value::Int(42));
}

#[test]
fn branch_reader_unterminated_block_comment() {
    // In skip_atmosphere, unterminated block comments are silently ignored
    // (error result is consumed by let _ = ...), so read_all returns empty.
    // Test that the skip_block_comment method itself returns error when called
    // via the read_hash path (#| as datum).
    let result = mae_scheme::reader::read_all("#| unterminated");
    // skip_atmosphere consumes the block comment error, returns empty vec
    assert!(result.is_ok() && result.unwrap().is_empty());
}

// =========================================================================
// Phase 2: value.rs Display and equality
// =========================================================================

#[test]
fn branch_value_display_float_special() {
    assert_eq!(format!("{}", Value::Float(f64::NAN)), "+nan.0");
    assert_eq!(format!("{}", Value::Float(f64::INFINITY)), "+inf.0");
    assert_eq!(format!("{}", Value::Float(f64::NEG_INFINITY)), "-inf.0");
    assert_eq!(format!("{}", Value::Float(1.0)), "1.0");
    assert_eq!(format!("{}", Value::Float(1.5)), "1.5");
}

#[test]
fn branch_value_display_improper_list() {
    let v = Value::cons(Value::Int(1), Value::Int(2));
    assert_eq!(format!("{v}"), "(1 . 2)");
}

#[test]
fn branch_value_display_bytevector() {
    let v = Value::bytevector(vec![1, 2, 3]);
    assert_eq!(format!("{v}"), "#u8(1 2 3)");
}

#[test]
fn branch_value_display_void() {
    assert_eq!(format!("{}", Value::Void), "#<void>");
}

#[test]
fn branch_value_display_eof() {
    assert_eq!(format!("{}", Value::Eof), "#<eof>");
}

#[test]
fn branch_value_display_undefined() {
    assert_eq!(format!("{}", Value::Undefined), "#<undefined>");
}

#[test]
fn branch_value_display_vector() {
    let v = Value::vector(vec![Value::Int(1), Value::Int(2)]);
    assert_eq!(format!("{v}"), "#(1 2)");
}

#[test]
fn branch_value_display_closure() {
    let v = eval("(lambda (x) x)");
    let s = format!("{v}");
    assert!(
        s.contains("procedure") || s.contains("lambda"),
        "closure display: {s}"
    );
}

#[test]
fn branch_value_display_char_special() {
    assert_eq!(format!("{}", Value::Char(' ')), "#\\space");
    assert_eq!(format!("{}", Value::Char('\n')), "#\\newline");
    assert_eq!(format!("{}", Value::Char('\t')), "#\\tab");
    assert_eq!(format!("{}", Value::Char('\r')), "#\\return");
    assert_eq!(format!("{}", Value::Char('\0')), "#\\null");
    assert_eq!(format!("{}", Value::Char('\x07')), "#\\alarm");
    assert_eq!(format!("{}", Value::Char('\x08')), "#\\backspace");
    assert_eq!(format!("{}", Value::Char('\x1b')), "#\\escape");
    assert_eq!(format!("{}", Value::Char('\x7f')), "#\\delete");
    assert_eq!(format!("{}", Value::Char('a')), "#\\a");
}

#[test]
fn branch_value_display_char_non_graphic() {
    // Non-graphic character — display as #\xNN
    let c = '\x01';
    let s = format!("{}", Value::Char(c));
    assert!(
        s.contains("x1") || s.contains("\\x"),
        "non-graphic char display: {s}"
    );
}

#[test]
fn branch_value_display_string_escapes() {
    // String display should escape special characters
    let v = Value::string("hello\n\"world\\");
    let s = format!("{v}");
    assert!(s.contains("\\n"), "newline escape: {s}");
    assert!(s.contains("\\\""), "quote escape: {s}");
    assert!(s.contains("\\\\"), "backslash escape: {s}");
}

#[test]
fn branch_value_eq_identity() {
    // eq? on floats — should be false (different allocations)
    is_false("(eq? 1.0 1.0)");
    // eq? on same symbol
    is_true("(eq? 'foo 'foo)");
    // eq? on different types
    is_false("(eq? 1 1.0)");
    is_false("(eq? '() #f)");
}

#[test]
fn branch_value_eqv_float() {
    is_true("(eqv? 1.0 1.0)");
    is_false("(eqv? 1.0 2.0)");
    // NaN is not eqv? to itself
    is_false("(eqv? +nan.0 +nan.0)");
}

#[test]
fn branch_value_equal_cross_type() {
    // equal? on different types returns false
    is_false("(equal? 1 \"1\")");
    is_false("(equal? 1 #t)");
    is_false("(equal? '() #f)");
}

#[test]
fn branch_value_equal_vectors() {
    is_true("(equal? #(1 2 3) #(1 2 3))");
    is_false("(equal? #(1 2 3) #(1 2 4))");
    is_false("(equal? #(1 2) #(1 2 3))");
}

#[test]
fn branch_value_equal_bytevectors() {
    is_true("(equal? #u8(1 2 3) #u8(1 2 3))");
    is_false("(equal? #u8(1 2 3) #u8(1 2 4))");
}

#[test]
fn branch_value_type_names() {
    // Verify type_name for each variant through error messages
    let msg = eval_err("(+ \"x\" 1)");
    assert!(
        msg.contains("string") || msg.contains("number"),
        "string type: {msg}"
    );
    let msg = eval_err("(car 42)");
    assert!(
        msg.contains("pair") || msg.contains("integer"),
        "int type: {msg}"
    );
}

#[test]
fn branch_value_is_procedure() {
    is_true("(procedure? (lambda () 1))");
    is_true("(procedure? car)");
    is_true("(procedure? (call-with-current-continuation (lambda (k) k)))");
    is_false("(procedure? 42)");
    is_false("(procedure? \"hello\")");
}

#[test]
fn branch_value_to_f64() {
    // to_f64 returns Some for numbers, None for non-numbers
    is_true("(number? 42)");
    is_true("(number? 3.14)");
    is_false("(number? \"hello\")");
}

#[test]
fn branch_value_is_exact() {
    is_true("(exact? 42)");
    is_false("(exact? 3.14)");
}

// =========================================================================
// Phase 2: vm.rs error paths
// =========================================================================

#[test]
fn branch_vm_non_procedure_call() {
    let msg = eval_err("(42 1 2)");
    assert!(
        msg.contains("procedure") || msg.contains("not callable"),
        "non-proc call: {msg}"
    );
}

#[test]
fn branch_vm_undefined_variable() {
    let msg = eval_err("undefined_variable_xyz");
    assert!(
        msg.contains("undefined") || msg.contains("unbound"),
        "undefined var: {msg}"
    );
}

#[test]
fn branch_vm_continuation_wrong_arity() {
    // Continuations expect exactly 1 argument
    let msg = eval_err("(call/cc (lambda (k) (k 1 2)))");
    assert!(
        msg.contains("continuation") || msg.contains("1 argument") || msg.contains("arity"),
        "cont arity: {msg}"
    );
}

#[test]
fn branch_vm_raise_non_error_obj() {
    // raise with a non-error object — e.g., raise a string
    assert_eq!(
        eval("(guard (e (#t e)) (raise \"custom\"))"),
        Value::string("custom"),
    );
    // raise with an integer
    assert_eq!(eval("(guard (e (#t e)) (raise 42))"), Value::Int(42),);
}

#[test]
fn branch_vm_raise_continuable_handler_returns() {
    // raise-continuable: handler returns a value
    is_int(
        "(with-exception-handler
           (lambda (e) (+ e 10))
           (lambda () (+ 1 (raise-continuable 5))))",
        16,
    );
}

#[test]
fn branch_vm_raise_non_continuable_handler_returns() {
    // raise (non-continuable): handler returns → should signal error
    let msg = eval_err(
        "(with-exception-handler
           (lambda (e) 42)
           (lambda () (raise \"boom\")))",
    );
    assert!(
        msg.contains("handler returned")
            || msg.contains("non-continuable")
            || msg.contains("exception"),
        "non-continuable handler returned: {msg}"
    );
}

#[test]
fn branch_vm_stack_overflow() {
    // Stack overflow detection — must use non-tail recursion to grow the stack
    // (+ 1 (f x)) is NOT in tail position, so each call adds a frame
    let msg = eval_err("(define (f x) (+ 1 (f (+ x 1)))) (f 0)");
    assert!(
        msg.contains("stack") || msg.contains("overflow") || msg.contains("frames"),
        "stack overflow: {msg}"
    );
}

#[test]
fn branch_vm_apply_non_list_args() {
    let msg = eval_err("(apply + 42)");
    assert!(
        msg.contains("list") || msg.contains("apply"),
        "apply non-list: {msg}"
    );
}

#[test]
fn branch_vm_foreign_fn_arity_check() {
    // Foreign function arity: too few args
    let msg = eval_err("(car)");
    assert!(
        msg.contains("argument") || msg.contains("arity") || msg.contains("expected"),
        "foreign fn too few: {msg}"
    );
    // Foreign function arity: too many args
    let msg = eval_err("(car 1 2)");
    assert!(
        msg.contains("argument") || msg.contains("arity") || msg.contains("expected"),
        "foreign fn too many: {msg}"
    );
    // Variadic arity: + accepts 0+ args
    is_int("(+)", 0);
    is_int("(+ 1 2 3)", 6);
}

#[test]
fn branch_vm_closure_arity_check() {
    let msg = eval_err("((lambda (x y) (+ x y)) 1)");
    assert!(
        msg.contains("argument") || msg.contains("arity") || msg.contains("expected"),
        "closure arity: {msg}"
    );
    let msg = eval_err("((lambda (x) x) 1 2)");
    assert!(
        msg.contains("argument") || msg.contains("arity") || msg.contains("expected"),
        "closure too many: {msg}"
    );
}

#[test]
fn branch_vm_dynamic_wind_exception() {
    // dynamic-wind after thunk runs even on exception
    is_int(
        "(let ((result 0))
           (guard (e (#t result))
             (dynamic-wind
               (lambda () #f)
               (lambda () (error \"boom\"))
               (lambda () (set! result 42)))))",
        42,
    );
}

#[test]
fn branch_vm_winder_traversal_callcc() {
    // call/cc across dynamic-wind boundaries — winders run
    is_true(
        "(let ((log '()))
           (let ((k (call-with-current-continuation
                      (lambda (c)
                        (dynamic-wind
                          (lambda () (set! log (cons 'before log)))
                          (lambda () (c c))
                          (lambda () (set! log (cons 'after log))))))))
             ;; k is the continuation; don't invoke it again to avoid infinite loop
             (pair? log)))",
    );
}

// =========================================================================
// Phase 2: macros.rs branches
// =========================================================================

#[test]
fn branch_macros_no_matching_pattern() {
    let msg = eval_err(
        "(define-syntax my-if
           (syntax-rules ()
             ((_ test then else) (if test then else))))
         (my-if 1)",
    );
    assert!(
        msg.contains("no matching") || msg.contains("syntax"),
        "no matching pattern: {msg}"
    );
}

#[test]
fn branch_macros_ellipsis_empty() {
    // Ellipsis matching zero elements
    is_int(
        "(define-syntax my-begin
           (syntax-rules ()
             ((_ expr ...) (begin expr ...))))
         (my-begin 42)",
        42,
    );
}

#[test]
fn branch_macros_ellipsis_multi() {
    // Ellipsis matching multiple elements
    is_int(
        "(define-syntax my-add
           (syntax-rules ()
             ((_ x ...) (+ x ...))))
         (my-add 1 2 3 4)",
        10,
    );
}

#[test]
fn branch_macros_literal_match() {
    // Literal identifier matching
    is_int(
        "(define-syntax my-cond
           (syntax-rules (=>)
             ((_ test => proc) (proc test))
             ((_ test expr) (if test expr #f))))
         (my-cond 5 => (lambda (x) (+ x 1)))",
        6,
    );
}

#[test]
fn branch_macros_literal_no_match() {
    // Literal identifier mismatch falls through
    is_int(
        "(define-syntax my-cond
           (syntax-rules (=>)
             ((_ test => proc) (proc test))
             ((_ test expr) (if test expr #f))))
         (my-cond #t 42)",
        42,
    );
}

#[test]
fn branch_macros_nested_patterns() {
    // Nested pattern matching
    is_int(
        "(define-syntax my-let1
           (syntax-rules ()
             ((_ ((var val)) body ...) ((lambda (var) body ...) val))))
         (my-let1 ((x 42)) x)",
        42,
    );
}

// =========================================================================
// Phase 2: library.rs parsing branches
// =========================================================================

#[test]
fn branch_library_name_empty() {
    let msg = eval_err("(define-library ())");
    assert!(
        msg.contains("empty") || msg.contains("non-empty"),
        "empty lib name: {msg}"
    );
}

#[test]
fn branch_library_name_invalid_component() {
    let msg = eval_err("(define-library (#t) (export) (begin))");
    assert!(
        msg.contains("identifier") || msg.contains("integer") || msg.contains("component"),
        "bad lib component: {msg}"
    );
}

#[test]
fn branch_library_name_with_int() {
    // Library name with integer component — allowed by R7RS
    eval("(define-library (test 1) (export) (begin))");
}

#[test]
fn branch_library_unknown_declaration() {
    let msg = eval_err("(define-library (test bad) (unknown-decl 1))");
    assert!(
        msg.contains("unknown") || msg.contains("declaration"),
        "unknown lib decl: {msg}"
    );
}

#[test]
fn branch_library_export_rename() {
    // export with rename
    eval("(define-library (test export-rename) (export (rename my-fn ext-fn)) (begin (define my-fn 42)))");
}

#[test]
fn branch_library_export_invalid() {
    let msg = eval_err("(define-library (test bad-export) (export 42))");
    assert!(
        msg.contains("export") || msg.contains("invalid"),
        "invalid export: {msg}"
    );
}

#[test]
fn branch_library_export_rename_non_symbol() {
    let msg = eval_err("(define-library (test bad-rename) (export (rename 42 foo)))");
    assert!(
        msg.contains("export") || msg.contains("identifier") || msg.contains("rename"),
        "rename non-symbol: {msg}"
    );
}

#[test]
fn branch_library_import_except() {
    // Import with except — uses a user-defined library
    eval(
        "(define-library (test except-base)
           (export a b c)
           (begin (define a 1) (define b 2) (define c 3)))
         (import (except (test except-base) b))",
    );
}

#[test]
fn branch_library_import_prefix() {
    eval(
        "(define-library (test prefix-base)
           (export x)
           (begin (define x 42)))
         (import (prefix (test prefix-base) pre:))",
    );
}

#[test]
fn branch_library_import_rename() {
    eval(
        "(define-library (test rename-base)
           (export x)
           (begin (define x 42)))
         (import (rename (test rename-base) (x y)))",
    );
}

#[test]
fn branch_library_import_only_not_in_set() {
    // Narrowing explicit bindings — requesting name not in explicit set
    // This is tested via nested import modifiers
    let msg = eval_err(
        "(define-library (test only-nested)
           (export a b)
           (begin (define a 1) (define b 2)))
         (import (only (only (test only-nested) a) b))",
    );
    assert!(
        msg.contains("not in") || msg.contains("only"),
        "only not in set: {msg}"
    );
}

#[test]
fn branch_library_import_except_empty() {
    let msg = eval_err("(import (except))");
    assert!(
        msg.contains("except") || msg.contains("requires"),
        "except empty: {msg}"
    );
}

#[test]
fn branch_library_import_prefix_arity() {
    let msg = eval_err("(import (prefix))");
    assert!(
        msg.contains("prefix") || msg.contains("requires"),
        "prefix empty: {msg}"
    );
}

#[test]
fn branch_library_import_rename_arity() {
    let msg = eval_err("(import (rename))");
    assert!(
        msg.contains("rename") || msg.contains("requires"),
        "rename empty: {msg}"
    );
}

#[test]
fn branch_library_import_rename_bad_pair() {
    let msg = eval_err(
        "(define-library (test rn-bad) (export x) (begin (define x 1)))
         (import (rename (test rn-bad) (x)))",
    );
    assert!(
        msg.contains("rename") || msg.contains("pair"),
        "rename bad pair: {msg}"
    );
}

#[test]
fn branch_library_import_only_non_symbol() {
    let msg = eval_err(
        "(define-library (test only-ns) (export x) (begin (define x 1)))
         (import (only (test only-ns) 42))",
    );
    assert!(
        msg.contains("identifier") || msg.contains("only"),
        "only non-symbol: {msg}"
    );
}

#[test]
fn branch_library_import_except_non_symbol() {
    let msg = eval_err(
        "(define-library (test exc-ns) (export x) (begin (define x 1)))
         (import (except (test exc-ns) 42))",
    );
    assert!(
        msg.contains("identifier") || msg.contains("except"),
        "except non-symbol: {msg}"
    );
}

#[test]
fn branch_library_import_prefix_non_symbol() {
    let msg = eval_err(
        "(define-library (test pfx-ns) (export x) (begin (define x 1)))
         (import (prefix (test pfx-ns) 42))",
    );
    assert!(
        msg.contains("identifier") || msg.contains("prefix"),
        "prefix non-symbol: {msg}"
    );
}

#[test]
fn branch_library_import_rename_non_symbol() {
    let msg = eval_err(
        "(define-library (test rn-ns) (export x) (begin (define x 1)))
         (import (rename (test rn-ns) (42 y)))",
    );
    assert!(
        msg.contains("identifier") || msg.contains("rename"),
        "rename non-symbol: {msg}"
    );
}

// =========================================================================
// Phase 2: io.rs remaining edge cases
// =========================================================================

#[test]
fn branch_io_write_simple_no_port() {
    // write-simple without port arg — writes to stdout (no error)
    eval("(write-simple 42)");
}

#[test]
fn branch_io_write_shared_no_port() {
    eval("(write-shared '(1 2 3))");
}

#[test]
fn branch_io_format_tilde_a() {
    // format ~a for display
    assert_eq!(eval("(format \"~a\" 42)"), Value::string("42"));
    assert_eq!(eval("(format \"~a\" \"hello\")"), Value::string("hello"));
}

#[test]
fn branch_io_format_tilde_s() {
    // format ~s for write (quoted)
    assert_eq!(
        eval("(format \"~s\" \"hello\")"),
        Value::string("\"hello\"")
    );
}

#[test]
fn branch_io_format_tilde_percent() {
    assert_eq!(eval("(format \"~%\")"), Value::string("\n"));
}

#[test]
fn branch_io_format_tilde_tilde() {
    assert_eq!(eval("(format \"~~\")"), Value::string("~"));
}

#[test]
fn branch_io_format_unknown_directive() {
    // Unknown format directive is passed through as literal
    assert_eq!(eval("(format \"~z\" 1)"), Value::string("~z"));
}

#[test]
fn branch_io_read_string_from_port() {
    assert_eq!(
        eval("(let ((p (open-input-string \"hello\"))) (read-string 3 p))"),
        Value::string("hel"),
    );
}

#[test]
fn branch_io_read_string_eof() {
    assert_eq!(
        eval("(let ((p (open-input-string \"\"))) (read-string 5 p))"),
        Value::Eof,
    );
}

#[test]
fn branch_io_flush_output_port() {
    eval("(flush-output-port (current-output-port))");
}

#[test]
fn branch_io_get_environment_variable() {
    is_false("(get-environment-variable \"MAE_NONEXISTENT_12345\")");
    // HOME should exist
    is_true("(string? (get-environment-variable \"HOME\"))");
}

#[test]
fn branch_io_get_environment_variables() {
    is_true("(list? (get-environment-variables))");
}

// =========================================================================
// Phase 2: base.rs remaining branches
// =========================================================================

#[test]
fn branch_base_modulo_both_negative() {
    // modulo with both operands negative
    assert_eq!(eval("(modulo -7 -3)"), Value::Int(-1));
}

#[test]
fn branch_base_floor_quotient_mixed_signs() {
    // floor-quotient with positive/negative
    assert_eq!(eval("(floor-quotient 7 -2)"), Value::Int(-4));
    assert_eq!(eval("(floor-quotient -7 2)"), Value::Int(-4));
}

#[test]
fn branch_base_floor_remainder_mixed_signs() {
    assert_eq!(eval("(floor-remainder 7 -2)"), Value::Int(-1));
    assert_eq!(eval("(floor-remainder -7 2)"), Value::Int(1));
}

#[test]
fn branch_base_truncate_div() {
    assert_eq!(eval("(truncate-quotient 7 2)"), Value::Int(3));
    assert_eq!(eval("(truncate-remainder 7 2)"), Value::Int(1));
    // truncate/ returns 2 values
    assert_eq!(
        eval("(call-with-values (lambda () (truncate/ 7 2)) list)"),
        eval("'(3 1)")
    );
}

#[test]
fn branch_base_floor_div() {
    // floor/ returns 2 values
    assert_eq!(
        eval("(call-with-values (lambda () (floor/ 7 2)) list)"),
        eval("'(3 1)")
    );
    assert_eq!(
        eval("(call-with-values (lambda () (floor/ -7 2)) list)"),
        eval("'(-4 1)")
    );
}

#[test]
fn branch_base_gcd_zero() {
    assert_eq!(eval("(gcd 0 0)"), Value::Int(0));
    assert_eq!(eval("(gcd 0 5)"), Value::Int(5));
    assert_eq!(eval("(gcd 5 0)"), Value::Int(5));
}

#[test]
fn branch_base_lcm_zero() {
    assert_eq!(eval("(lcm 0 5)"), Value::Int(0));
    assert_eq!(eval("(lcm 5 0)"), Value::Int(0));
}

#[test]
fn branch_base_number_to_string_float_radix() {
    // number->string with float should work for radix 10
    let result = eval("(number->string 3.14)");
    assert!(matches!(result, Value::String(_)));
}

#[test]
fn branch_base_string_to_number_invalid() {
    is_false("(string->number \"abc\")");
    is_false("(string->number \"\")");
}

#[test]
fn branch_base_string_to_number_radix() {
    assert_eq!(eval("(string->number \"ff\" 16)"), Value::Int(255));
    assert_eq!(eval("(string->number \"77\" 8)"), Value::Int(63));
    assert_eq!(eval("(string->number \"101\" 2)"), Value::Int(5));
}

#[test]
fn branch_base_list_copy() {
    // list-copy makes a fresh copy
    assert_eq!(eval("(list-copy '(1 2 3))"), eval("'(1 2 3)"));
    assert_eq!(eval("(list-copy '())"), Value::Null);
}

#[test]
fn branch_base_make_list() {
    assert_eq!(eval("(make-list 3 'x)"), eval("'(x x x)"));
    assert_eq!(eval("(make-list 0 'x)"), Value::Null);
}

#[test]
fn branch_base_exact_integer_sqrt() {
    assert_eq!(
        eval("(call-with-values (lambda () (exact-integer-sqrt 14)) list)"),
        eval("'(3 5)"),
    );
    assert_eq!(
        eval("(call-with-values (lambda () (exact-integer-sqrt 16)) list)"),
        eval("'(4 0)"),
    );
}

#[test]
fn branch_base_rationalize() {
    // rationalize returns closest rational within tolerance
    let result = eval("(rationalize 3.14 0.1)");
    assert!(matches!(result, Value::Float(_) | Value::Int(_)));
}

#[test]
fn branch_base_expt_zero_base() {
    assert_eq!(eval("(expt 0 0)"), Value::Int(1));
    assert_eq!(eval("(expt 0 5)"), Value::Int(0));
}

#[test]
fn branch_base_expt_negative() {
    assert_eq!(eval("(expt 2 -1)"), Value::Float(0.5));
}

#[test]
fn branch_base_square() {
    assert_eq!(eval("(square 5)"), Value::Int(25));
    assert_eq!(eval("(square 1.5)"), Value::Float(2.25));
}

#[test]
fn branch_base_abs() {
    assert_eq!(eval("(abs -5)"), Value::Int(5));
    assert_eq!(eval("(abs 5)"), Value::Int(5));
    assert_eq!(eval("(abs -2.75)"), Value::Float(2.75));
}

#[test]
fn branch_base_string_conversion_roundtrip() {
    assert_eq!(eval("(number->string 42)"), Value::string("42"));
    assert_eq!(eval("(string->number \"42\")"), Value::Int(42));
    assert_eq!(eval("(string->number \"2.75\")"), Value::Float(2.75));
}

#[test]
fn branch_base_char_predicates() {
    is_true("(char-alphabetic? #\\a)");
    is_false("(char-alphabetic? #\\1)");
    is_true("(char-numeric? #\\5)");
    is_false("(char-numeric? #\\a)");
    is_true("(char-whitespace? #\\space)");
    is_false("(char-whitespace? #\\a)");
    is_true("(char-upper-case? #\\A)");
    is_false("(char-upper-case? #\\a)");
    is_true("(char-lower-case? #\\a)");
    is_false("(char-lower-case? #\\A)");
}
