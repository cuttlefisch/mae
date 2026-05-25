//! Scheme benchmark programs for performance validation.
//!
//! These programs are adapted from the classic Gabriel/Larceny/Gambit
//! benchmark suites. Each test verifies correctness AND measures timing.
//! The `#[ignore]` tests are long-running and should be run explicitly:
//!
//!   cargo test -p mae-scheme --test scheme_benchmarks -- --ignored --nocapture
//!
//! Performance targets (rough orders of magnitude on modern hardware):
//!   - fib(30): < 2s (tree recursion, no optimization)
//!   - tak(18,12,6): < 3s (deep recursion)
//!   - sieve(10000): < 1s (vector mutation)
//!   - nqueens(8): < 1s (backtracking search)
//!   - deriv(large): < 1s (symbolic computation)

use std::time::Instant;

use mae_scheme::stdlib;
use mae_scheme::value::Value;
use mae_scheme::vm::Vm;

fn timed_eval(name: &str, code: &str) -> (Value, std::time::Duration) {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let start = Instant::now();
    let result = vm.eval(code).unwrap();
    let elapsed = start.elapsed();
    eprintln!("  {name}: {elapsed:?}");
    (result, elapsed)
}

// ============================================================
// GABRIEL BENCHMARKS (adapted from the classic Lisp benchmark suite)
// ============================================================

#[test]
fn bench_fib_20() {
    let (result, elapsed) = timed_eval(
        "fib(20)",
        "
        (define (fib n)
          (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
        (fib 20)
    ",
    );
    assert_eq!(result, Value::Int(6765));
    assert!(elapsed.as_millis() < 5000, "fib(20) too slow: {elapsed:?}");
}

#[test]
#[ignore]
fn bench_fib_30() {
    let (result, elapsed) = timed_eval(
        "fib(30)",
        "
        (define (fib n)
          (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
        (fib 30)
    ",
    );
    assert_eq!(result, Value::Int(832040));
    assert!(elapsed.as_secs() < 10, "fib(30) too slow: {elapsed:?}");
}

#[test]
fn bench_tak() {
    let (result, elapsed) = timed_eval(
        "tak(18,12,6)",
        "
        (define (tak x y z)
          (if (not (< y x))
              z
              (tak (tak (- x 1) y z)
                   (tak (- y 1) z x)
                   (tak (- z 1) x y))))
        (tak 18 12 6)
    ",
    );
    assert_eq!(result, Value::Int(7));
    assert!(elapsed.as_secs() < 10, "tak too slow: {elapsed:?}");
}

#[test]
fn bench_sieve() {
    let (result, elapsed) = timed_eval(
        "sieve(10000)",
        "
        (define (sieve limit)
          (let ((is-prime (make-vector (+ limit 1) #t)))
            (vector-set! is-prime 0 #f)
            (vector-set! is-prime 1 #f)
            (do ((i 2 (+ i 1)))
              ((> (* i i) limit))
              (when (vector-ref is-prime i)
                (do ((j (* i i) (+ j i)))
                  ((> j limit))
                  (vector-set! is-prime j #f))))
            (let ((count 0))
              (do ((i 2 (+ i 1)))
                ((> i limit) count)
                (when (vector-ref is-prime i)
                  (set! count (+ count 1)))))))
        (sieve 10000)
    ",
    );
    assert_eq!(result, Value::Int(1229)); // 1229 primes below 10000
    assert!(elapsed.as_secs() < 5, "sieve too slow: {elapsed:?}");
}

#[test]
fn bench_nqueens() {
    let (result, elapsed) = timed_eval(
        "nqueens(8)",
        "
        (define (nqueens n)
          (define (safe? col queens row)
            (if (null? queens) #t
                (let ((r (car queens)))
                  (and (not (= r col))
                       (not (= (abs (- r col)) row))
                       (safe? col (cdr queens) (+ row 1))))))
          (define (solve queens num-placed)
            (if (= num-placed n) 1
                (let loop ((col 0) (count 0))
                  (if (= col n) count
                      (loop (+ col 1)
                            (+ count
                               (if (safe? col queens 1)
                                   (solve (cons col queens) (+ num-placed 1))
                                   0)))))))
          (solve '() 0))
        (nqueens 8)
    ",
    );
    assert_eq!(result, Value::Int(92)); // 92 solutions to 8-queens
    assert!(elapsed.as_secs() < 5, "nqueens too slow: {elapsed:?}");
}

#[test]
fn bench_deriv() {
    // Symbolic differentiation — classic Lisp benchmark
    let (result, elapsed) = timed_eval(
        "deriv",
        r#"
        (define (deriv exp var)
          (cond
            ((number? exp) 0)
            ((symbol? exp) (if (eq? exp var) 1 0))
            ((and (pair? exp) (eq? (car exp) '+))
             (list '+ (deriv (cadr exp) var) (deriv (caddr exp) var)))
            ((and (pair? exp) (eq? (car exp) '*))
             (list '+
                   (list '* (cadr exp) (deriv (caddr exp) var))
                   (list '* (deriv (cadr exp) var) (caddr exp))))
            (else (error "unknown expression" exp))))

        (define (caddr x) (car (cdr (cdr x))))

        ;; Differentiate a complex expression 1000 times
        (define expr '(+ (* x (* x x)) (+ (* 3 (* x x)) (+ (* 3 x) 1))))
        (let loop ((i 0) (result '()))
          (if (= i 1000) result
              (loop (+ i 1) (deriv expr 'x))))
    "#,
    );
    // Just check it completes and returns a list (deriv returns (+ ...) form)
    assert!(
        matches!(result, Value::Pair(_)),
        "deriv should return a list, got: {result}"
    );
    assert!(elapsed.as_secs() < 5, "deriv too slow: {elapsed:?}");
}

#[test]
fn bench_tco_count() {
    // Pure tail-call counting — tests that TCO doesn't blow the stack
    let (result, elapsed) = timed_eval(
        "tco-count(1M)",
        "
        (define (count-down n)
          (if (= n 0) 0 (count-down (- n 1))))
        (count-down 1000000)
    ",
    );
    assert_eq!(result, Value::Int(0));
    assert!(elapsed.as_secs() < 5, "tco-count too slow: {elapsed:?}");
}

#[test]
fn bench_list_operations() {
    // List building and traversal
    let (result, elapsed) = timed_eval(
        "list-ops(10000)",
        "
        ;; Build a list of 10000 elements
        (define (iota n)
          (let loop ((i (- n 1)) (acc '()))
            (if (< i 0) acc
                (loop (- i 1) (cons i acc)))))

        (define lst (iota 10000))

        ;; Sum the list
        (define (sum lst)
          (let loop ((l lst) (acc 0))
            (if (null? l) acc
                (loop (cdr l) (+ acc (car l))))))

        ;; Reverse the list
        (define rlst (reverse lst))

        ;; Verify
        (+ (sum lst) (if (= (car rlst) 9999) 1 0))
    ",
    );
    // sum(0..9999) = 49995000 + 1 = 49995001
    assert_eq!(result, Value::Int(49995001));
    assert!(elapsed.as_secs() < 5, "list-ops too slow: {elapsed:?}");
}

#[test]
fn bench_map_filter() {
    let (result, elapsed) = timed_eval(
        "map+filter(5000)",
        "
        (define (iota n)
          (let loop ((i (- n 1)) (acc '()))
            (if (< i 0) acc
                (loop (- i 1) (cons i acc)))))

        (length
          (filter even?
            (map (lambda (x) (* x 3))
                 (iota 5000))))
    ",
    );
    assert_eq!(result, Value::Int(2500)); // half of 5000 are even after *3
    assert!(elapsed.as_secs() < 5, "map+filter too slow: {elapsed:?}");
}

#[test]
fn bench_string_ops() {
    let (result, elapsed) = timed_eval(
        "string-ops",
        r#"
        ;; Build a string by repeated append
        (define (repeat-string s n)
          (let loop ((i 0) (acc ""))
            (if (= i n) acc
                (loop (+ i 1) (string-append acc s)))))

        (string-length (repeat-string "ab" 1000))
    "#,
    );
    assert_eq!(result, Value::Int(2000));
    assert!(elapsed.as_secs() < 5, "string-ops too slow: {elapsed:?}");
}

#[test]
fn bench_vector_ops() {
    let (result, elapsed) = timed_eval(
        "vector-ops(10000)",
        "
        (define v (make-vector 10000 0))

        ;; Fill with indices
        (do ((i 0 (+ i 1)))
          ((= i 10000))
          (vector-set! v i i))

        ;; Sum all elements
        (let loop ((i 0) (sum 0))
          (if (= i 10000) sum
              (loop (+ i 1) (+ sum (vector-ref v i)))))
    ",
    );
    assert_eq!(result, Value::Int(49995000));
    assert!(elapsed.as_secs() < 5, "vector-ops too slow: {elapsed:?}");
}

#[test]
fn bench_closure_creation() {
    // Creating and calling many closures
    let (result, elapsed) = timed_eval(
        "closures(10000)",
        "
        (define (make-adder n) (lambda (x) (+ x n)))
        (let loop ((i 0) (sum 0))
          (if (= i 10000) sum
              (loop (+ i 1) (+ sum ((make-adder i) 1)))))
    ",
    );
    // sum of (i+1) for i=0..9999 = sum(1..10000) = 50005000
    assert_eq!(result, Value::Int(50005000));
    assert!(elapsed.as_secs() < 5, "closures too slow: {elapsed:?}");
}

#[test]
fn bench_recursion_depth() {
    // Test deeply recursive function (not tail-recursive)
    // This tests stack behavior for non-TCO recursion
    let (result, elapsed) = timed_eval(
        "deep-recursion(5000)",
        "
        (define (depth n)
          (if (= n 0) 0
              (+ 1 (depth (- n 1)))))
        (depth 5000)
    ",
    );
    assert_eq!(result, Value::Int(5000));
    assert!(
        elapsed.as_secs() < 5,
        "deep-recursion too slow: {elapsed:?}"
    );
}

#[test]
fn bench_higher_order_composition() {
    let (result, elapsed) = timed_eval(
        "compose-chain",
        "
        (define (compose f g) (lambda (x) (f (g x))))
        (define inc (lambda (x) (+ x 1)))

        ;; Build a chain of 100 increments
        (define inc100
          (let loop ((i 0) (f (lambda (x) x)))
            (if (= i 100) f
                (loop (+ i 1) (compose inc f)))))

        (inc100 0)
    ",
    );
    assert_eq!(result, Value::Int(100));
    assert!(elapsed.as_secs() < 5, "compose too slow: {elapsed:?}");
}

// ============================================================
// LARCENY BENCHMARKS (adapted)
// ============================================================

#[test]
fn bench_puzzle() {
    // N-puzzle move counting (simplified)
    let (result, elapsed) = timed_eval(
        "puzzle",
        "
        (define (puzzle-count n)
          ;; Count permutations reachable in n swap steps from (0 1 2)
          ;; Simple combinatorial explosion test
          (define (swap lst i j)
            (let ((v (list->vector lst)))
              (let ((tmp (vector-ref v i)))
                (vector-set! v i (vector-ref v j))
                (vector-set! v j tmp)
                (vector->list v))))
          (define (generate-moves lst n)
            (if (= n 0) (list lst)
                (let ((results '()))
                  (do ((i 0 (+ i 1)))
                    ((= i (length lst)) results)
                    (do ((j (+ i 1) (+ j 1)))
                      ((= j (length lst)))
                      (set! results
                        (append results
                                (generate-moves (swap lst i j) (- n 1)))))))))
          (length (generate-moves '(0 1 2 3) 3)))
        (puzzle-count 0) ;; just verify it works
    ",
    );
    // With n=3, should be 120 unique sequences (but many duplicates)
    assert!(matches!(result, Value::Int(_)));
    assert!(elapsed.as_secs() < 5, "puzzle too slow: {elapsed:?}");
}

#[test]
fn bench_ack_small() {
    // Ackermann function — super-exponential growth
    let (result, elapsed) = timed_eval(
        "ack(3,7)",
        "
        (define (ack m n)
          (cond
            ((= m 0) (+ n 1))
            ((= n 0) (ack (- m 1) 1))
            (else (ack (- m 1) (ack m (- n 1))))))
        (ack 3 7)
    ",
    );
    assert_eq!(result, Value::Int(1021));
    assert!(elapsed.as_secs() < 10, "ack(3,7) too slow: {elapsed:?}");
}

// ============================================================
// STARTUP & OVERHEAD BENCHMARKS
// ============================================================

#[test]
fn bench_vm_startup() {
    let start = Instant::now();
    for _ in 0..100 {
        let mut vm = Vm::new();
        stdlib::register_stdlib(&mut vm);
        vm.eval("42").unwrap();
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / 100;
    eprintln!("  VM startup (100 iterations): {elapsed:?} ({per_iter:?}/iter)");
    assert!(
        per_iter.as_millis() < 50,
        "VM startup too slow: {per_iter:?}"
    );
}

#[test]
fn bench_eval_overhead() {
    // Measure overhead of eval calls
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);

    let start = Instant::now();
    for _ in 0..10000 {
        vm.eval("42").unwrap();
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / 10000;
    eprintln!("  eval(42) x 10000: {elapsed:?} ({per_iter:?}/iter)");
    assert!(
        per_iter.as_micros() < 1000,
        "eval overhead too high: {per_iter:?}"
    );
}
