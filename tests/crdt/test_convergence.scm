;;; test_convergence.scm — Two-buffer CRDT convergence test
;;;
;;; Tests that two buffers with separate client IDs can exchange sync
;;; updates and converge to the same content.

(define *test-state-a* #f)
(define *test-updates-b* (list))

(define (apply-updates-to buf lst)
  (if (null? lst) #t
      (begin
        (buffer-apply-update buf (car lst))
        (apply-updates-to buf (cdr lst)))))

(describe-group "Two-buffer CRDT convergence"
  (lambda ()
    (it-test "setup A and B with shared state"
      (lambda ()
        (create-buffer "*crdt-a*")
        (buffer-enable-sync 1)
        (buffer-insert "hello from A")
        (should-equal (buffer-string) "hello from A")
        (set! *test-state-a* (buffer-encode-state))
        (should *test-state-a*)
        (create-buffer "*crdt-b*")
        (buffer-load-sync-state *test-state-a* 2)
        (should-equal (buffer-string) "hello from A")))

    (it-test "B edits and exchanges updates with A"
      (lambda ()
        (goto-char 12)
        (buffer-insert " and B")
        (should-equal (buffer-string) "hello from A and B")
        (set! *test-updates-b* (buffer-drain-updates))
        (should (> (length *test-updates-b*) 0))
        ;; Apply B's updates to A
        (switch-to-buffer (get-buffer-by-name "*crdt-a*"))
        (apply-updates-to "*crdt-a*" *test-updates-b*)
        (should-contain (buffer-string) "and B")))))
