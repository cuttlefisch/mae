;;; test_sync_insert.scm — Insert generates CRDT updates (no server needed)
;;;
;;; Validates that buffer mutations on a synced buffer keep the CRDT doc
;;; in sync with the rope.

(describe-group "Sync insert generates updates"
  (lambda ()
    (it-test "inserts generate updates and keep sync content correct"
      (lambda ()
        (create-buffer "*sync-insert-test*")
        (buffer-enable-sync 1)
        (buffer-insert "hello world")
        (let ((updates (buffer-drain-updates)))
          (should (> (length updates) 0)))
        (should-equal (buffer-sync-content) "hello world")
        (buffer-insert " more")
        (should-equal (buffer-sync-content) "hello world more")
        (should-equal (buffer-text "*sync-insert-test*")
                      (buffer-sync-content))))))
