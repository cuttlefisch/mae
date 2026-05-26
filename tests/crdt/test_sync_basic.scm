;;; test_sync_basic.scm — Basic CRDT sync enable/insert/state tests
;;;
;;; Tests that enabling sync on a buffer works, that inserts generate
;;; sync updates, and that the yrs doc content matches the rope.

(describe-group "CRDT sync basics"
  (lambda ()
    (it-test "enable sync, insert, and verify"
      (lambda ()
        (create-buffer "*test-sync-basic*")
        (buffer-enable-sync 1)
        (should (buffer-sync-enabled?))
        (buffer-insert "hello")
        (should-equal (buffer-string) "hello")
        (should-equal (buffer-sync-content) (buffer-string))))

    (it-test "drain returns base64 updates"
      (lambda ()
        (define updates (buffer-drain-updates))
        (should (> (length updates) 0))))

    (it-test "disable sync"
      (lambda ()
        (buffer-disable-sync)
        (should-not (buffer-sync-enabled?))))))
