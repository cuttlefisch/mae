;;; test_share_state.scm — Share workflow state transitions (no server needed)
;;;
;;; Validates that collab-share enables sync on the active buffer.

(describe-group "Share state transitions"
  (lambda ()
    (it-test "collab-share enables sync and content matches"
      (lambda ()
        (create-buffer "*share-test*")
        (buffer-insert "test content")
        (should-not (buffer-sync-enabled?))
        (run-command "collab-share")
        (should (buffer-sync-enabled?))
        (should-equal (buffer-sync-content) (buffer-string))))))
