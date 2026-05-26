;;; mae-test.scm — Scheme testing library for MAE
;;;
;;; Modeled after Emacs ERT + Buttercup:
;;;   - define-test / describe-group / it-test — test registration
;;;   - should / should-not / should-equal / should-contain — assertions
;;;   - wait-until — async polling (event-loop-aware via sleep-ms)
;;;   - run-tests — execute all registered tests, print TAP output, exit
;;;
;;; Usage:
;;;   mae --test tests/my-test.scm
;;;
;;; The --test CLI mode auto-loads this library before evaluating test files.

;; --- Test registry ---

(define *test-registry* (list))
(define *test-results* (list))
(define *current-describe* "")
(define *before-each-fns* (list))
(define *after-each-fns* (list))

;; --- Utility ---

;; (string-contains? STR SUB) — check if STR contains SUB.
(define (string-contains? str sub)
  (let ((str-len (string-length str))
        (sub-len (string-length sub)))
    (if (> sub-len str-len)
        #f
        (let loop ((i 0))
          (cond
            ((> (+ i sub-len) str-len) #f)
            ((equal? (substring str i (+ i sub-len)) sub) #t)
            (else (loop (+ i 1))))))))

;; (to-string VAL) — convert any value to a string representation.
(define (to-string val)
  (cond
    ((string? val) val)
    ((number? val) (number->string val))
    ((boolean? val) (if val "#t" "#f"))
    ((symbol? val) (symbol->string val))
    ((error-object? val) (error-object-message val))
    (else
      (guard (exn (#t "<?>"))
        (error-object-message val)))))

;; --- Test registration ---

;; (register-test! NAME THUNK) — register a named test.
(define (register-test! name thunk)
  (set! *test-registry*
    (append *test-registry* (list (list name thunk)))))

;; (describe-group NAME THUNK) — BDD grouping. Sets the group prefix for
;; nested `it-test` blocks.
(define (describe-group name thunk)
  (let ((prev-describe *current-describe*)
        (prev-before *before-each-fns*)
        (prev-after *after-each-fns*))
    (set! *current-describe*
      (if (equal? prev-describe "")
          name
          (string-append prev-describe " > " name)))
    (thunk)
    (set! *current-describe* prev-describe)
    (set! *before-each-fns* prev-before)
    (set! *after-each-fns* prev-after)))

;; (it-test NAME THUNK) — register a test within a describe block.
(define (it-test name thunk)
  (let ((full-name (if (equal? *current-describe* "")
                       name
                       (string-append *current-describe* " > " name))))
    (register-test! full-name thunk)))

;; (before-each HOOK-FN) — register setup function for current describe scope.
(define (before-each hook-fn)
  (set! *before-each-fns* (append *before-each-fns* (list hook-fn))))

;; (after-each HOOK-FN) — register teardown function for current describe scope.
(define (after-each hook-fn)
  (set! *after-each-fns* (append *after-each-fns* (list hook-fn))))

;; --- Assertions ---

(define *assertion-count* 0)

;; (should VAL) — assert VAL is truthy. Signals error on failure.
(define (should val)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not val)
      (error "Assertion failed: expected truthy value")
      #t))

;; (should-not VAL) — assert VAL is falsy.
(define (should-not val)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if val
      (error "Assertion failed: expected falsy value")
      #t))

;; (should-equal A B) — assert A equals B.
(define (should-equal a b)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not (equal? a b))
      (error (string-append "Assertion failed: expected "
                            (to-string b) " got " (to-string a)))
      #t))

;; (should-contain HAYSTACK NEEDLE) — assert string contains substring.
(define (should-contain haystack needle)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not (string-contains? haystack needle))
      (error (string-append "Assertion failed: expected '" needle "' in string"))
      #t))

;; (should-error THUNK) — assert THUNK signals an error. Passes if error raised,
;; fails if THUNK returns normally.
(define (should-error thunk)
  (set! *assertion-count* (+ *assertion-count* 1))
  (guard (exn (#t #t))
    (thunk)
    (error "Expected error but none was raised")))

;; (should-match HAYSTACK PATTERN) — assert HAYSTACK contains PATTERN substring.
;; Alias for should-contain with a more descriptive name for pattern-like usage.
(define (should-match haystack pattern)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not (string-contains? haystack pattern))
      (error (string-append "Expected match for '" pattern "' in: "
                            (substring haystack 0 (min (string-length haystack) 80))))
      #t))

;; (should-mode EXPECTED) — assert current editor mode matches expected string.
(define (should-mode expected)
  (should-equal (current-mode) expected))

;; (should-greater-than A B) — assert A > B (numeric).
(define (should-greater-than a b)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not (> a b))
      (error (string-append "Assertion failed: expected "
                            (to-string a) " > " (to-string b)))
      #t))

;; (should-less-than A B) — assert A < B (numeric).
(define (should-less-than a b)
  (set! *assertion-count* (+ *assertion-count* 1))
  (if (not (< a b))
      (error (string-append "Assertion failed: expected "
                            (to-string a) " < " (to-string b)))
      #t))

;; (should-buffer-state TEXT ROW COL) — combined buffer content + cursor check.
;; Uses SharedState-backed test primitives directly (always available).
(define (should-buffer-state text row col)
  (should-equal (test-buffer-string) text)
  (should-equal (test-cursor-row) row)
  (should-equal (test-cursor-col) col))

;; --- Async helpers ---

;; (wait-until PRED TIMEOUT-MS) — poll PRED every 50ms, sleeping between checks.
;; The test runner handles sleep-ms by draining collab/shell events.
;; Returns #t on success, signals error on timeout.
(define (wait-until pred timeout-ms)
  (define (loop elapsed)
    (if (pred)
        #t
        (if (>= elapsed timeout-ms)
            (error (string-append "wait-until timed out after "
                                  (number->string timeout-ms) "ms"))
            (begin
              (sleep-ms 50)
              (loop (+ elapsed 50))))))
  (loop 0))

;; wait-for-file is a native yield primitive registered by (mae async).
;; It yields to the host event loop, which can drain collab/shell events
;; during the wait. No Scheme wrapper needed.

;; --- Test runner ---

;; Helper to run hooks.
(define (run-hook-list hooks)
  (if (null? hooks)
      #t
      (begin
        ((car hooks))
        (run-hook-list (cdr hooks)))))

;; Run a single test, catching errors. Returns (STATUS NAME MESSAGE).
(define (run-single-test name thunk)
  ;; Run before-each hooks
  (run-hook-list *before-each-fns*)
  (let ((status "PASS")
        (msg ""))
    (guard (err
            (#t (set! status "FAIL")
                (set! msg (to-string err))))
      (thunk))
    ;; Run after-each hooks
    (run-hook-list *after-each-fns*)
    (list status name msg)))

;; --- Rust-side iteration API ---
;; These allow the test runner to iterate tests from Rust,
;; calling inject_editor_state + apply_to_editor between each test.

;; (test-count) — number of registered tests.
(define (test-count)
  (length *test-registry*))

;; (list-ref LST N) — get Nth element of a list (0-indexed).
(define (list-ref-safe lst n)
  (if (= n 0) (car lst) (list-ref-safe (cdr lst) (- n 1))))

;; (test-name N) — return the name of the Nth test (0-indexed).
(define (test-name n)
  (car (list-ref-safe *test-registry* n)))

;; (run-nth-test N) — run the Nth test (0-indexed).
;; Returns "PASS" or "FAIL:message".
(define (run-nth-test n)
  (let* ((entry (list-ref-safe *test-registry* n))
         (name (car entry))
         (thunk (car (cdr entry)))
         (result (run-single-test name thunk))
         (status (car result))
         (msg (car (cdr (cdr result)))))
    (if (equal? status "PASS")
        "PASS"
        (string-append "FAIL:" msg))))

;; (run-tests) — execute all registered tests, print TAP output, exit.
(define (run-tests)
  (define total (length *test-registry*))
  (define pass-count 0)
  (define fail-count 0)
  (define test-num 0)
  ;; TAP header
  (display "TAP version 14")
  (newline)
  (display (string-append "1.." (number->string total)))
  (newline)
  (define (run-entry entry)
    (define name (car entry))
    (define thunk (car (cdr entry)))
    (define result (run-single-test name thunk))
    (define s (car result))
    (define m (car (cdr (cdr result))))
    (set! test-num (+ test-num 1))
    (if (equal? s "PASS")
        (begin
          (set! pass-count (+ pass-count 1))
          (display (string-append "ok " (number->string test-num) " - " name))
          (newline))
        (begin
          (set! fail-count (+ fail-count 1))
          (display (string-append "not ok " (number->string test-num) " - " name))
          (newline)
          (display "  ---")
          (newline)
          (display (string-append "  message: " m))
          (newline)
          (display "  ...")
          (newline))))
  (define (run-all entries)
    (if (null? entries)
        #t
        (begin
          (run-entry (car entries))
          (run-all (cdr entries)))))
  (run-all *test-registry*)
  ;; Summary
  (newline)
  (display (string-append "# " (number->string pass-count) " passed, "
                         (number->string fail-count) " failed, "
                         (number->string *assertion-count*) " assertions"))
  (newline)
  (exit (if (= fail-count 0) 0 1)))
