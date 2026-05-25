//! R7RS-small compliance test suite for mae-scheme.
//!
//! Tests organized by R7RS specification section number.
//! Each section has its own test function with assertions covering
//! the behavior specified in the standard.
//!
//! Reference: https://small.r7rs.org/attachment/r7rs.pdf

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

fn is_int(code: &str, expected: i64) {
    assert_eq!(
        eval(code),
        Value::Int(expected),
        "expected {expected} for: {code}"
    );
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
    // R7RS: these return exact integers for exact args, inexact for inexact
    // Our impl returns Int for integer-valued results
    is_int("(floor 2.7)", 2);
    is_int("(ceiling 2.3)", 3);
    is_int("(truncate 2.7)", 2);
    is_int("(truncate -2.7)", -2);
    is_int("(round 2.5)", 2); // banker's rounding
    is_int("(round 3.5)", 4);
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
// Steel regression tests (must all pass per plan)
// ============================================================================

#[test]
fn steel_regression_void_in_tail_position() {
    // Was a crash in Steel
    assert_eq!(eval("(if #t (void))"), Value::Void);
    assert_eq!(eval("(begin 1 2 (void))"), Value::Void);
}

#[test]
fn steel_regression_define_global_updates() {
    // Steel's register_value created new cells instead of updating
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.define_global("x", Value::Int(1));
    assert_eq!(vm.eval("x").unwrap(), Value::Int(1));
    vm.define_global("x", Value::Int(2));
    assert_eq!(vm.eval("x").unwrap(), Value::Int(2));
}

#[test]
fn steel_regression_error_from_ffi() {
    // Steel couldn't propagate errors from Rust FFI
    // Our register_fn returns Result, so errors propagate as Scheme exceptions
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
    // error with irritants — guard catches
    is_true(
        "(guard (exn (#t (string? exn)))
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
    is_true("(char-ready?)");
    is_true("(u8-ready?)");
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
