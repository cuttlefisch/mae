;;; test_join.scm — Client B: Join workflow
;;;
;;; Waits for Client A to share, joins the document, edits,
;;; verifies round-trip CRDT convergence. Joined buffers have no
;;; auto file_path — uses :saveas to create local copies.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.
;;; Uses wait-for-file for inter-client synchronization (native yield, event-loop-aware).

(describe-group "Client B: Join workflow"
  (lambda ()

    (it-test "connects to state server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "waits for Client A to share"
      (lambda ()
        (wait-for-file "/sync/a-shared" 60000)))

    ;; --- Scenario 1: Join + edit + sync ---
    (it-test "joins the shared document"
      (lambda ()
        (execute-ex "collab-join test.txt")
        (sleep-ms 5000)))

    (it-test "verifies join succeeded"
      (lambda ()
        (should (get-buffer-by-name "test.txt"))))

    (it-test "has Client A's content"
      (lambda ()
        (let ((text (buffer-text "test.txt")))
          (should (string-contains? text "Hello from Client A")))))

    (it-test "switches to joined buffer"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "test.txt"))
        (run-command "move-to-last-line")))

    (it-test "edits and syncs back"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "Hello from Client B\n")
        (run-command "enter-normal-mode")
        (sleep-ms 3000)))

    ;; Signal to Client A that B's edit is done.
    (it-test "signals edit done"
      (lambda ()
        (write-file "/sync/b-edit-done" "done")))

    (it-test "saves to local disk with explicit path"
      (lambda ()
        (execute-ex "saveas /workspace/test.txt")
        (sleep-ms 500)))

    ;; --- Scenario 2: Save to shared filesystem (after A finishes) ---
    (it-test "waits for Client A to save shared"
      (lambda ()
        (wait-for-file "/sync/a-saved-shared" 60000)))

    (it-test "saves to shared disk"
      (lambda ()
        (execute-ex "saveas /shared/test.txt")
        (sleep-ms 500)))

    (it-test "signals client-b done"
      (lambda ()
        (write-file "/sync/client-b-done" "done")))))
