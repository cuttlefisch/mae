;;; test_undo_joiner.scm — Client B (joiner) for CRDT undo E2E test
;;;
;;; Scenario: B joins A's shared buffer, makes its own edit, then verifies
;;; that A's undo does NOT undo B's edit (per-user undo isolation).
;;;
;;; SYNC STRATEGY: Content-based barriers via wait-for-content / wait-content-absent.
;;; All CRDT-dependent assertions are preceded by content polls that drain
;;; collab events until the expected state is reached. No fixed sleep-ms
;;; for convergence.
;;;
;;; No (run-tests) — uses Rust-side iteration.

(load "/tests/lib/test-helpers.scm")

(describe-group "CRDT undo — joiner (Client B)"
  (lambda ()

    (it-test "connects to daemon"
      (lambda ()
        (wait-connected 30000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    ;; --- Wait for A to share and edit ---
    (it-test "waits for A to share and edit"
      (lambda ()
        (wait-for-file "/sync/a-edit-done" 60000)))

    ;; --- Join the shared document ---
    (it-test "joins the shared document"
      (lambda ()
        (execute-ex "collab-join undo-test.txt")
        ;; Wait until the buffer actually appears.
        (wait-buffer-exists "undo-test.txt" 30000)))

    (it-test "verifies join succeeded"
      (lambda ()
        (should (get-buffer-by-name "undo-test.txt"))))

    (it-test "switches to joined buffer"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "undo-test.txt"))))

    ;; --- Content barrier: wait for A's content to arrive via CRDT ---
    (it-test "waits for A's content via CRDT"
      (lambda ()
        (wait-for-content "undo-test.txt" "from-A" 30000)))

    (it-test "has A's content"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-A")))))

    ;; --- B makes its own edit ---
    (it-test "B inserts 'from-B'"
      (lambda ()
        (run-command "move-to-last-line")
        (run-command "enter-insert-mode")
        (buffer-insert "from-B\n")
        (run-command "enter-normal-mode")
        ;; Brief settle for CRDT transaction generation.
        (sleep-ms 500)))

    (it-test "verifies B's edit is in buffer"
      (lambda ()
        (should (string-contains? (buffer-string) "from-B"))))

    (it-test "signals B edit done"
      (lambda ()
        (write-file "/sync/b-edit-done" "done")))

    ;; --- Wait for A's undo to propagate via CRDT ---
    (it-test "waits for A's undo signal"
      (lambda ()
        (wait-for-file "/sync/a-undo-done" 60000)))

    ;; Content barrier: wait until "from-A" is actually removed by CRDT.
    (it-test "waits for A's undo to propagate via CRDT"
      (lambda ()
        (wait-content-absent "undo-test.txt" "from-A" 30000)))

    (it-test "verifies A's undo removed only A's text"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-B"))
          (should-not (string-contains? text "from-A")))))

    ;; --- B undoes its own edit ---
    (it-test "B undoes its own edit"
      (lambda ()
        (run-command "undo")
        ;; Content barrier: wait until "from-B" is gone.
        (wait-content-absent "undo-test.txt" "from-B" 30000)))

    (it-test "verifies B's undo removed only B's text"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should-not (string-contains? text "from-B"))
          (should-not (string-contains? text "from-A")))))

    ;; Signal B's undo is done so A can proceed with redo.
    (it-test "signals B undo done"
      (lambda ()
        (write-file "/sync/b-undo-done" "done")))

    (it-test "saves B's final state"
      (lambda ()
        (execute-ex "saveas /workspace/undo-test.txt")
        (sleep-ms 200)))

    ;; Wait for A to finish (redo + save + signal).
    (it-test "waits for A to finish"
      (lambda ()
        (wait-for-file "/sync/a-all-done" 60000)))

    (it-test "verifies A finished"
      (lambda ()
        (should (file-exists? "/sync/a-all-done"))))))
