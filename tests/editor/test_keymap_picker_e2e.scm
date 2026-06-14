;;; E2E: the guided keybindings picker (dashboard quick-action).
;;;
;;; `choose-keymap-flavor` opens a command-palette listing the flavors with
;;; descriptions; selecting one live-switches via keymap-set-flavor. Validates
;;; the onboarding flow end-to-end through the real palette + key pipeline.

(describe-group "keybindings picker"
  (lambda ()
    (it-test "baseline doom (Normal)"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "doom is normal mode"
      (lambda () (should-mode "normal")))
    (it-test "open the guided picker"
      (lambda () (run-command "choose-keymap-flavor")))
    (it-test "picker opened the command palette"
      (lambda () (should-mode "command-palette")))
    (it-test "filter to nonmodal and select it"
      (lambda () (feed-keys "n o n Enter")))
    (it-test "picker live-switched to nonmodal (Insert default)"
      (lambda () (should-mode "insert")))
    (it-test "and the non-modal C-; keypad works after the picker"
      (lambda () (feed-keys "C-;")))
    (it-test "keypad open"
      (lambda () (should (which-key-open?))))
    (it-test "cancel"
      (lambda () (feed-keys "escape")))
    (it-test "restore doom flavor for remaining tests"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "back to normal mode"
      (lambda () (should-mode "normal")))))
