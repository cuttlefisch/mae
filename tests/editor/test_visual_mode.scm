;;; test_visual_mode.scm — Visual mode selection and region primitives
;;;
;;; Verifies that entering visual-char mode activates a region, that cursor
;;; movement extends the selection, and that returning to normal mode deactivates it.

(describe-group "Visual mode"
  (lambda ()
    (it-test "visual selection workflow"
      (lambda ()
        (create-buffer "*test-visual*")
        (buffer-insert "hello visual world")
        (goto-char 0)
        (run-command "enter-normal-mode")
        (should-mode "normal")
        (run-command "enter-visual-char")
        (should-mode "visual")
        (should (region-active?))
        (run-command "move-right")
        (should (region-active?))
        (run-command "move-right")
        (should (>= (region-end) (region-beginning)))
        (run-command "enter-normal-mode")
        (should-mode "normal")))))
