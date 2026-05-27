;;; test_undo_redo.scm — Undo/redo basic operations

(describe-group "Undo/Redo"
  (lambda ()
    (it-test "insert, undo, redo cycle"
      (lambda ()
        (create-buffer "*test-undo*")
        (buffer-insert "hello")
        (should-equal (buffer-string) "hello")
        (buffer-undo)
        (should-equal (buffer-string) "")
        (buffer-redo)
        (should-equal (buffer-string) "hello")))))
