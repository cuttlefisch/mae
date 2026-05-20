;;; test_undo_sync.scm — Undo with sync enabled
;;;
;;; Tests that undo works correctly when CRDT sync is active,
;;; and that the sync doc stays in agreement with the rope after undo.

(describe-group "Undo with sync enabled"
  (lambda ()
    (it-test "setup clean buffer with sync"
      (lambda ()
        (create-buffer "*test-undo-sync*")
        (buffer-enable-sync 1)))

    (it-test "insert first line"
      (lambda ()
        (buffer-insert "line 1\n")))

    (it-test "verify first insert"
      (lambda ()
        (should-contain (buffer-string) "line 1")))

    (it-test "insert second line"
      (lambda ()
        (buffer-insert "line 2\n")))

    (it-test "verify both lines present"
      (lambda ()
        (should-contain (buffer-string) "line 1")
        (should-contain (buffer-string) "line 2")))

    (it-test "undo removes last insert"
      (lambda ()
        (buffer-undo)))

    (it-test "verify undo result"
      (lambda ()
        (should-contain (buffer-string) "line 1")
        (should-not (string-contains? (buffer-string) "line 2"))))

    (it-test "sync doc matches after undo"
      (lambda ()
        (should-equal (buffer-sync-content) (buffer-string))))))
