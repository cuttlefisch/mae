;;; test_sync_insert.scm — Insert generates CRDT updates (no server needed)
;;;
;;; Validates that buffer mutations on a synced buffer keep the CRDT doc
;;; in sync with the rope. Updates are drained between test steps by the
;;; test runner, so pending-updates is 0 at assertion time; instead we
;;; verify sync content correctness.

(describe-group "Sync insert generates updates"
  (lambda ()
    (it-test "setup: create synced buffer"
      (lambda ()
        (create-buffer "*sync-insert-test*")))
    (it-test "enable sync"
      (lambda ()
        (buffer-enable-sync 1)))
    (it-test "insert text"
      (lambda ()
        (buffer-insert "hello world")))
    (it-test "drain returns base64 updates"
      (lambda ()
        (let ((updates (buffer-drain-updates)))
          (should (> (length updates) 0)))))
    (it-test "sync content matches buffer"
      (lambda ()
        (should-equal (buffer-sync-content) "hello world")))
    (it-test "second insert appends"
      (lambda ()
        (buffer-insert " more")))
    (it-test "sync content matches after append"
      (lambda ()
        (should-equal (buffer-sync-content) "hello world more")))
    (it-test "buffer-text matches sync-content"
      (lambda ()
        (should-equal (buffer-text "*sync-insert-test*")
                      (buffer-sync-content))))))
