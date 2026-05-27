;;; test_share.scm — Client A: Share workflow
;;;
;;; Creates a file, shares it via collab, waits for Client B's edit to arrive
;;; via CRDT, verifies convergence. Tests both separate and shared filesystem
;;; save scenarios.
;;;
;;; SYNC STRATEGY: Content-based barriers via wait-for-content / wait-for-file.
;;; NO fixed sleep-ms for CRDT propagation — we poll until the expected content
;;; actually appears in the buffer, with collab events draining on every poll.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(load "/tests/lib/test-helpers.scm")

(describe-group "Client A: Share workflow"
  (lambda ()

    ;; --- Connect ---
    (it-test "connects to state server"
      (lambda ()
        (wait-connected 30000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    ;; --- Scenario 1: Separate filesystems ---
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
        ;; Brief settle for save to flush.
        (sleep-ms 200)))

    (it-test "shares the file"
      (lambda ()
        (run-command "collab-share")))

    (it-test "verifies sync is active"
      (lambda ()
        ;; wait-synced polls collab-synced-buffers until the buffer appears.
        ;; The share intent is drained between test steps by process_side_effects.
        (wait-synced "test.txt" 30000)))

    (it-test "signals readiness to Client B"
      (lambda ()
        (write-file "/sync/a-shared" "ready")))

    ;; --- Wait for B's edit via CRDT (content barrier, not timer) ---
    (it-test "waits for Client B's content via CRDT"
      (lambda ()
        ;; This polls buffer-text every 50ms, draining collab events each cycle.
        ;; No fixed sleep — returns as soon as CRDT delivers B's edit.
        (wait-for-content "test.txt" "Hello from Client B" 60000)))

    (it-test "has no content duplication"
      (lambda ()
        (let ((text (buffer-text "test.txt")))
          (should-not (string-contains? text "Hello from Client A\nHello from Client A")))))

    (it-test "saves converged state to local disk"
      (lambda ()
        (run-command "save")
        (sleep-ms 200)))

    ;; --- Scenario 2: Shared filesystem ---
    (it-test "saves converged state to shared disk"
      (lambda ()
        (execute-ex "saveas /shared/test.txt")
        (sleep-ms 200)))

    (it-test "signals save complete"
      (lambda ()
        (write-file "/sync/a-saved-shared" "done")))

    (it-test "signals client-a done"
      (lambda ()
        (write-file "/sync/client-a-done" "done")))))
