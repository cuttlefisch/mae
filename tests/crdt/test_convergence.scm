;;; test_convergence.scm — Two-buffer CRDT convergence test
;;;
;;; Tests that two buffers with separate client IDs can exchange sync
;;; updates and converge to the same content.

(define *test-state-a* #f)
(define *test-updates-b* (list))

(describe-group "Two-buffer CRDT convergence"
  (lambda ()
    (it-test "setup buffer A"
      (lambda ()
        (create-buffer "*crdt-a*")))

    (it-test "enable sync on A"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "insert text into A"
      (lambda ()
        (buffer-insert "hello from A")))

    (it-test "buffer A has correct content"
      (lambda ()
        (should-equal (buffer-string) "hello from A")))

    (it-test "encode state from A"
      (lambda ()
        (set! *test-state-a* (buffer-encode-state))
        (should *test-state-a*)))

    (it-test "create buffer B"
      (lambda ()
        (create-buffer "*crdt-b*")))

    (it-test "load A's state into B"
      (lambda ()
        (buffer-load-sync-state *test-state-a* 2)))

    (it-test "B has A's content"
      (lambda ()
        (should-equal (buffer-string) "hello from A")))

    (it-test "move cursor to end in B"
      (lambda ()
        (goto-char 12)))

    (it-test "B inserts additional text"
      (lambda ()
        (buffer-insert " and B")))

    (it-test "B content is correct after edit"
      (lambda ()
        (should-equal (buffer-string) "hello from A and B")))

    (it-test "request drain of B's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve B's drained updates"
      (lambda ()
        (set! *test-updates-b* (buffer-drain-updates))
        (should (> (length *test-updates-b*) 0))))

    (it-test "switch to buffer A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*crdt-a*"))))

    (it-test "apply each update from B to A"
      (lambda ()
        (define (apply-all lst)
          (if (null? lst) #t
              (begin
                (buffer-apply-update "*crdt-a*" (car lst))
                (apply-all (cdr lst)))))
        (apply-all *test-updates-b*)))

    (it-test "A converged with B's edit"
      (lambda ()
        (should-contain (buffer-string) "and B")))))
