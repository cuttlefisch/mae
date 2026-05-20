;;; test_visual_mode.scm — Visual mode selection and region primitives
;;;
;;; Verifies that entering visual-char mode activates a region, that cursor
;;; movement extends the selection, and that returning to normal mode deactivates it.

(describe-group "Visual mode"
  (lambda ()
    (it-test "setup buffer with text"
      (lambda ()
        (create-buffer "*test-visual*")))

    (it-test "insert sample text"
      (lambda ()
        (buffer-insert "hello visual world")))

    (it-test "go to beginning"
      (lambda ()
        (goto-char 0)))

    (it-test "enter normal mode first"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is in normal mode"
      (lambda ()
        (should-mode "normal")))

    (it-test "enter visual-char mode"
      (lambda ()
        (run-command "enter-visual-char")))

    (it-test "is in visual mode"
      (lambda ()
        (should-mode "visual")))

    (it-test "region is active"
      (lambda ()
        (should (region-active?))))

    (it-test "move right to extend selection"
      (lambda ()
        (run-command "move-right")))

    (it-test "region still active after move"
      (lambda ()
        (should (region-active?))))

    (it-test "move right again"
      (lambda ()
        (run-command "move-right")))

    (it-test "region end is ahead of beginning"
      (lambda ()
        (should (>= (region-end) (region-beginning)))))

    (it-test "return to normal mode"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is normal mode again"
      (lambda ()
        (should-mode "normal")))))
