;;; test_dispatch_nav.scm — Navigation commands dispatched via run-command
;;;
;;; Tests cursor movement commands by verifying cursor position after each
;;; navigation command.

(describe-group "Dispatch navigation commands"
  (lambda ()
    (it-test "first-line / last-line navigation"
      (lambda ()
        (create-buffer "*test-dispatch-nav*")
        (buffer-insert "one two three\nfour five six\nseven eight nine\nten eleven twelve")
        (cursor-goto 2 0)
        (run-command "move-to-first-line")
        (should-equal (test-cursor-row) 0)
        (run-command "move-to-last-line")
        (should-equal (test-cursor-row) 3)))

    (it-test "line-start / line-end navigation"
      (lambda ()
        (cursor-goto 0 5)
        (run-command "move-to-line-start")
        (should-equal (test-cursor-col) 0)
        (run-command "move-to-line-end")
        (should-greater-than (test-cursor-col) 5)))

    (it-test "word-forward navigation"
      (lambda ()
        (cursor-goto 0 0)
        (run-command "move-word-forward")
        (should-greater-than (test-cursor-col) 0)))

    (it-test "paragraph-forward navigation"
      (lambda ()
        (create-buffer "*test-para-nav*")
        (buffer-insert "paragraph one\n\nparagraph two\n\nparagraph three")
        (cursor-goto 0 0)
        (run-command "move-paragraph-forward")
        (should-greater-than (test-cursor-row) 0)))))
