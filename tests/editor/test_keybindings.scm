;;; test_keybindings.scm — Command existence and mode-switching via commands
;;;
;;; Verifies that core commands are registered, that run-command transitions
;;; modes correctly, and that unknown commands are not falsely reported present.

(describe-group "Keybindings and commands"
  (lambda ()
    (it-test "setup fresh buffer"
      (lambda ()
        (create-buffer "*test-keybindings*")))

    (it-test "command 'save' exists"
      (lambda ()
        (should (command-exists? "save"))))

    (it-test "command 'enter-insert-mode' exists"
      (lambda ()
        (should (command-exists? "enter-insert-mode"))))

    (it-test "command 'enter-normal-mode' exists"
      (lambda ()
        (should (command-exists? "enter-normal-mode"))))

    (it-test "command 'enter-visual-char' exists"
      (lambda ()
        (should (command-exists? "enter-visual-char"))))

    (it-test "command 'next-buffer' exists"
      (lambda ()
        (should (command-exists? "next-buffer"))))

    (it-test "nonexistent command returns false"
      (lambda ()
        (should-not (command-exists? "nonexistent-cmd-xyz"))))

    (it-test "start in normal mode"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is in normal mode"
      (lambda ()
        (should-mode "normal")))

    (it-test "enter insert mode via run-command"
      (lambda ()
        (run-command "enter-insert-mode")))

    (it-test "is in insert mode"
      (lambda ()
        (should-mode "insert")))

    (it-test "return to normal mode via run-command"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is in normal mode again"
      (lambda ()
        (should-mode "normal")))))
