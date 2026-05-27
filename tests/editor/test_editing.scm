;;; test_editing.scm — Basic buffer editing operations
;;;
;;; Tests insert, delete, and replace as continuous editing sessions.

(describe-group "Basic editing"
  (lambda ()
    (it-test "insert at cursor and at beginning"
      (lambda ()
        (create-buffer "*test-editing*")
        (buffer-insert "world")
        (should-equal (buffer-string) "world")
        (goto-char 0)
        (buffer-insert "hello ")
        (should-equal (buffer-string) "hello world")))

    (it-test "delete and replace ranges"
      (lambda ()
        (buffer-delete-range 5 6)
        (should-equal (buffer-string) "helloworld")
        (buffer-replace-range 5 10 " universe")
        (should-equal (buffer-string) "hello universe")))))
