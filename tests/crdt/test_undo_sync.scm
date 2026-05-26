;;; test_undo_sync.scm — Undo with sync enabled
;;;
;;; Tests that undo works correctly when CRDT sync is active,
;;; and that the sync doc stays in agreement with the rope after undo.

(describe-group "Undo with sync enabled"
  (lambda ()
    (it-test "insert, undo boundary, insert, undo — sync doc matches"
      (lambda ()
        (create-buffer "*test-undo-sync*")
        (buffer-enable-sync 1)
        (buffer-insert "line 1\n")
        (should-contain (buffer-string) "line 1")
        (buffer-undo-boundary)
        (buffer-insert "line 2\n")
        (should-contain (buffer-string) "line 1")
        (should-contain (buffer-string) "line 2")
        (buffer-undo)
        (should-contain (buffer-string) "line 1")
        (should-not (string-contains? (buffer-string) "line 2"))
        (should-equal (buffer-sync-content) (buffer-string))))))
