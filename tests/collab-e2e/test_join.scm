;;; test_join.scm — Client B: Join workflow
;;;
;;; Waits for Client A to share, joins the document, edits,
;;; verifies round-trip CRDT convergence. Joined buffers have no
;;; auto file_path — uses :saveas to create local copies.
;;;
;;; SYNC STRATEGY: Content-based barriers via wait-for-content / wait-buffer-exists.
;;; NO fixed sleep-ms for CRDT propagation.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(load "/tests/lib/test-helpers.scm")

(describe-group "Client B: Join workflow"
  (lambda ()

    ;; --- Connect ---
    (it-test "connects to daemon"
      (lambda ()
        (wait-connected 30000)))

    (it-test "waits for Client A to share"
      (lambda ()
        (wait-for-file "/sync/a-shared" 60000)))

    ;; --- Scenario 1: Join + edit + sync ---
    (it-test "joins the shared document"
      (lambda ()
        (execute-ex "collab-join test.txt")
        ;; Wait until the buffer actually exists (created by join handler).
        (wait-buffer-exists "test.txt" 30000)))

    (it-test "verifies join succeeded"
      (lambda ()
        (should (get-buffer-by-name "test.txt"))))

    (it-test "has Client A's content"
      (lambda ()
        ;; Content barrier: wait until A's text has propagated.
        (wait-for-content "test.txt" "Hello from Client A" 30000)))

    (it-test "switches to joined buffer"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "test.txt"))
        (run-command "move-to-last-line")))

    (it-test "edits and syncs back"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "Hello from Client B\n")
        (run-command "enter-normal-mode")
        ;; Brief settle for the CRDT transaction to be generated.
        (sleep-ms 500)))

    ;; Signal to Client A that B's edit is done.
    ;; Note: Client A uses wait-for-content, so it won't check until
    ;; the CRDT update has actually arrived — no race condition.
    (it-test "signals edit done"
      (lambda ()
        (write-file "/sync/b-edit-done" "done")))

    (it-test "saves to local disk with explicit path"
      (lambda ()
        (execute-ex "saveas /workspace/test.txt")
        (sleep-ms 200)))

    ;; --- Scenario 2: Save to shared filesystem (after A finishes) ---
    (it-test "waits for Client A to save shared"
      (lambda ()
        (wait-for-file "/sync/a-saved-shared" 60000)))

    (it-test "saves to shared disk"
      (lambda ()
        (execute-ex "saveas /shared/test.txt")
        (sleep-ms 200)))

    (it-test "signals client-b done"
      (lambda ()
        (write-file "/sync/client-b-done" "done")))))
