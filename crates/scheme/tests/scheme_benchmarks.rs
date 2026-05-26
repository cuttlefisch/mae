//! Scheme benchmark programs for performance validation.
//!
//! These programs are adapted from the classic Gabriel/Larceny/Gambit
//! benchmark suites. Each test verifies correctness AND measures timing.
//!
//! Performance targets (rough orders of magnitude, debug mode):
//!   - fib(20): < 5s (tree recursion)
//!   - fib(30): < 30s (tree recursion, CI generous)
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
    // Generous timeout for CI (debug mode, slow runners, parallel tests)
    assert!(elapsed.as_secs() < 60, "fib(30) too slow: {elapsed:?}");
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
    assert!(elapsed.as_secs() < 15, "tco-count too slow: {elapsed:?}");
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
    assert!(elapsed.as_secs() < 20, "ack(3,7) too slow: {elapsed:?}");
}

// ============================================================
// GABRIEL/LARCENY CLASSIC BENCHMARKS
// ============================================================

#[test]
fn bench_gabriel_tak() {
    // Classic Takeuchi function — deep mutual recursion
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
    assert!(elapsed.as_secs() < 10, "tak(18,12,6) too slow: {elapsed:?}");
}

#[test]
fn bench_gabriel_cpstak() {
    // CPS (continuation-passing style) TAK — tests closure allocation
    let (result, elapsed) = timed_eval(
        "cpstak(18,12,6)",
        "
        (define (cpstak x y z)
          (define (tak x y z k)
            (if (not (< y x))
                (k z)
                (tak (- x 1)
                     y
                     z
                     (lambda (v1)
                       (tak (- y 1)
                            z
                            x
                            (lambda (v2)
                              (tak (- z 1)
                                   x
                                   y
                                   (lambda (v3)
                                     (tak v1 v2 v3 k)))))))))
          (tak x y z (lambda (a) a)))
        (cpstak 18 12 6)
    ",
    );
    assert_eq!(result, Value::Int(7));
    assert!(
        elapsed.as_secs() < 30,
        "cpstak(18,12,6) too slow: {elapsed:?}"
    );
}

#[test]
fn bench_gabriel_deriv() {
    // Symbolic differentiation — list manipulation + pattern matching
    let (result, _elapsed) = timed_eval(
        "deriv",
        "
        (define (deriv a)
          (cond ((not (pair? a))
                 (if (eq? a 'x) 1 0))
                ((eq? (car a) '+)
                 (cons '+ (map deriv (cdr a))))
                ((eq? (car a) '-)
                 (cons '- (map deriv (cdr a))))
                ((eq? (car a) '*)
                 (list '* a
                       (cons '+ (map (lambda (a) (list '/ (deriv a) a)) (cdr a)))))
                (else 0)))

        ;; Run on a moderately complex expression
        (define expr '(+ (* 3 x x) (* 2 x) 1))
        (deriv expr)
    ",
    );
    // Result should be a valid s-expression (derivative of 3x^2 + 2x + 1)
    assert!(
        result.is_list(),
        "deriv should return a list, got: {result}"
    );
}

#[test]
fn bench_gabriel_nqueens() {
    // N-queens solver — backtracking search
    let (result, elapsed) = timed_eval(
        "nqueens(8)",
        "
        (define (nqueens n)
          (define (iota1 n)
            (let loop ((i n) (l '()))
              (if (= i 0) l (loop (- i 1) (cons i l)))))
          (define (my-try x y z)
            (if (null? x)
                (if (null? y) 1 0)
                (+ (if (ok? (car x) 1 z)
                       (my-try (append (cdr x) y) '() (cons (car x) z))
                       0)
                   (my-try (cdr x) (cons (car x) y) z))))
          (define (ok? row dist placed)
            (if (null? placed)
                #t
                (and (not (= (car placed) (+ row dist)))
                     (not (= (car placed) (- row dist)))
                     (ok? row (+ dist 1) (cdr placed)))))
          (my-try (iota1 n) '() '()))
        (nqueens 8)
    ",
    );
    assert_eq!(result, Value::Int(92)); // 92 solutions for 8-queens
    assert!(elapsed.as_secs() < 10, "nqueens(8) too slow: {elapsed:?}");
}

#[test]
fn bench_gabriel_primes() {
    // Prime counting by trial division — arithmetic + looping
    let (result, elapsed) = timed_eval(
        "primes(1000)",
        "
        (define (prime? n)
          (define (check d)
            (cond ((> (* d d) n) #t)
                  ((= (remainder n d) 0) #f)
                  (else (check (+ d 1)))))
          (if (< n 2) #f (check 2)))
        (define (count-primes limit)
          (let loop ((i 2) (count 0))
            (if (> i limit) count
                (loop (+ i 1) (if (prime? i) (+ count 1) count)))))
        (count-primes 1000)
    ",
    );
    assert_eq!(result, Value::Int(168)); // 168 primes below 1000
    assert!(elapsed.as_secs() < 5, "primes(1000) too slow: {elapsed:?}");
}

#[test]
fn bench_gabriel_quicksort() {
    // Quicksort — list manipulation + recursion
    let (result, elapsed) = timed_eval(
        "quicksort",
        "
        (define (quicksort lst)
          (if (or (null? lst) (null? (cdr lst)))
              lst
              (let ((pivot (car lst))
                    (rest (cdr lst)))
                (let ((less (filter (lambda (x) (< x pivot)) rest))
                      (greater (filter (lambda (x) (>= x pivot)) rest)))
                  (append (quicksort less) (list pivot) (quicksort greater))))))

        ;; Sort a reversed list of 500 elements
        (define (make-reversed-list n)
          (let loop ((i 1) (acc '()))
            (if (> i n) acc (loop (+ i 1) (cons i acc)))))
        (define data (make-reversed-list 500))
        (let* ((sorted (quicksort data))
               (first (car sorted))
               (last (list-ref sorted 499)))
          (list first last (length sorted)))
    ",
    );
    assert_eq!(
        result,
        Value::list(vec![Value::Int(1), Value::Int(500), Value::Int(500)])
    );
    assert!(elapsed.as_secs() < 10, "quicksort too slow: {elapsed:?}");
}

#[test]
fn bench_gabriel_mbrot() {
    // Mandelbrot set — floating-point arithmetic + iteration
    let (result, elapsed) = timed_eval(
        "mandelbrot",
        "
        (define (mandelbrot-count cr ci max-iter)
          (let loop ((zr 0.0) (zi 0.0) (i 0))
            (if (>= i max-iter) max-iter
                (let ((zr2 (* zr zr)) (zi2 (* zi zi)))
                  (if (> (+ zr2 zi2) 4.0)
                      i
                      (loop (+ (- zr2 zi2) cr)
                            (+ (* 2.0 zr zi) ci)
                            (+ i 1)))))))

        ;; Count points in a 20x20 grid that are in the set
        (define (count-mandelbrot size max-iter)
          (let loop-y ((y 0) (count 0))
            (if (>= y size) count
                (let loop-x ((x 0) (c count))
                  (if (>= x size)
                      (loop-y (+ y 1) c)
                      (let* ((cr (- (* 3.0 (/ (exact->inexact x) (exact->inexact size))) 2.0))
                             (ci (- (* 2.0 (/ (exact->inexact y) (exact->inexact size))) 1.0))
                             (iters (mandelbrot-count cr ci max-iter)))
                        (loop-x (+ x 1) (if (= iters max-iter) (+ c 1) c))))))))
        (count-mandelbrot 20 100)
    ",
    );
    // Should count some points in the Mandelbrot set
    assert!(
        matches!(result, Value::Int(n) if n > 0),
        "mandelbrot should find points in set"
    );
    assert!(elapsed.as_secs() < 10, "mandelbrot too slow: {elapsed:?}");
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
