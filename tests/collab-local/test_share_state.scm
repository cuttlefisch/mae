;;; test_share_state.scm — Share workflow state transitions (no server needed)
;;;
;;; Validates that collab-share enables sync on the active buffer.
;;; Note: synced-buffers list only updates on server confirmation (BufferShared event),
;;; so we can't test that here without a server.

(describe-group "Share state transitions"
  (lambda ()
    (it-test "setup: create buffer with content"
      (lambda ()
        (create-buffer "*share-test*")
        (buffer-insert "test content")))
    (it-test "before share: sync not enabled"
      (lambda ()
        (should-not (buffer-sync-enabled?))))
    (it-test "share the buffer"
      (lambda ()
        (run-command "collab-share")))
    (it-test "after share: sync is enabled"
      (lambda ()
        (should (buffer-sync-enabled?))))
    (it-test "after share: sync content matches rope"
      (lambda ()
        (should-equal (buffer-sync-content) (buffer-string))))))
