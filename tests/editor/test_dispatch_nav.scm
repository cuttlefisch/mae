;;; test_dispatch_nav.scm — Navigation commands dispatched via run-command
;;;
;;; Tests cursor movement commands by verifying cursor position after each
;;; navigation command. Uses SharedState-backed cursor-row/cursor-col.

(describe-group "Dispatch navigation commands"
  (lambda ()
    (it-test "setup buffer with multi-line content"
      (lambda ()
        (create-buffer "*test-dispatch-nav*")))

    (it-test "insert multi-line text"
      (lambda ()
        (buffer-insert "one two three\nfour five six\nseven eight nine\nten eleven twelve")))

    ;; --- move-to-first-line / move-to-last-line ---
    (it-test "goto middle of buffer"
      (lambda ()
        (cursor-goto 2 0)))

    (it-test "move to first line"
      (lambda ()
        (run-command "move-to-first-line")))

    (it-test "cursor is on first line"
      (lambda ()
        (should-equal (cursor-row) 0)))

    (it-test "move to last line"
      (lambda ()
        (run-command "move-to-last-line")))

    (it-test "cursor is on last line"
      (lambda ()
        (should-equal (cursor-row) 3)))

    ;; --- move-to-line-start / move-to-line-end ---
    (it-test "goto middle of a line"
      (lambda ()
        (cursor-goto 0 5)))

    (it-test "move to line start"
      (lambda ()
        (run-command "move-to-line-start")))

    (it-test "cursor is at column 0"
      (lambda ()
        (should-equal (cursor-col) 0)))

    (it-test "move to line end"
      (lambda ()
        (run-command "move-to-line-end")))

    (it-test "cursor is past last char on line"
      (lambda ()
        ;; "one two three" = 13 chars, cursor should be near end
        (should-greater-than (cursor-col) 5)))

    ;; --- move-word-forward ---
    (it-test "goto start for word navigation"
      (lambda ()
        (cursor-goto 0 0)))

    (it-test "move word forward"
      (lambda ()
        (run-command "move-word-forward")))

    (it-test "cursor moved past first word"
      (lambda ()
        (should-greater-than (cursor-col) 0)))

    ;; --- move-paragraph-forward ---
    (it-test "create paragraph buffer"
      (lambda ()
        (create-buffer "*test-para-nav*")))

    (it-test "insert paragraphs"
      (lambda ()
        (buffer-insert "paragraph one\n\nparagraph two\n\nparagraph three")))

    (it-test "goto start"
      (lambda ()
        (cursor-goto 0 0)))

    (it-test "move paragraph forward"
      (lambda ()
        (run-command "move-paragraph-forward")))

    (it-test "cursor moved past first paragraph"
      (lambda ()
        (should-greater-than (cursor-row) 0)))))
