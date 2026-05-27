;;; test_hooks_firing.scm — Verify hook registration, removal, and command dispatch
;;;
;;; Note: In the headless test runner, pending_hook_evals are queued by
;;; fire_hook but never flushed (flush_pending_hooks runs in the event loop,
;;; not the test runner). So we test registration/removal mechanics and
;;; verify that the hook-triggering commands exist and execute without error.

(describe-group "Hook system"
  (lambda ()
    (it-test "setup buffer"
      (lambda ()
        (create-buffer "*hook-test*")))

    ;; Registration
    (it-test "add-hook! succeeds"
      (lambda ()
        (define (my-test-hook) #t)
        (add-hook! "mode-change" "my-test-hook")
        (should #t)))

    (it-test "remove-hook! succeeds"
      (lambda ()
        (remove-hook! "mode-change" "my-test-hook")
        (should #t)))

    ;; Mode change commands execute without error
    (it-test "enter-insert-mode command exists"
      (lambda ()
        (should (command-exists? "enter-insert-mode"))))

    (it-test "enter-normal-mode command exists"
      (lambda ()
        (should (command-exists? "enter-normal-mode"))))

    (it-test "enter insert mode executes"
      (lambda ()
        (run-command "enter-insert-mode")))

    (it-test "mode is insert"
      (lambda ()
        (should-equal *mode* "insert")))

    (it-test "return to normal mode"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "mode is normal"
      (lambda ()
        (should-equal *mode* "normal")))))
