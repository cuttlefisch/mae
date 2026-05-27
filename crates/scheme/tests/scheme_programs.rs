//! Classic Scheme programs for validation.
//!
//! These are well-known Scheme programs adapted from SICP, SRFI reference
//! implementations, and the Scheme community. They exercise the full range
//! of R7RS features: closures, higher-order functions, tail calls, macros,
//! continuations, and data structures.
//!
//! Each program is a self-contained test that validates both correctness
//! and spec compliance through real-world usage patterns.

use mae_scheme::stdlib;
use mae_scheme::value::Value;
use mae_scheme::vm::Vm;

fn eval(code: &str) -> Value {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(code).unwrap()
}

fn _eval_vm(code: &str) -> (Value, Vm) {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    let result = vm.eval(code).unwrap();
    (result, vm)
}

// ============================================================
// SICP: Metacircular evaluator components
// ============================================================

#[test]
fn sicp_environment_model() {
    // SICP §3.2: Environment model — frames, bindings, enclosure
    let result = eval(
        r#"
        (define (make-frame vars vals)
          (map cons vars vals))

        (define (frame-lookup var frame)
          (cond
            ((null? frame) #f)
            ((equal? (caar frame) var) (cdar frame))
            (else (frame-lookup var (cdr frame)))))

        (let ((frame (make-frame '(x y z) '(1 2 3))))
          (list (frame-lookup 'x frame)
                (frame-lookup 'y frame)
                (frame-lookup 'z frame)
                (frame-lookup 'w frame)))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(1));
    assert_eq!(items[1], Value::Int(2));
    assert_eq!(items[2], Value::Int(3));
    assert_eq!(items[3], Value::Bool(false));
}

#[test]
fn sicp_streams() {
    // SICP §3.5: Streams as delayed evaluation
    let result = eval(
        r#"
        ;; cons-stream must be a macro so the second arg is delayed
        (define-syntax cons-stream
          (syntax-rules ()
            ((_ a b) (cons a (delay b)))))
        (define (stream-car s) (car s))
        (define (stream-cdr s) (force (cdr s)))
        (define stream-null '())
        (define (stream-null? s) (null? s))

        (define (stream-take n s)
          (if (or (= n 0) (stream-null? s))
              '()
              (cons (stream-car s)
                    (stream-take (- n 1) (stream-cdr s)))))

        (define (stream-map f s)
          (if (stream-null? s)
              stream-null
              (cons-stream (f (stream-car s))
                           (stream-map f (stream-cdr s)))))

        (define (integers-from n)
          (cons-stream n (integers-from (+ n 1))))

        (define naturals (integers-from 1))

        (stream-take 10 (stream-map (lambda (x) (* x x)) naturals))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items.len(), 10);
    assert_eq!(items[0], Value::Int(1));
    assert_eq!(items[1], Value::Int(4));
    assert_eq!(items[2], Value::Int(9));
    assert_eq!(items[9], Value::Int(100));
}

#[test]
fn sicp_symbolic_differentiator() {
    // SICP §2.3.2: Symbolic differentiation
    let result = eval(
        r#"
        (define (variable? x) (symbol? x))
        (define (same-variable? v1 v2)
          (and (variable? v1) (variable? v2) (eq? v1 v2)))

        (define (=number? exp num)
          (and (number? exp) (= exp num)))

        (define (make-sum a1 a2)
          (cond ((=number? a1 0) a2)
                ((=number? a2 0) a1)
                ((and (number? a1) (number? a2)) (+ a1 a2))
                (else (list '+ a1 a2))))

        (define (make-product m1 m2)
          (cond ((or (=number? m1 0) (=number? m2 0)) 0)
                ((=number? m1 1) m2)
                ((=number? m2 1) m1)
                ((and (number? m1) (number? m2)) (* m1 m2))
                (else (list '* m1 m2))))

        (define (sum? x)
          (and (pair? x) (eq? (car x) '+)))
        (define (addend s) (cadr s))
        (define (augend s) (caddr s))

        (define (product? x)
          (and (pair? x) (eq? (car x) '*)))
        (define (multiplier p) (cadr p))
        (define (multiplicand p) (caddr p))

        (define (deriv exp var)
          (cond ((number? exp) 0)
                ((variable? exp)
                 (if (same-variable? exp var) 1 0))
                ((sum? exp)
                 (make-sum (deriv (addend exp) var)
                           (deriv (augend exp) var)))
                ((product? exp)
                 (make-sum
                  (make-product (multiplier exp)
                                (deriv (multiplicand exp) var))
                  (make-product (deriv (multiplier exp) var)
                                (multiplicand exp))))
                (else (error "unknown expression type" exp))))

        ;; d/dx (x * x + 3 * x + 5) = 2x + 3
        (deriv '(+ (+ (* x x) (* 3 x)) 5) 'x)
    "#,
    );
    // Should simplify to (+ (+ x x) 3)
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::symbol("+"));
}

// ============================================================
// Classic: Towers of Hanoi
// ============================================================

#[test]
fn classic_towers_of_hanoi() {
    let result = eval(
        r#"
        (define moves '())
        (define (hanoi n from to via)
          (when (> n 0)
            (hanoi (- n 1) from via to)
            (set! moves (cons (list from to) moves))
            (hanoi (- n 1) via to from)))
        (hanoi 4 'A 'C 'B)
        (length moves)
    "#,
    );
    // 2^4 - 1 = 15 moves
    assert_eq!(result, Value::Int(15));
}

// ============================================================
// Classic: Game of Life
// ============================================================

#[test]
fn classic_game_of_life() {
    let result = eval(
        r#"
        (define (make-grid rows cols)
          (let ((grid (make-vector rows)))
            (do ((r 0 (+ r 1)))
                ((= r rows) grid)
              (vector-set! grid r (make-vector cols #f)))))

        (define (grid-ref grid r c)
          (vector-ref (vector-ref grid r) c))

        (define (grid-set! grid r c val)
          (vector-set! (vector-ref grid r) c val))

        (define (grid-rows grid) (vector-length grid))
        (define (grid-cols grid) (vector-length (vector-ref grid 0)))

        (define (count-neighbors grid r c)
          (let ((rows (grid-rows grid))
                (cols (grid-cols grid))
                (count 0))
            (do ((dr -1 (+ dr 1)))
                ((> dr 1) count)
              (do ((dc -1 (+ dc 1)))
                  ((> dc 1))
                (unless (and (= dr 0) (= dc 0))
                  (let ((nr (+ r dr))
                        (nc (+ c dc)))
                    (when (and (>= nr 0) (< nr rows)
                               (>= nc 0) (< nc cols)
                               (grid-ref grid nr nc))
                      (set! count (+ count 1)))))))))

        (define (step grid)
          (let* ((rows (grid-rows grid))
                 (cols (grid-cols grid))
                 (new (make-grid rows cols)))
            (do ((r 0 (+ r 1)))
                ((= r rows) new)
              (do ((c 0 (+ c 1)))
                  ((= c cols))
                (let ((n (count-neighbors grid r c))
                      (alive (grid-ref grid r c)))
                  (grid-set! new r c
                    (or (and alive (or (= n 2) (= n 3)))
                        (and (not alive) (= n 3)))))))))

        (define (count-alive grid)
          (let ((rows (grid-rows grid))
                (cols (grid-cols grid))
                (count 0))
            (do ((r 0 (+ r 1)))
                ((= r rows) count)
              (do ((c 0 (+ c 1)))
                  ((= c cols))
                (when (grid-ref grid r c)
                  (set! count (+ count 1)))))))

        ;; Blinker oscillator: period 2
        (let ((grid (make-grid 5 5)))
          (grid-set! grid 2 1 #t)
          (grid-set! grid 2 2 #t)
          (grid-set! grid 2 3 #t)
          (let ((g1 (step grid))
                (g2 (step (step grid))))
            ;; After 1 step: vertical, after 2 steps: back to horizontal
            (list (count-alive grid)
                  (count-alive g1)
                  (count-alive g2)
                  ;; Period 2: grid should equal g2
                  (grid-ref g2 2 1)
                  (grid-ref g2 2 2)
                  (grid-ref g2 2 3))))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(3)); // 3 alive initially
    assert_eq!(items[1], Value::Int(3)); // still 3 after step
    assert_eq!(items[2], Value::Int(3)); // still 3 after 2 steps
    assert_eq!(items[3], Value::Bool(true)); // back to original pattern
    assert_eq!(items[4], Value::Bool(true));
    assert_eq!(items[5], Value::Bool(true));
}

// ============================================================
// Classic: Huffman encoding (SICP §2.3.4)
// ============================================================

#[test]
fn sicp_huffman_encoding() {
    let result = eval(
        r#"
        ;; Huffman tree nodes
        (define (make-leaf symbol weight) (list 'leaf symbol weight))
        (define (leaf? x) (and (pair? x) (eq? (car x) 'leaf)))
        (define (symbol-leaf x) (cadr x))
        (define (weight-leaf x) (caddr x))

        (define (make-code-tree left right)
          (list left right
                (append (symbols left) (symbols right))
                (+ (weight left) (weight right))))

        (define (left-branch tree) (car tree))
        (define (right-branch tree) (cadr tree))

        (define (symbols tree)
          (if (leaf? tree) (list (symbol-leaf tree)) (caddr tree)))

        (define (weight tree)
          (if (leaf? tree) (weight-leaf tree) (cadddr tree)))

        ;; Decode bits against tree
        (define (decode bits tree)
          (define (decode-1 bits current-branch)
            (if (null? bits)
                '()
                (let ((next-branch
                       (if (= (car bits) 0)
                           (left-branch current-branch)
                           (right-branch current-branch))))
                  (if (leaf? next-branch)
                      (cons (symbol-leaf next-branch)
                            (decode-1 (cdr bits) tree))
                      (decode-1 (cdr bits) next-branch)))))
          (decode-1 bits tree))

        ;; Build a simple tree: A=0, B=10, C=11
        (let ((tree (make-code-tree
                     (make-leaf 'A 5)
                     (make-code-tree
                      (make-leaf 'B 2)
                      (make-leaf 'C 1)))))
          ;; Decode 0 10 11 0 = A B C A
          (decode '(0 1 0 1 1 0) tree))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0], Value::symbol("A"));
    assert_eq!(items[1], Value::symbol("B"));
    assert_eq!(items[2], Value::symbol("C"));
    assert_eq!(items[3], Value::symbol("A"));
}

// ============================================================
// Pattern: Object system via closures (message passing)
// ============================================================

#[test]
fn oop_message_passing() {
    let result = eval(
        r#"
        (define (make-account balance)
          (define (withdraw amount)
            (if (>= balance amount)
                (begin (set! balance (- balance amount)) balance)
                (error "Insufficient funds")))
          (define (deposit amount)
            (set! balance (+ balance amount))
            balance)
          (define (dispatch msg)
            (cond ((eq? msg 'withdraw) withdraw)
                  ((eq? msg 'deposit) deposit)
                  ((eq? msg 'balance) balance)
                  (else (error "Unknown message" msg))))
          dispatch)

        (let ((acc (make-account 100)))
          (list ((acc 'deposit) 50)
                ((acc 'withdraw) 30)
                (acc 'balance)))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(150));
    assert_eq!(items[1], Value::Int(120));
    assert_eq!(items[2], Value::Int(120));
}

// ============================================================
// Algorithm: Merge sort
// ============================================================

#[test]
fn algorithm_merge_sort() {
    let result = eval(
        r#"
        (define (merge lst1 lst2)
          (cond ((null? lst1) lst2)
                ((null? lst2) lst1)
                ((<= (car lst1) (car lst2))
                 (cons (car lst1) (merge (cdr lst1) lst2)))
                (else
                 (cons (car lst2) (merge lst1 (cdr lst2))))))

        (define (split lst)
          (let loop ((l lst) (a '()) (b '()) (toggle #t))
            (if (null? l)
                (list (reverse a) (reverse b))
                (if toggle
                    (loop (cdr l) (cons (car l) a) b #f)
                    (loop (cdr l) a (cons (car l) b) #t)))))

        (define (merge-sort lst)
          (if (or (null? lst) (null? (cdr lst)))
              lst
              (let ((halves (split lst)))
                (merge (merge-sort (car halves))
                       (merge-sort (cadr halves))))))

        (merge-sort '(5 3 8 1 9 2 7 4 6))
    "#,
    );
    let items = result.to_vec().unwrap();
    let expected: Vec<i64> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
    for (i, v) in items.iter().enumerate() {
        assert_eq!(*v, Value::Int(expected[i]));
    }
}

// ============================================================
// Algorithm: Red-black tree (simplified — insert only)
// ============================================================

#[test]
fn algorithm_balanced_tree() {
    // AVL-like balanced BST using pure functional approach
    let result = eval(
        r#"
        ;; BST: (val left right)
        (define (make-node val left right) (list val left right))
        (define (node-val t) (car t))
        (define (node-left t) (cadr t))
        (define (node-right t) (caddr t))
        (define (empty? t) (null? t))

        (define (insert t val)
          (if (empty? t)
              (make-node val '() '())
              (let ((v (node-val t)))
                (cond ((< val v)
                       (make-node v (insert (node-left t) val) (node-right t)))
                      ((> val v)
                       (make-node v (node-left t) (insert (node-right t) val)))
                      (else t)))))

        (define (inorder t)
          (if (empty? t)
              '()
              (append (inorder (node-left t))
                      (list (node-val t))
                      (inorder (node-right t)))))

        (define (tree-member? t val)
          (if (empty? t)
              #f
              (let ((v (node-val t)))
                (cond ((= val v) #t)
                      ((< val v) (tree-member? (node-left t) val))
                      (else (tree-member? (node-right t) val))))))

        (let ((tree (insert (insert (insert (insert (insert '() 5) 3) 7) 1) 9)))
          (list (inorder tree)
                (tree-member? tree 7)
                (tree-member? tree 4)))
    "#,
    );
    let items = result.to_vec().unwrap();
    let sorted = items[0].to_vec().unwrap();
    assert_eq!(
        sorted,
        vec![
            Value::Int(1),
            Value::Int(3),
            Value::Int(5),
            Value::Int(7),
            Value::Int(9)
        ]
    );
    assert_eq!(items[1], Value::Bool(true));
    assert_eq!(items[2], Value::Bool(false));
}

// ============================================================
// Pattern: Continuation-based state machine
// ============================================================

#[test]
fn continuation_escape() {
    // call/cc for non-local exit (escape continuation)
    let result = eval(
        r#"
        ;; Use call/cc as an early return
        (define (find-first pred lst)
          (call/cc (lambda (return)
            (for-each (lambda (x)
              (when (pred x) (return x)))
              lst)
            #f)))

        (list (find-first even? '(1 3 5 4 7))    ;; → 4
              (find-first even? '(1 3 5 7))       ;; → #f
              (find-first (lambda (x) (> x 10)) '(1 20 3)) ;; → 20
              ;; Nested: find first pair where sum > 10
              (find-first (lambda (x) (> (+ (car x) (cdr x)) 10))
                          '((1 . 2) (5 . 7) (3 . 4)))) ;; → (5 . 7)
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(4));
    assert_eq!(items[1], Value::Bool(false));
    assert_eq!(items[2], Value::Int(20));
}

// ============================================================
// Pattern: Monad-like computation (Maybe monad)
// ============================================================

#[test]
fn pattern_maybe_monad() {
    let result = eval(
        r#"
        ;; Maybe monad: #f = Nothing, value = Just value
        (define (maybe-bind m f)
          (if m (f m) #f))

        (define (maybe-return x) x)

        (define (safe-div a b)
          (if (= b 0) #f (/ a b)))

        (define (safe-sqrt x)
          (if (< x 0) #f (sqrt x)))

        ;; Chain: 100 / 4 = 25, sqrt(25) = 5
        (define result1
          (maybe-bind (safe-div 100 4)
            (lambda (x) (safe-sqrt x))))

        ;; Chain: 100 / 0 = Nothing, whole chain fails
        (define result2
          (maybe-bind (safe-div 100 0)
            (lambda (x) (safe-sqrt x))))

        ;; Chain: sqrt(-1) = Nothing
        (define result3
          (maybe-bind (maybe-return -1)
            safe-sqrt))

        (list result1 result2 result3)
    "#,
    );
    let items = result.to_vec().unwrap();
    // sqrt(25) may return Int(5) or Float(5.0) depending on implementation
    let r1 = items[0]
        .as_int()
        .unwrap_or_else(|_| items[0].as_float().unwrap() as i64);
    assert_eq!(r1, 5);
    assert_eq!(items[1], Value::Bool(false));
    assert_eq!(items[2], Value::Bool(false));
}

// ============================================================
// Pattern: Parser combinator
// ============================================================

#[test]
fn pattern_parser_combinators() {
    let result = eval(
        r#"
        ;; Simple parser combinator library
        ;; A parser takes a string and position, returns (value . new-pos) or #f

        (define (parse-char pred)
          (lambda (str pos)
            (if (>= pos (string-length str))
                #f
                (let ((c (string-ref str pos)))
                  (if (pred c)
                      (cons c (+ pos 1))
                      #f)))))

        (define (parse-seq . parsers)
          (lambda (str pos)
            (let loop ((ps parsers) (p pos) (acc '()))
              (if (null? ps)
                  (cons (reverse acc) p)
                  (let ((result ((car ps) str p)))
                    (if result
                        (loop (cdr ps) (cdr result) (cons (car result) acc))
                        #f))))))

        (define (parse-many parser)
          (lambda (str pos)
            (let loop ((p pos) (acc '()))
              (let ((result (parser str p)))
                (if result
                    (loop (cdr result) (cons (car result) acc))
                    (cons (reverse acc) p))))))

        (define digit? (lambda (c) (and (char>=? c #\0) (char<=? c #\9))))
        (define letter? char-alphabetic?)

        (define parse-digit (parse-char digit?))
        (define parse-letter (parse-char letter?))
        (define parse-digits (parse-many parse-digit))

        ;; Parse "abc" at position 0
        (let ((r1 ((parse-seq parse-letter parse-letter parse-letter) "abc123" 0))
              (r2 (parse-digits "123abc" 0)))
          (list (car r1) (cdr r1)       ;; (#\a #\b #\c) . 3
                (car r2) (cdr r2)))     ;; (#\1 #\2 #\3) . 3
    "#,
    );
    let items = result.to_vec().unwrap();
    let letters = items[0].to_vec().unwrap();
    assert_eq!(
        letters,
        vec![Value::Char('a'), Value::Char('b'), Value::Char('c')]
    );
    assert_eq!(items[1], Value::Int(3));
    let digits = items[2].to_vec().unwrap();
    assert_eq!(
        digits,
        vec![Value::Char('1'), Value::Char('2'), Value::Char('3')]
    );
    assert_eq!(items[3], Value::Int(3));
}

// ============================================================
// Algorithm: Topological sort
// ============================================================

#[test]
fn algorithm_topological_sort() {
    let result = eval(
        r#"
        ;; Adjacency list graph: ((node . (neighbors ...)) ...)
        (define (neighbors node graph)
          (let ((entry (assq node graph)))
            (if entry (cdr entry) '())))

        (define (topological-sort graph)
          (let ((visited '())
                (result '()))
            (define (visit node)
              (unless (memq node visited)
                (set! visited (cons node visited))
                (for-each visit (neighbors node graph))
                (set! result (cons node result))))
            (for-each (lambda (entry) (visit (car entry))) graph)
            (reverse result)))

        ;; Build order: A depends on B, C; B depends on D; C depends on D
        (define (index-of item lst)
          (let loop ((l lst) (i 0))
            (cond ((null? l) -1)
                  ((eq? (car l) item) i)
                  (else (loop (cdr l) (+ i 1))))))

        (let ((graph '((A . (B C))
                       (B . (D))
                       (C . (D))
                       (D . ()))))
          (let ((order (topological-sort graph)))
            ;; Return order for inspection
            order))
    "#,
    );
    // Verify it's a valid topological order
    let items = result.to_vec().unwrap();
    assert_eq!(items.len(), 4);
    // Find positions
    let pos = |sym: &str| items.iter().position(|v| *v == Value::symbol(sym)).unwrap();
    let (pa, pb, pc, pd) = (pos("A"), pos("B"), pos("C"), pos("D"));
    // D must come before B and C; B and C must come before A
    assert!(
        pd < pb,
        "D at {pd} should be before B at {pb}, order: {result}"
    );
    assert!(
        pd < pc,
        "D at {pd} should be before C at {pc}, order: {result}"
    );
    assert!(
        pb < pa,
        "B at {pb} should be before A at {pa}, order: {result}"
    );
    assert!(
        pc < pa,
        "C at {pc} should be before A at {pa}, order: {result}"
    );
}

// ============================================================
// Pattern: Macro-defined DSL (simple pattern matching)
// ============================================================

#[test]
fn macro_dsl_pattern_match() {
    let result = eval(
        r#"
        ;; define-syntax for a simple match expression
        (define-syntax my-match
          (syntax-rules ()
            ((_ expr
               ((pattern) body) ...)
             (let ((val expr))
               (cond
                 ((equal? val 'pattern) body) ...
                 (else (error "no match" val)))))))

        ;; Use the match macro
        (define (describe-shape shape)
          (my-match shape
            ((circle) "round")
            ((square) "boxy")
            ((triangle) "pointy")))

        (list (describe-shape 'circle)
              (describe-shape 'square)
              (describe-shape 'triangle))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::string("round"));
    assert_eq!(items[1], Value::string("boxy"));
    assert_eq!(items[2], Value::string("pointy"));
}

// ============================================================
// Classic: Church numerals
// ============================================================

#[test]
fn classic_church_numerals() {
    let result = eval(
        r#"
        (define zero (lambda (f) (lambda (x) x)))
        (define (succ n) (lambda (f) (lambda (x) (f ((n f) x)))))
        (define (church->int n) ((n (lambda (x) (+ x 1))) 0))
        (define (church-add a b) (lambda (f) (lambda (x) ((a f) ((b f) x)))))
        (define (church-mult a b) (lambda (f) (a (b f))))

        (define one (succ zero))
        (define two (succ one))
        (define three (succ two))

        (list (church->int zero)
              (church->int one)
              (church->int two)
              (church->int three)
              (church->int (church-add two three))
              (church->int (church-mult two three)))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(0));
    assert_eq!(items[1], Value::Int(1));
    assert_eq!(items[2], Value::Int(2));
    assert_eq!(items[3], Value::Int(3));
    assert_eq!(items[4], Value::Int(5));
    assert_eq!(items[5], Value::Int(6));
}

// ============================================================
// Classic: Y combinator
// ============================================================

#[test]
fn classic_y_combinator() {
    let result = eval(
        r#"
        ;; Z combinator (applicative-order Y)
        (define Z
          (lambda (f)
            ((lambda (x) (f (lambda (v) ((x x) v))))
             (lambda (x) (f (lambda (v) ((x x) v)))))))

        ;; Factorial via Z combinator (no explicit recursion)
        (define factorial
          (Z (lambda (self)
               (lambda (n)
                 (if (<= n 1) 1 (* n (self (- n 1))))))))

        (list (factorial 0) (factorial 1) (factorial 5) (factorial 10))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(1));
    assert_eq!(items[1], Value::Int(1));
    assert_eq!(items[2], Value::Int(120));
    assert_eq!(items[3], Value::Int(3628800));
}

// ============================================================
// Classic: Sieve of Eratosthenes (functional)
// ============================================================

#[test]
fn classic_functional_sieve() {
    let result = eval(
        r#"
        (define (sieve lst)
          (if (null? lst)
              '()
              (let ((p (car lst)))
                (cons p
                      (sieve (filter (lambda (n) (not (= 0 (modulo n p))))
                                     (cdr lst)))))))

        (define (range a b)
          (if (> a b) '()
              (cons a (range (+ a 1) b))))

        (sieve (range 2 30))
    "#,
    );
    let items = result.to_vec().unwrap();
    let primes: Vec<i64> = items.iter().map(|v| v.as_int().unwrap()).collect();
    assert_eq!(primes, vec![2, 3, 5, 7, 11, 13, 17, 19, 23, 29]);
}

// ============================================================
// Classic: Matrix operations
// ============================================================

#[test]
fn classic_matrix_operations() {
    let result = eval(
        r#"
        ;; Matrix as vector of vectors
        (define (make-matrix rows cols init)
          (let ((m (make-vector rows)))
            (do ((r 0 (+ r 1)))
                ((= r rows) m)
              (vector-set! m r (make-vector cols init)))))

        (define (matrix-ref m r c) (vector-ref (vector-ref m r) c))
        (define (matrix-set! m r c v) (vector-set! (vector-ref m r) c v))

        (define (matrix-multiply a b rows-a cols-a cols-b)
          (let ((result (make-matrix rows-a cols-b 0)))
            (do ((i 0 (+ i 1)))
                ((= i rows-a) result)
              (do ((j 0 (+ j 1)))
                  ((= j cols-b))
                (do ((k 0 (+ k 1)))
                    ((= k cols-a))
                  (matrix-set! result i j
                    (+ (matrix-ref result i j)
                       (* (matrix-ref a i k)
                          (matrix-ref b k j)))))))))

        ;; 2x2 identity * [[1,2],[3,4]] = [[1,2],[3,4]]
        (let ((I (make-matrix 2 2 0))
              (A (make-matrix 2 2 0)))
          (matrix-set! I 0 0 1) (matrix-set! I 1 1 1)
          (matrix-set! A 0 0 1) (matrix-set! A 0 1 2)
          (matrix-set! A 1 0 3) (matrix-set! A 1 1 4)
          (let ((R (matrix-multiply I A 2 2 2)))
            (list (matrix-ref R 0 0) (matrix-ref R 0 1)
                  (matrix-ref R 1 0) (matrix-ref R 1 1))))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(1));
    assert_eq!(items[1], Value::Int(2));
    assert_eq!(items[2], Value::Int(3));
    assert_eq!(items[3], Value::Int(4));
}

// ============================================================
// Record types: Linked list with records
// ============================================================

#[test]
fn record_type_linked_list() {
    let result = eval(
        r#"
        (define-record-type <node>
          (make-node value next)
          node?
          (value node-value)
          (next node-next))

        (define (list->linked-list lst)
          (if (null? lst)
              #f
              (make-node (car lst) (list->linked-list (cdr lst)))))

        (define (linked-list->list ll)
          (if (not ll)
              '()
              (cons (node-value ll)
                    (linked-list->list (node-next ll)))))

        (define (linked-length ll)
          (if (not ll) 0 (+ 1 (linked-length (node-next ll)))))

        (let ((ll (list->linked-list '(10 20 30 40 50))))
          (list (linked-length ll)
                (node-value ll)
                (node-value (node-next ll))
                (linked-list->list ll)))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(5));
    assert_eq!(items[1], Value::Int(10));
    assert_eq!(items[2], Value::Int(20));
    let converted = items[3].to_vec().unwrap();
    assert_eq!(converted.len(), 5);
    assert_eq!(converted[0], Value::Int(10));
    assert_eq!(converted[4], Value::Int(50));
}

// ============================================================
// Comprehensive: CPS transform (continuation-passing style)
// ============================================================

#[test]
fn pattern_cps_transform() {
    let result = eval(
        r#"
        ;; CPS versions of basic operations
        (define (add-cps a b k) (k (+ a b)))
        (define (mul-cps a b k) (k (* a b)))
        (define (sub-cps a b k) (k (- a b)))

        ;; CPS factorial
        (define (fact-cps n k)
          (if (= n 0)
              (k 1)
              (fact-cps (- n 1)
                        (lambda (r) (k (* n r))))))

        ;; CPS fibonacci
        (define (fib-cps n k)
          (if (<= n 1)
              (k n)
              (fib-cps (- n 1)
                       (lambda (a)
                         (fib-cps (- n 2)
                                  (lambda (b)
                                    (k (+ a b))))))))

        ;; Compute (3 + 4) * (5 - 2) = 7 * 3 = 21 in CPS
        (define cps-result #f)
        (add-cps 3 4
          (lambda (sum)
            (sub-cps 5 2
              (lambda (diff)
                (mul-cps sum diff
                  (lambda (r) (set! cps-result r)))))))

        (list cps-result
              (fact-cps 10 (lambda (x) x))
              (fib-cps 10 (lambda (x) x)))
    "#,
    );
    let items = result.to_vec().unwrap();
    assert_eq!(items[0], Value::Int(21));
    assert_eq!(items[1], Value::Int(3628800));
    assert_eq!(items[2], Value::Int(55));
}
