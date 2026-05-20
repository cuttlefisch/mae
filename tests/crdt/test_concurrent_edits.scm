;;; test_concurrent_edits.scm — Concurrent insert convergence test
;;;
;;; Two buffers with the same initial state each insert at position 0
;;; concurrently. They exchange updates and must converge to identical
;;; content (CRDT interleaving order determined by client-id in YATA).

(define *concurrent-state-a* #f)
(define *concurrent-updates-a* (list))
(define *concurrent-updates-b* (list))

(describe-group "Concurrent inserts converge"
  (lambda ()
    (it-test "setup buffer A"
      (lambda ()
        (create-buffer "*concurrent-a*")))

    (it-test "enable sync on A (client 1)"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "insert shared initial text into A"
      (lambda ()
        (buffer-insert "base")))

    (it-test "A has correct initial content"
      (lambda ()
        (should-equal (buffer-string) "base")))

    (it-test "encode A's state for seeding B"
      (lambda ()
        (set! *concurrent-state-a* (buffer-encode-state))
        (should *concurrent-state-a*)))

    (it-test "create buffer B"
      (lambda ()
        (create-buffer "*concurrent-b*")))

    (it-test "load A's state into B (client 2)"
      (lambda ()
        (buffer-load-sync-state *concurrent-state-a* 2)))

    (it-test "B has the shared initial content"
      (lambda ()
        (should-equal (buffer-string) "base")))

    ;; B inserts at position 0
    (it-test "B moves to position 0"
      (lambda ()
        (goto-char 0)))

    (it-test "B inserts its concurrent text"
      (lambda ()
        (buffer-insert "B:")))

    ;; Drain B's updates (two-step pattern)
    (it-test "request drain of B's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve B's drained updates"
      (lambda ()
        (set! *concurrent-updates-b* (buffer-drain-updates))
        (should (> (length *concurrent-updates-b*) 0))))

    (it-test "switch to buffer A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*concurrent-a*"))))

    ;; A inserts at position 0 (concurrent)
    (it-test "A moves to position 0"
      (lambda ()
        (goto-char 0)))

    (it-test "A inserts its concurrent text"
      (lambda ()
        (buffer-insert "A:")))

    ;; Drain A's updates (two-step pattern)
    (it-test "request drain of A's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve A's drained updates"
      (lambda ()
        (set! *concurrent-updates-a* (buffer-drain-updates))
        (should (> (length *concurrent-updates-a*) 0))))

    ;; Exchange: apply B's updates to A, then A's updates to B.
    (it-test "apply B's updates to A"
      (lambda ()
        (define (apply-all lst)
          (if (null? lst) #t
              (begin
                (buffer-apply-update "*concurrent-a*" (car lst))
                (apply-all (cdr lst)))))
        (apply-all *concurrent-updates-b*)))

    (it-test "switch to buffer B"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*concurrent-b*"))))

    (it-test "apply A's updates to B"
      (lambda ()
        (define (apply-all lst)
          (if (null? lst) #t
              (begin
                (buffer-apply-update "*concurrent-b*" (car lst))
                (apply-all (cdr lst)))))
        (apply-all *concurrent-updates-a*)))

    (it-test "A and B have identical content after convergence"
      (lambda ()
        (should-equal (buffer-text "*concurrent-a*")
                      (buffer-text "*concurrent-b*"))))

    (it-test "converged content contains A's insert"
      (lambda ()
        (should-contain (buffer-text "*concurrent-a*") "A:")))

    (it-test "converged content contains B's insert"
      (lambda ()
        (should-contain (buffer-text "*concurrent-a*") "B:")))

    (it-test "converged content contains the shared base"
      (lambda ()
        (should-contain (buffer-text "*concurrent-a*") "base")))))
