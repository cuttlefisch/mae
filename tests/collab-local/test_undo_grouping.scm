;;; test_undo_grouping.scm — CRDT undo must respect undo groups
;;;
;;; When sync is enabled, multiple sequential inserts without an explicit
;;; undo boundary should merge into one undo item.  This mirrors vim's
;;; insert-mode behavior where typing "hello" then pressing Esc undoes
;;; all five characters at once.

(describe-group "CRDT undo grouping"
  (lambda ()
    (it-test "sequential inserts merge into one undo group"
      (lambda ()
        (create-buffer "*undo-group-test*")
        (buffer-enable-sync 1)
        ;; Simulate typing "hello" character by character
        (buffer-insert "h")
        (buffer-insert "e")
        (buffer-insert "l")
        (buffer-insert "l")
        (buffer-insert "o")
        (should-equal (buffer-string) "hello")
        ;; Single undo reverts ALL five inserts (same undo group)
        (buffer-undo)
        (should-equal (buffer-string) "")
        (should-equal (buffer-sync-content) "")))))
