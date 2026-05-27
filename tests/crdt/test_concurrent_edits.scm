;;; test_concurrent_edits.scm — Concurrent insert convergence test
;;;
;;; Two buffers with the same initial state each insert at position 0
;;; concurrently. They exchange updates and must converge to identical
;;; content (CRDT interleaving order determined by client-id in YATA).

(define *concurrent-state-a* #f)
(define *concurrent-updates-a* (list))
(define *concurrent-updates-b* (list))

(define (apply-updates-to buf lst)
  (if (null? lst) #t
      (begin
        (buffer-apply-update buf (car lst))
        (apply-updates-to buf (cdr lst)))))

(describe-group "Concurrent inserts converge"
  (lambda ()
    (it-test "setup A and B with shared base"
      (lambda ()
        (create-buffer "*concurrent-a*")
        (buffer-enable-sync 1)
        (buffer-insert "base")
        (should-equal (buffer-string) "base")
        (set! *concurrent-state-a* (buffer-encode-state))
        (should *concurrent-state-a*)
        (create-buffer "*concurrent-b*")
        (buffer-load-sync-state *concurrent-state-a* 2)
        (should-equal (buffer-string) "base")))

    (it-test "concurrent inserts at position 0"
      (lambda ()
        ;; B inserts
        (goto-char 0)
        (buffer-insert "B:")
        (set! *concurrent-updates-b* (buffer-drain-updates))
        (should (> (length *concurrent-updates-b*) 0))
        ;; Switch to A and insert
        (switch-to-buffer (get-buffer-by-name "*concurrent-a*"))
        (goto-char 0)
        (buffer-insert "A:")
        (set! *concurrent-updates-a* (buffer-drain-updates))
        (should (> (length *concurrent-updates-a*) 0))))

    (it-test "exchange updates and verify convergence"
      (lambda ()
        ;; Apply B's updates to A
        (apply-updates-to "*concurrent-a*" *concurrent-updates-b*)
        ;; Apply A's updates to B
        (switch-to-buffer (get-buffer-by-name "*concurrent-b*"))
        (apply-updates-to "*concurrent-b*" *concurrent-updates-a*)
        ;; Verify convergence
        (should-equal (buffer-text "*concurrent-a*")
                      (buffer-text "*concurrent-b*"))
        (should-contain (buffer-text "*concurrent-a*") "A:")
        (should-contain (buffer-text "*concurrent-a*") "B:")
        (should-contain (buffer-text "*concurrent-a*") "base")))))
