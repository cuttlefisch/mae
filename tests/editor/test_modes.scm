;;; test_modes.scm — Mode transition tests

(describe-group "Mode transitions"
  (lambda ()
    (it-test "normal → insert → normal cycle"
      (lambda ()
        (create-buffer "*test-modes*")
        (should-mode "normal")
        (run-command "enter-insert-mode")
        (should-mode "insert")
        (run-command "enter-normal-mode")
        (should-mode "normal")))))
