;;; test_keybindings.scm — Command existence and mode-switching via commands
;;;
;;; Verifies that core commands are registered, that run-command transitions
;;; modes correctly, and that unknown commands are not falsely reported present.

(describe-group "Keybindings and commands"
  (lambda ()
    (it-test "core commands exist"
      (lambda ()
        (create-buffer "*test-keybindings*")
        (should (command-exists? "save"))
        (should (command-exists? "enter-insert-mode"))
        (should (command-exists? "enter-normal-mode"))
        (should (command-exists? "enter-visual-char"))
        (should (command-exists? "next-buffer"))
        (should-not (command-exists? "nonexistent-cmd-xyz"))))

    (it-test "mode transitions via run-command"
      (lambda ()
        (run-command "enter-normal-mode")
        (should-mode "normal")
        (run-command "enter-insert-mode")
        (should-mode "insert")
        (run-command "enter-normal-mode")
        (should-mode "normal")))))
