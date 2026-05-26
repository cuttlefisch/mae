;;; test_remote_cursor.scm — Remote edit applies correctly to buffer
;;;
;;; When a remote peer inserts text, the local buffer content should
;;; be updated correctly.

(define *remote-state* #f)
(define *remote-updates* (list))

(define (apply-updates-to buf lst)
  (if (null? lst) #t
      (begin
        (buffer-apply-update buf (car lst))
        (apply-updates-to buf (cdr lst)))))

(describe-group "Remote edit content correctness"
  (lambda ()
    (it-test "setup A and B with shared state"
      (lambda ()
        (create-buffer "*remote-a*")
        (buffer-enable-sync 1)
        (buffer-insert "hello world")
        (should-equal (buffer-string) "hello world")
        (set! *remote-state* (buffer-encode-state))
        (should *remote-state*)
        (create-buffer "*remote-b*")
        (buffer-load-sync-state *remote-state* 2)
        (should-equal (buffer-string) "hello world")))

    (it-test "B edits and A receives update"
      (lambda ()
        (goto-char 5)
        (buffer-insert ",")
        (should-equal (buffer-string) "hello, world")
        (set! *remote-updates* (buffer-drain-updates))
        (should (> (length *remote-updates*) 0))
        ;; Apply to A
        (switch-to-buffer (get-buffer-by-name "*remote-a*"))
        (apply-updates-to "*remote-a*" *remote-updates*)
        (should-equal (buffer-string) "hello, world")
        (should-equal (buffer-sync-content) (buffer-string))))))
