;;; test_remote_cursor.scm — Remote edit applies correctly to buffer
;;;
;;; When a remote peer inserts text, the local buffer content should
;;; be updated correctly.  Cursor adjustment for remote edits is a
;;; known limitation (tracked separately).

(define *remote-state* #f)
(define *remote-updates* (list))

(describe-group "Remote edit content correctness"
  (lambda ()
    ;; Setup: buffer A with "hello world"
    (it-test "setup buffer A"
      (lambda ()
        (create-buffer "*remote-a*")
        (buffer-enable-sync 1)))

    (it-test "insert content"
      (lambda ()
        (buffer-insert "hello world")))

    (it-test "verify A content"
      (lambda ()
        (should-equal (buffer-string) "hello world")))

    ;; Encode A's state, create B
    (it-test "encode A state"
      (lambda ()
        (set! *remote-state* (buffer-encode-state))
        (should *remote-state*)))

    (it-test "setup buffer B"
      (lambda ()
        (create-buffer "*remote-b*")
        (buffer-load-sync-state *remote-state* 2)))

    (it-test "B has A content"
      (lambda ()
        (should-equal (buffer-string) "hello world")))

    ;; B inserts at position 5 (between "hello" and " world")
    (it-test "B moves to pos 5"
      (lambda ()
        (goto-char 5)))

    (it-test "B inserts comma"
      (lambda ()
        (buffer-insert ",")))

    (it-test "B content correct"
      (lambda ()
        (should-equal (buffer-string) "hello, world")))

    ;; Get B's updates, apply to A
    (it-test "drain B updates"
      (lambda ()
        (set! *remote-updates* (buffer-drain-updates))
        (should (> (length *remote-updates*) 0))))

    (it-test "switch to A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*remote-a*"))))

    (it-test "apply B updates to A"
      (lambda ()
        (define (apply-all lst)
          (if (null? lst) #t
              (begin
                (buffer-apply-update "*remote-a*" (car lst))
                (apply-all (cdr lst)))))
        (apply-all *remote-updates*)))

    (it-test "A content matches B"
      (lambda ()
        (should-equal (buffer-string) "hello, world")))

    (it-test "sync content matches buffer"
      (lambda ()
        (should-equal (buffer-sync-content) (buffer-string))))))
