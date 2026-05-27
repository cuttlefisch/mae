;;; test_window_management.scm — Window split, focus, close operations
;;;
;;; Exercises window management through the Scheme API:
;;; split, focus movement, close.
;;; Uses relative window count comparisons since prior tests may leave windows.

(define *base-window-count* 0)

(describe-group "Window management"
  (lambda ()
    (it-test "setup: create initial buffer and capture baseline"
      (lambda ()
        (create-buffer "*win-a*")
        (buffer-insert "buffer A content")
        (set! *base-window-count* *window-count*)))

    (it-test "vertical split creates a window"
      (lambda ()
        (run-command "split-vertical")))

    (it-test "window count increased by 1"
      (lambda ()
        (should-equal *window-count* (+ *base-window-count* 1))))

    (it-test "close window returns to baseline"
      (lambda ()
        (run-command "close-window")))

    (it-test "window count back to baseline"
      (lambda ()
        (should-equal *window-count* *base-window-count*)))

    (it-test "horizontal split also works"
      (lambda ()
        (run-command "split-horizontal")))

    (it-test "window count increased by 1 after hsplit"
      (lambda ()
        (should-equal *window-count* (+ *base-window-count* 1))))

    (it-test "close returns to baseline again"
      (lambda ()
        (run-command "close-window")))

    (it-test "final: back to baseline"
      (lambda ()
        (should-equal *window-count* *base-window-count*)))

    ;; Verify focus commands exist
    (it-test "focus commands exist"
      (lambda ()
        (should (command-exists? "focus-left"))
        (should (command-exists? "focus-right"))
        (should (command-exists? "focus-up"))
        (should (command-exists? "focus-down"))
        (should (command-exists? "focus-next-window"))))

    ;; Verify window manipulation commands exist
    (it-test "window commands exist"
      (lambda ()
        (should (command-exists? "window-maximize"))
        (should (command-exists? "window-balance"))
        (should (command-exists? "window-grow"))
        (should (command-exists? "window-shrink"))))))
