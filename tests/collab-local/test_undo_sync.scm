;;; test_undo_sync.scm — Undo/redo with sync enabled (no server needed)
;;;
;;; Validates that undo on a synced buffer properly reverts both the rope
;;; and the CRDT document, keeping them in sync.
;;;
;;; With capture_timeout_millis = u64::MAX, sequential inserts merge into
;;; one undo item unless separated by an explicit boundary.

(describe-group "Undo with sync"
  (lambda ()
    (it-test "setup synced buffer"
      (lambda ()
        (create-buffer "*undo-sync-test*")
        (buffer-enable-sync 1)))
    (it-test "insert first"
      (lambda ()
        (buffer-insert "first")))
    (it-test "verify first"
      (lambda ()
        (should-equal (buffer-string) "first")))
    (it-test "mark undo boundary"
      (lambda ()
        (buffer-undo-boundary)))
    (it-test "insert second"
      (lambda ()
        (buffer-insert " second")))
    (it-test "verify both"
      (lambda ()
        (should-equal (buffer-string) "first second")))
    (it-test "undo removes second"
      (lambda ()
        (run-command "undo")))
    (it-test "verify undo result"
      (lambda ()
        (should-equal (buffer-string) "first")))
    (it-test "sync content matches after undo"
      (lambda ()
        (should-equal (buffer-sync-content) (buffer-string))))))
