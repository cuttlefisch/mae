;;; test_undo_sharer.scm — Client A (sharer) for CRDT undo E2E test
;;;
;;; Scenario: A shares a buffer, both A and B make edits, A undoes its
;;; own edit, verifies B's edit is preserved, then checks final convergence.
;;;
;;; SYNC STRATEGY: Content-based barriers via wait-for-content / wait-content-absent.
;;; After every CRDT-dependent step, we poll the buffer for expected content
;;; instead of using fixed sleep-ms. The test runner drains collab events
;;; on each poll cycle, so CRDT updates are applied between checks.
;;;
;;; No (run-tests) — uses Rust-side iteration.

(load "/tests/lib/test-helpers.scm")

(describe-group "CRDT undo — sharer (Client A)"
  (lambda ()

    ;; Docker volumes are created fresh each run (docker-compose down --volumes),
    ;; so no signal cleanup is needed.

    (it-test "connects to state server"
      (lambda ()
        (wait-connected 30000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

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
        (sleep-ms 200)))

    (it-test "shares the buffer"
      (lambda ()
        (run-command "collab-share")))

    ;; Separate step so apply_to_editor drains the share intent first.
    (it-test "waits for sync to activate"
      (lambda ()
        (wait-synced "undo-test.txt" 30000)))

    (it-test "verifies sync is active"
      (lambda ()
        (should (buffer-sync-enabled?))))

    ;; --- Round 1: A edits ---
    (it-test "A inserts 'from-A'"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "from-A\n")
        (run-command "enter-normal-mode")
        ;; Brief settle for CRDT transaction generation.
        (sleep-ms 500)))

    (it-test "signals A edit done"
      (lambda ()
        (write-file "/sync/a-edit-done" "ready")))

    ;; --- Wait for B's edit via CRDT content barrier ---
    (it-test "waits for B's edit to arrive via CRDT"
      (lambda ()
        ;; Polls buffer-string every 50ms until "from-B" appears.
        ;; No fixed sleep — returns as soon as CRDT delivers.
        (wait-for-content "undo-test.txt" "from-B" 60000)))

    (it-test "verifies B's edit is present"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "from-B")))))

    ;; --- Round 2: A undoes its own edit ---
    (it-test "A undoes its own edit"
      (lambda ()
        (run-command "undo")
        ;; Wait until "from-A" is actually gone (CRDT propagation of undo).
        (wait-content-absent "undo-test.txt" "from-A" 30000)))

    (it-test "verifies A's undo preserved B's content"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-B"))
          (should-not (string-contains? text "from-A")))))

    (it-test "signals undo done"
      (lambda ()
        (write-file "/sync/a-undo-done" "done")))

    ;; --- Wait for B to undo + finish ---
    (it-test "waits for B to finish undo"
      (lambda ()
        (wait-for-file "/sync/b-undo-done" 60000)))

    ;; --- Round 3: A redoes ---
    (it-test "A redoes its edit"
      (lambda ()
        (run-command "redo")
        ;; Wait until "from-A" reappears via redo.
        (wait-for-content "undo-test.txt" "from-A" 30000)))

    (it-test "verifies redo restored A's content (B already undid its edit)"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-A"))
          ;; B undid its own edit, so from-B should be gone.
          ;; But wait — we need to wait for B's undo to propagate too.
          )))

    ;; Wait for B's undo to propagate (from-B should be gone).
    (it-test "waits for B's undo to propagate"
      (lambda ()
        (wait-content-absent "undo-test.txt" "from-B" 30000)))

    (it-test "verifies final converged state"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-A"))
          (should-not (string-contains? text "from-B")))))

    (it-test "saves final state"
      (lambda ()
        (run-command "save")
        (sleep-ms 200)))

    (it-test "signals all done"
      (lambda ()
        (write-file "/sync/a-all-done" "done")))

    ;; Brief wait for joiner to see the a-all-done signal.
    (it-test "waits for joiner to exit"
      (lambda ()
        (sleep-ms 3000)))))
