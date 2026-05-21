;;; test_share.scm — Client A: Share workflow
;;;
;;; Creates a file, shares it via collab, waits for Client B's edit,
;;; verifies CRDT convergence with no duplication. Tests both separate
;;; and shared filesystem save scenarios.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.
;;; Uses sleep-ms instead of wait-until (sleep is processed between test steps).

(describe-group "Client A: Share workflow"
  (lambda ()

    (it-test "connects to state server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    ;; --- Scenario 1: Separate filesystems ---
    ;; Each pending op (open-file, buffer-insert, run-command) is processed
    ;; by apply_to_editor AFTER the test step. Split into separate steps so
    ;; open-file completes before buffer-insert targets the new buffer.
    ;; Create the file first (open-file fails on non-existent files).
    (it-test "creates test file"
      (lambda ()
        (write-file "/workspace/test.txt" "")))

    (it-test "opens test file"
      (lambda ()
        (open-file "/workspace/test.txt")))

    (it-test "inserts content and saves"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "Hello from Client A\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (sleep-ms 500)))

    (it-test "shares the file"
      (lambda ()
        (run-command "collab-share")
        (sleep-ms 3000)))

    (it-test "signals readiness to Client B"
      (lambda ()
        (write-file "/sync/a-shared" "ready")))

    (it-test "receives Client B's edit"
      (lambda ()
        ;; Wait for Client B to join, edit, and sync back.
        (sleep-ms 30000)))

    (it-test "verifies Client B's content"
      (lambda ()
        (should (string-contains? (buffer-text "test.txt") "Hello from Client B"))))

    (it-test "has no content duplication"
      (lambda ()
        (let ((text (buffer-text "test.txt")))
          (should-not (string-contains? text "Hello from Client A\nHello from Client A")))))

    (it-test "saves converged state to local disk"
      (lambda ()
        (run-command "save")
        (sleep-ms 500)))

    ;; --- Scenario 2: Shared filesystem ---
    (it-test "saves converged state to shared disk"
      (lambda ()
        (execute-ex "saveas /shared/test.txt")
        (sleep-ms 500)))

    (it-test "signals save complete"
      (lambda ()
        (write-file "/sync/a-saved-shared" "done")))))
