;;; test_undo_sync.scm — Undo/redo with sync enabled (no server needed)
;;;
;;; Validates that undo on a synced buffer properly reverts both the rope
;;; and the CRDT document, keeping them in sync.

(describe-group "Undo with sync"
  (lambda ()
    (it-test "insert, boundary, insert, undo — sync stays correct"
      (lambda ()
        (create-buffer "*undo-sync-test*")
        (buffer-enable-sync 1)
        (buffer-insert "first")
        (should-equal (buffer-string) "first")
        (buffer-undo-boundary)
        (buffer-insert " second")
        (should-equal (buffer-string) "first second")
        (run-command "undo")
        (should-equal (buffer-string) "first")
        (should-equal (buffer-sync-content) (buffer-string))))))
