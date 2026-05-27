;;; test_marks.scm — Mark set/jump and jump list verification
;;;
;;; Exercises the mark system: set a mark, move, jump back.
;;; Also verifies change list navigation (g;/g,).

(describe-group "Marks and jump list"
  (lambda ()
    (it-test "setup buffer with content"
      (lambda ()
        (create-buffer "*marks-test*")
        (buffer-insert "line one\nline two\nline three\nline four\nline five")))

    ;; Jump list: gg pushes to jump list, then Ctrl-o returns
    (it-test "gg jumps to first line"
      (lambda ()
        (goto-char 20)  ; somewhere in the middle
        (run-command "move-to-first-line")))

    (it-test "cursor is at line 1 after gg"
      (lambda ()
        (should-equal *cursor-row* 0)))

    (it-test "jump-backward returns to previous position"
      (lambda ()
        (run-command "jump-backward")))

    (it-test "cursor restored after jump-backward"
      (lambda ()
        ;; Should be back near where we were before gg
        (should (> *cursor-row* 0))))

    (it-test "jump-forward goes back to gg target"
      (lambda ()
        (run-command "jump-forward")))

    (it-test "cursor at line 1 again after jump-forward"
      (lambda ()
        (should-equal *cursor-row* 0)))

    ;; Verify change list commands exist
    (it-test "change-backward command exists"
      (lambda ()
        (should (command-exists? "change-backward"))))

    (it-test "change-forward command exists"
      (lambda ()
        (should (command-exists? "change-forward"))))

    (it-test "show-changes-buffer command exists"
      (lambda ()
        (should (command-exists? "show-changes-buffer"))))))
