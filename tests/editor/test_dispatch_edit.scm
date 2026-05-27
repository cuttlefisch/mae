;;; test_dispatch_edit.scm — Edit commands dispatched via run-command
;;;
;;; Tests edit commands that modify buffer content, verifying results
;;; via buffer-string and cursor position checks.

(describe-group "Dispatch edit commands"
  (lambda ()
    (it-test "delete-char-forward and backward"
      (lambda ()
        (create-buffer "*test-dispatch-edit*")
        (buffer-insert "hello world\nsecond line\nthird line")
        (should-contain (buffer-string) "hello world")
        (goto-char 0)
        (run-command "delete-char-forward")
        (should-equal (substring (buffer-string) 0 4) "ello")
        (goto-char 4)
        (run-command "delete-char-backward")
        (should-less-than (string-length (buffer-string)) 33)))

    (it-test "delete-line command"
      (lambda ()
        (create-buffer "*test-del-line*")
        (buffer-insert "line one\nline two\nline three")
        (goto-char 0)
        (run-command "delete-line")
        (should-contain (buffer-string) "line two")))

    (it-test "uppercase and lowercase line"
      (lambda ()
        (create-buffer "*test-case*")
        (buffer-insert "hello world")
        (goto-char 0)
        (run-command "uppercase-line")
        (should-equal (buffer-string) "HELLO WORLD")
        (goto-char 0)
        (run-command "lowercase-line")
        (should-equal (buffer-string) "hello world")))))
