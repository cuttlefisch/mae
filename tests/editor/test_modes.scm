;;; test_modes.scm — Mode transition tests

(describe-group "Mode transitions"
  (lambda ()
    (it-test "setup fresh buffer"
      (lambda ()
        (create-buffer "*test-modes*")))

    (it-test "starts in normal mode"
      (lambda ()
        (should-mode "normal")))

    (it-test "enter insert mode"
      (lambda ()
        (run-command "enter-insert-mode")))

    (it-test "is in insert mode"
      (lambda ()
        (should-mode "insert")))

    (it-test "back to normal"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is normal again"
      (lambda ()
        (should-mode "normal")))))
