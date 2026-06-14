;;; E2E: keybind flavors (doom modal + nonmodal) via REAL key injection.
;;;
;;; `(feed-keys "...")` drives raw key events through the actual handle_key
;;; pipeline against the real loaded flavor keymaps — exercising the transient
;;; leader keypad, the non-modal Insert-default flavor, CUA chords, and LIVE
;;; flavor switching (reset + reload). This is true end-to-end: real editor,
;;; real event-loop key routing, real which-key + dispatch. Each it-test is one
;;; eval→apply cycle (feed in one step, assert in the next).

(describe-group "keymap flavor: doom (modal) — SPC leader keypad"
  (lambda ()
    (it-test "select doom flavor"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "doom boots in normal mode"
      (lambda () (should-mode "normal")))
    (it-test "editable buffer (off the dashboard)"
      (lambda () (create-buffer "*doom-e2e*")))
    (it-test "baseline line_numbers off"
      (lambda () (set-option! "line_numbers" "false")))
    (it-test "SPC opens the leader keypad"
      (lambda () (feed-keys "SPC")))
    (it-test "which-key popup is open with entries"
      (lambda ()
        (should (which-key-open?))
        (should (> (which-key-entry-count) 0))))
    (it-test "t l resolves toggle-line-numbers from the keypad"
      (lambda () (feed-keys "t l")))
    (it-test "command ran, keypad closed, back to normal"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "normal")
        (should-equal (get-option "line_numbers") "true")))
    (it-test "SPC opens keypad again"
      (lambda () (feed-keys "SPC")))
    (it-test "open before cancel"
      (lambda () (should (which-key-open?))))
    (it-test "escape cancels the keypad"
      (lambda () (feed-keys "escape")))
    (it-test "keypad closed and NO command ran (line_numbers unchanged)"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "normal")
        (should-equal (get-option "line_numbers") "true")))))

(describe-group "keymap flavor: nonmodal (CUA) — Insert default + C-; keypad"
  (lambda ()
    (it-test "switch to nonmodal flavor (live)"
      (lambda () (execute-ex "keymap-set-flavor nonmodal")))
    (it-test "nonmodal boots in insert mode"
      (lambda () (should-mode "insert")))
    (it-test "fresh buffer"
      (lambda () (create-buffer "*nm-e2e*")))
    (it-test "ensure insert (create-buffer may reset mode)"
      (lambda () (run-command "enter-insert-mode")))
    (it-test "typing inserts plain text (non-modal)"
      (lambda () (feed-keys "h i")))
    (it-test "buffer contains the typed text"
      (lambda () (should-equal (buffer-string) "hi")))
    (it-test "C-; opens the keypad from insert"
      (lambda () (feed-keys "C-;")))
    (it-test "keypad open (same mae which-key menu)"
      (lambda ()
        (should (which-key-open?))
        (should (> (which-key-entry-count) 0))))
    (it-test "t w toggles via the keypad"
      (lambda () (feed-keys "t w")))
    (it-test "one command then BACK TO INSERT (base mode preserved)"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "insert")))
    (it-test "C-s (CUA save chord) dispatches"
      (lambda () (feed-keys "C-s")))
    (it-test "still insert after the CUA chord"
      (lambda () (should-mode "insert")))
    (it-test "C-; then escape cancels"
      (lambda () (feed-keys "C-; escape")))
    (it-test "keypad closed, still insert, nothing stray"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "insert")))
    (it-test "restore doom flavor for remaining tests"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "back to normal mode after restore"
      (lambda () (should-mode "normal")))))

(describe-group "leader: N-level traversal is flavor-independent, restoration is flavor-specific"
  (lambda ()
    ;; The leader tree can be arbitrarily deep (SPC n d t = 3 levels under the
    ;; leader). Once the keypad is active, traversal goes through the SAME shared
    ;; `leader` keymap regardless of how it was entered (SPC vs C-;), so the menu
    ;; + navigation are identical across flavors. Only the post-traversal
    ;; restoration differs (Normal for doom, Insert for nonmodal) because the
    ;; base mode is preserved untouched.

    ;; doom: traverse 3 levels deep, cancel, restore to NORMAL.
    (it-test "doom flavor"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "editable buffer"
      (lambda () (create-buffer "*deep-doom*")))
    (it-test "traverse 3 levels: SPC n d (dailies submenu)"
      (lambda () (feed-keys "SPC n d")))
    (it-test "mid-traversal: keypad open with submenu entries"
      (lambda ()
        (should (which-key-open?))
        (should (> (which-key-entry-count) 0))))
    (it-test "escape cancels the deep traversal"
      (lambda () (feed-keys "escape")))
    (it-test "restored to NORMAL (doom restoration)"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "normal")))

    ;; nonmodal: the SAME traversal reaches the SAME submenu; restore to INSERT.
    (it-test "nonmodal flavor"
      (lambda () (execute-ex "keymap-set-flavor nonmodal")))
    (it-test "editable buffer"
      (lambda () (create-buffer "*deep-nm*")))
    (it-test "ensure insert"
      (lambda () (run-command "enter-insert-mode")))
    (it-test "same traversal via C-; n d"
      (lambda () (feed-keys "C-; n d")))
    (it-test "identical mid-traversal state (same shared leader keymap)"
      (lambda ()
        (should (which-key-open?))
        (should (> (which-key-entry-count) 0))))
    (it-test "escape cancels"
      (lambda () (feed-keys "escape")))
    (it-test "restored to INSERT (nonmodal restoration differs)"
      (lambda ()
        (should (not (which-key-open?)))
        (should-mode "insert")))))

(describe-group "keymap flavor: live switch back to doom (no stale bindings)"
  (lambda ()
    (it-test "switch back to doom"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "doom normal mode restored"
      (lambda () (should-mode "normal")))
    (it-test "SPC leader works again after the round-trip"
      (lambda () (feed-keys "SPC")))
    (it-test "keypad open under doom again"
      (lambda () (should (which-key-open?))))
    (it-test "cancel"
      (lambda () (feed-keys "escape")))
    (it-test "closed"
      (lambda () (should (not (which-key-open?)))))))
