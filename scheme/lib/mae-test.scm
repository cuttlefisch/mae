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

;; (await-condition PRED TIMEOUT-MS) — yield-tick based condition wait.
;; Unlike wait-until (which sleeps between polls), this yields to the event
;; loop after each check, allowing hooks and side effects to process.
;; Use this when waiting for state that changes through hooks or commands.
;; Returns #t on success, signals error on timeout.
(define (await-condition pred timeout-ms)
  (define start (current-milliseconds))
  (define (loop)
    (if (pred)
        #t
        (if (>= (- (current-milliseconds) start) timeout-ms)
            (error (string-append "await-condition timed out after "
                                  (number->string timeout-ms) "ms"))
            (begin
              (yield-tick)
              (loop)))))
  (loop))

;; wait-for-file is a native yield primitive registered by (mae async).
;; It yields to the host event loop, which can drain collab/shell events
;; during the wait. No Scheme wrapper needed.
;;
;; yield-tick and await-hook are native yield primitives from (mae async).
;; yield-tick: yield for one event loop iteration (hooks/side effects drain).
;; await-hook: suspend until a named hook fires (or timeout).
;; await-condition: Scheme helper that polls a predicate using yield-tick.

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

;; --- Auto-flush wrappers ---
;;
;; In the real editor, the event loop calls apply_to_editor after every
;; Scheme eval, so buffer mutations take effect automatically. In tests,
;; the runner simulates this between it-test steps. To allow multiple
;; mutations within a single test, we wrap mutating functions to yield
;; (flush!) after each call. The test runner catches the yield, applies
;; pending ops, refreshes state, and resumes — making mutations appear
;; immediate.
;;
;; This only affects test mode. In the real editor, flush! is a no-op
;; yield that blocks and resumes immediately.

(define %raw-buffer-insert buffer-insert)
(define (buffer-insert text) (%raw-buffer-insert text) (flush!))

(define %raw-goto-char goto-char)
(define (goto-char offset) (%raw-goto-char offset) (flush!))

(define %raw-cursor-goto cursor-goto)
(define (cursor-goto row col) (%raw-cursor-goto row col) (flush!))

(define %raw-create-buffer create-buffer)
(define (create-buffer name) (%raw-create-buffer name) (flush!))

(define %raw-run-command run-command)
(define (run-command name) (%raw-run-command name) (flush!))

(define %raw-execute-ex execute-ex)
(define (execute-ex cmd) (%raw-execute-ex cmd) (flush!))

(define %raw-open-file open-file)
(define (open-file path) (%raw-open-file path) (flush!))

(define %raw-buffer-delete-range buffer-delete-range)
(define (buffer-delete-range start end) (%raw-buffer-delete-range start end) (flush!))

(define %raw-buffer-replace-range buffer-replace-range)
(define (buffer-replace-range start end text) (%raw-buffer-replace-range start end text) (flush!))

(define %raw-buffer-undo buffer-undo)
(define (buffer-undo) (%raw-buffer-undo) (flush!))

(define %raw-buffer-redo buffer-redo)
(define (buffer-redo) (%raw-buffer-redo) (flush!))

(define %raw-buffer-undo-boundary buffer-undo-boundary)
(define (buffer-undo-boundary) (%raw-buffer-undo-boundary) (flush!))

(define %raw-buffer-enable-sync buffer-enable-sync)
(define (buffer-enable-sync client-id) (%raw-buffer-enable-sync client-id) (flush!))

(define %raw-buffer-disable-sync buffer-disable-sync)
(define (buffer-disable-sync) (%raw-buffer-disable-sync) (flush!))

(define %raw-switch-to-buffer switch-to-buffer)
(define (switch-to-buffer idx) (%raw-switch-to-buffer idx) (flush!))

(define %raw-set-option! set-option!)
(define (set-option! key val) (%raw-set-option! key val) (flush!))

(define %raw-add-hook! add-hook!)
(define (add-hook! hook fn) (%raw-add-hook! hook fn) (flush!))

(define %raw-remove-hook! remove-hook!)
(define (remove-hook! hook fn) (%raw-remove-hook! hook fn) (flush!))

(define %raw-advice-add! advice-add!)
(define (advice-add! cmd kind fn) (%raw-advice-add! cmd kind fn) (flush!))

(define %raw-advice-remove! advice-remove!)
(define (advice-remove! cmd fn) (%raw-advice-remove! cmd fn) (flush!))

(define %raw-buffer-load-sync-state buffer-load-sync-state)
(define (buffer-load-sync-state state client-id)
  (%raw-buffer-load-sync-state state client-id) (flush!))

(define %raw-buffer-encode-state-vector buffer-encode-state-vector)
(define (buffer-encode-state-vector) (%raw-buffer-encode-state-vector) (flush!))

(define %raw-buffer-compute-diff buffer-compute-diff)
(define (buffer-compute-diff sv) (%raw-buffer-compute-diff sv) (flush!))

(define %raw-buffer-reconcile-to buffer-reconcile-to)
(define (buffer-reconcile-to target) (%raw-buffer-reconcile-to target) (flush!))

(define %raw-buffer-apply-update buffer-apply-update)
(define (buffer-apply-update buf-name update)
  (%raw-buffer-apply-update buf-name update) (flush!))

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
