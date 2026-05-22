;;; test_undo_sharer.scm — Client A (sharer) for CRDT undo E2E test
;;;
;;; Scenario: A shares a buffer, both A and B make edits, A undoes its
;;; own edit, verifies B's edit is preserved, then checks final convergence.
;;;
;;; Coordination via /sync volume (file-based signaling with client B).
;;; Timing: A signals first, B uses static sleep to ensure A is ready,
;;; then signals back.  sleep-ms is processed by the test runner which
;;; drains collab events during the wait.
;;;
;;; No (run-tests) — uses Rust-side iteration.

(describe-group "CRDT undo — sharer (Client A)"
  (lambda ()

    ;; Clean stale signal files from previous Docker runs.
    (it-test "cleans sync signals"
      (lambda ()
        (write-file "/sync/a-edit-done" "")
        (write-file "/sync/b-edit-done" "")
        (write-file "/sync/a-undo-done" "")
        (write-file "/sync/a-all-done" "")))

    (it-test "connects to state server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    ;; Create the file first (open-file fails on non-existent files).
    (it-test "creates test file"
      (lambda ()
        (write-file "/workspace/undo-test.txt" "")))

    (it-test "opens test file"
      (lambda ()
        (open-file "/workspace/undo-test.txt")))

    (it-test "inserts base content"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "base\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (sleep-ms 500)))

    (it-test "shares the buffer"
      (lambda ()
        (run-command "collab-share")
        (sleep-ms 3000)))

    (it-test "verifies sync is active"
      (lambda ()
        (should (buffer-sync-enabled?))))

    ;; --- Round 1: A edits ---
    (it-test "A inserts 'from-A'"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "from-A\n")
        (run-command "enter-normal-mode")
        (sleep-ms 2000)))

    (it-test "signals A edit done"
      (lambda ()
        (write-file "/sync/a-edit-done" "ready")))

    ;; --- Wait for B's edit ---
    ;; B needs: see signal (~instant) + join (5s) + insert (3s) + signal = ~10s
    ;; Use 30s to be safe.
    (it-test "waits for B's edit to propagate"
      (lambda ()
        (sleep-ms 30000)))

    (it-test "verifies B's edit arrived via CRDT"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "from-B")))))

    ;; --- Round 2: A undoes its own edit ---
    (it-test "A undoes its own edit"
      (lambda ()
        (run-command "undo")
        (sleep-ms 3000)))

    (it-test "verifies A's undo preserved B's content"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-B"))
          (should-not (string-contains? text "from-A")))))

    (it-test "signals undo done"
      (lambda ()
        (write-file "/sync/a-undo-done" "done")))

    ;; --- Wait for B to verify convergence ---
    (it-test "waits for B to finish"
      (lambda ()
        (sleep-ms 15000)))

    ;; --- Round 3: A redoes ---
    (it-test "A redoes its edit"
      (lambda ()
        (run-command "redo")
        (sleep-ms 3000)))

    (it-test "verifies redo restored A's content (B already undid its edit)"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-A"))
          ;; B undid its own edit during the wait, so from-B should be gone.
          (should-not (string-contains? text "from-B")))))

    (it-test "saves final state"
      (lambda ()
        (run-command "save")
        (sleep-ms 500)))

    (it-test "signals all done"
      (lambda ()
        (write-file "/sync/a-all-done" "done")))

    ;; Brief wait for joiner to see the a-all-done signal and exit.
    ;; With wait-for-file on the joiner side, this can be short.
    (it-test "waits for joiner to finish"
      (lambda ()
        (sleep-ms 10000)))))
