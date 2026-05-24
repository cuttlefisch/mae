;;; test_editing.scm — Basic buffer editing operations
;;;
;;; Each mutation step is a separate it-test because pending ops
;;; (buffer-insert, goto-char) are applied between test steps.

(describe-group "Basic editing"
  (lambda ()
    (it-test "setup clean buffer"
      (lambda ()
        (create-buffer "*test-editing*")))

    (it-test "insert at cursor"
      (lambda ()
        (buffer-insert "world")))

    (it-test "verify initial insert"
      (lambda ()
        (should-equal (buffer-string) "world")))

    (it-test "goto beginning"
      (lambda ()
        (goto-char 0)))

    (it-test "insert at beginning"
      (lambda ()
        (buffer-insert "hello ")))

    (it-test "content correct after prepend"
      (lambda ()
        (should-equal (buffer-string) "hello world")))

    (it-test "delete range"
      (lambda ()
        (buffer-delete-range 5 6)))

    (it-test "content after delete"
      (lambda ()
        (should-equal (buffer-string) "helloworld")))

    (it-test "replace range"
      (lambda ()
        (buffer-replace-range 5 10 " universe")))

    (it-test "content after replace"
      (lambda ()
        (should-equal (buffer-string) "hello universe")))))
