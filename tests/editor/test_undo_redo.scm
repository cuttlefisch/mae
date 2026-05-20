;;; test_undo_redo.scm — Undo/redo basic operations

(describe-group "Undo/Redo"
  (lambda ()
    (it-test "setup clean buffer"
      (lambda ()
        (create-buffer "*test-undo*")))

    (it-test "insert text"
      (lambda ()
        (buffer-insert "hello")))

    (it-test "verify insert"
      (lambda ()
        (should-equal (buffer-string) "hello")))

    (it-test "undo reverts insert"
      (lambda ()
        (buffer-undo)))

    (it-test "buffer is empty after undo"
      (lambda ()
        (should-equal (buffer-string) "")))

    (it-test "redo restores text"
      (lambda ()
        (buffer-redo)))

    (it-test "buffer has text after redo"
      (lambda ()
        (should-equal (buffer-string) "hello")))))
