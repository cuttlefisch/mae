;;; test_collaborative_undo.scm — Collaborative undo convergence test
;;;
;;; A inserts "hello". B receives that state and inserts " world".
;;; A then undoes its own insert. Updates are exchanged so both peers
;;; see the full picture. Convergence is verified.

(define *undo-state-a* #f)
(define *undo-updates-b* (list))
(define *undo-updates-a-after-undo* (list))

(define (apply-updates-to buf lst)
  (if (null? lst) #t
      (begin
        (buffer-apply-update buf (car lst))
        (apply-updates-to buf (cdr lst)))))

(describe-group "Collaborative undo convergence"
  (lambda ()
    (it-test "setup A and B with shared state"
      (lambda ()
        (create-buffer "*undo-a*")
        (buffer-enable-sync 1)
        (buffer-insert "hello")
        (should-equal (buffer-string) "hello")
        (set! *undo-state-a* (buffer-encode-state))
        (should *undo-state-a*)
        (create-buffer "*undo-b*")
        (buffer-load-sync-state *undo-state-a* 2)
        (should-equal (buffer-string) "hello")))

    (it-test "B edits and A undoes"
      (lambda ()
        (goto-char 5)
        (buffer-insert " world")
        (should-equal (buffer-string) "hello world")
        (set! *undo-updates-b* (buffer-drain-updates))
        (should (> (length *undo-updates-b*) 0))
        ;; Switch to A and undo
        (switch-to-buffer (get-buffer-by-name "*undo-a*"))
        (buffer-undo)
        (should-equal (buffer-string) "")
        (set! *undo-updates-a-after-undo* (buffer-drain-updates))
        (should (list? *undo-updates-a-after-undo*))))

    (it-test "exchange updates and verify convergence"
      (lambda ()
        (apply-updates-to "*undo-a*" *undo-updates-b*)
        (apply-updates-to "*undo-b*" *undo-updates-a-after-undo*)
        (should-equal (buffer-text "*undo-a*")
                      (buffer-text "*undo-b*"))))))
