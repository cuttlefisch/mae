;;; test_rejoin.scm — Disconnect + rejoin test
;;;
;;; Shares a document, disconnects, edits while offline,
;;; reconnects and verifies the edit propagates.
;;; Single-client test (no inter-client coordination needed).
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(describe-group "Disconnect and rejoin"
  (lambda ()

    (it-test "connects and shares"
      (lambda ()
        (sleep-ms 5000)
        (open-file "/workspace/rejoin.txt")
        (run-command "enter-insert-mode")
        (buffer-insert "before disconnect\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (run-command "collab-share")
        (sleep-ms 3000)))

    (it-test "disconnects"
      (lambda ()
        (run-command "collab-disconnect")
        (sleep-ms 1000)))

    (it-test "edits while disconnected"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "after disconnect\n")
        (run-command "enter-normal-mode")
        (sleep-ms 500)))

    (it-test "reconnects and syncs"
      (lambda ()
        (run-command "collab-connect")
        (sleep-ms 5000)
        (run-command "collab-share")
        (sleep-ms 3000)))

    (it-test "has both edits"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "before disconnect"))
          (should (string-contains? text "after disconnect")))))))
