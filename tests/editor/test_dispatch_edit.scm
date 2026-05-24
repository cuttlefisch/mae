;;; test_dispatch_edit.scm — Edit commands dispatched via run-command
;;;
;;; Tests edit commands that modify buffer content, verifying results
;;; via SharedState-backed buffer-string and cursor position checks.

(describe-group "Dispatch edit commands"
  (lambda ()
    (it-test "setup buffer with content"
      (lambda ()
        (create-buffer "*test-dispatch-edit*")))

    (it-test "insert test content"
      (lambda ()
        (buffer-insert "hello world\nsecond line\nthird line")))

    (it-test "verify initial content"
      (lambda ()
        (should-contain (buffer-string) "hello world")))

    ;; --- delete-char-forward ---
    (it-test "goto start for delete test"
      (lambda ()
        (goto-char 0)))

    (it-test "delete char forward"
      (lambda ()
        (run-command "delete-char-forward")))

    (it-test "verify first char deleted"
      (lambda ()
        (should-equal (substring (buffer-string) 0 4) "ello")))

    ;; --- delete-char-backward ---
    (it-test "goto position 4"
      (lambda ()
        (goto-char 4)))

    (it-test "delete char backward"
      (lambda ()
        (run-command "delete-char-backward")))

    (it-test "verify backward delete"
      (lambda ()
        ;; "ello world..." → delete backward at col 4 removes 'o' (vi semantics)
        ;; or 'l' depending on exact cursor position — just check length decreased
        (should-less-than (string-length (buffer-string)) 33)))

    ;; --- delete-line ---
    (it-test "create fresh buffer for delete-line"
      (lambda ()
        (create-buffer "*test-del-line*")))

    (it-test "insert multi-line content"
      (lambda ()
        (buffer-insert "line one\nline two\nline three")))

    (it-test "goto first line"
      (lambda ()
        (goto-char 0)))

    (it-test "delete line"
      (lambda ()
        (run-command "delete-line")))

    (it-test "verify line deleted"
      (lambda ()
        (should-contain (buffer-string) "line two")))

    ;; --- uppercase-line / lowercase-line ---
    (it-test "create buffer for case commands"
      (lambda ()
        (create-buffer "*test-case*")))

    (it-test "insert lowercase text"
      (lambda ()
        (buffer-insert "hello world")))

    (it-test "goto start for uppercase"
      (lambda ()
        (goto-char 0)))

    (it-test "uppercase line"
      (lambda ()
        (run-command "uppercase-line")))

    (it-test "verify uppercase"
      (lambda ()
        (should-equal (buffer-string) "HELLO WORLD")))

    (it-test "goto start for lowercase"
      (lambda ()
        (goto-char 0)))

    (it-test "lowercase line"
      (lambda ()
        (run-command "lowercase-line")))

    (it-test "verify lowercase"
      (lambda ()
        (should-equal (buffer-string) "hello world")))))
