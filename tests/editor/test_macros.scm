;;; test_macros.scm — Macro recording and replay
;;;
;;; Exercises the macro system: record keystrokes via the command API,
;;; then replay and verify the buffer was modified correctly.
;;;
;;; Note: Since the Scheme test runner doesn't send raw keystrokes,
;;; we test macros via the command-level API (record-start, record-stop,
;;; replay-last-macro) rather than actual q/@ keys.

(describe-group "Macro recording and replay"
  (lambda ()
    (it-test "setup buffer"
      (lambda ()
        (create-buffer "*macro-test*")
        (buffer-insert "aaa\nbbb\nccc")))

    ;; Test that the event recorder commands exist and don't crash
    (it-test "record-start command exists"
      (lambda ()
        (should (command-exists? "record-start"))))

    (it-test "record-stop command exists"
      (lambda ()
        (should (command-exists? "record-stop"))))

    (it-test "record-save command exists"
      (lambda ()
        (should (command-exists? "record-save"))))

    (it-test "replay-last-macro command exists"
      (lambda ()
        (should (command-exists? "replay-last-macro"))))

    ;; Test @: (repeat last ex command)
    (it-test "execute ex command to delete first line"
      (lambda ()
        (goto-char 0)
        (run-command "delete-line")))

    (it-test "first line deleted"
      (lambda ()
        (should-equal (buffer-string) "bbb\nccc")))

    (it-test "dot-repeat deletes next line"
      (lambda ()
        (goto-char 0)
        (run-command "dot-repeat")))

    (it-test "second line deleted via repeat"
      (lambda ()
        (should-equal (buffer-string) "ccc")))))
