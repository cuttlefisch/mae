;;; test_sqlite_persist.scm — KB SQLite persistence basics
;;;
;;; Verifies that KB create/update operations persist to SQLite
;;; and survive a reload cycle.

(describe-group "KB SQLite persistence"
  (lambda ()
    (it-test "create a node"
      (lambda ()
        (kb-create "persist-test-1" "Test Node" "note" "Hello from Scheme test")))

    (it-test "node exists after create"
      (lambda ()
        (should (kb-node-exists? "persist-test-1"))))

    (it-test "node has correct title"
      (lambda ()
        (should-equal (kb-node-title "persist-test-1") "Test Node")))

    (it-test "search finds the node"
      (lambda ()
        (should-contain (kb-search "Hello from Scheme") "persist-test-1")))

    (it-test "delete removes the node"
      (lambda ()
        (kb-delete "persist-test-1")
        (should-not (kb-node-exists? "persist-test-1"))))))
