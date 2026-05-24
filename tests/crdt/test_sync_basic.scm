;;; test_sync_basic.scm — Basic CRDT sync enable/insert/state tests
;;;
;;; Tests that enabling sync on a buffer works, that inserts generate
;;; sync updates, and that the yrs doc content matches the rope.

(describe-group "CRDT sync basics"
  (lambda ()
    (it-test "setup clean buffer"
      (lambda ()
        (create-buffer "*test-sync-basic*")))

    (it-test "enable sync on buffer"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "sync is enabled"
      (lambda ()
        (should (buffer-sync-enabled?))))

    (it-test "insert generates text in buffer"
      (lambda ()
        (buffer-insert "hello")))

    (it-test "buffer has inserted text"
      (lambda ()
        (should-equal (buffer-string) "hello")))

    (it-test "sync doc matches rope content"
      (lambda ()
        (should-equal (buffer-sync-content) (buffer-string))))

    (it-test "drain returns base64 updates"
      (lambda ()
        (define updates (buffer-drain-updates))
        (should (> (length updates) 0))))

    (it-test "disable sync"
      (lambda ()
        (buffer-disable-sync)))

    (it-test "sync is disabled after disable"
      (lambda ()
        (should-not (buffer-sync-enabled?))))))
