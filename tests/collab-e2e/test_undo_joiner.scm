;;; test_undo_joiner.scm — Client B (joiner) for CRDT undo E2E test
;;;
;;; Scenario: B joins A's shared buffer, makes its own edit, then verifies
;;; that A's undo does NOT undo B's edit (per-user undo isolation).
;;;
;;; Coordination: A starts first and signals via /sync/a-edit-done.
;;; B waits long enough for A to share + edit + signal, then joins.
;;; sleep-ms is processed by the test runner which drains collab events.
;;;
;;; No (run-tests) — uses Rust-side iteration.

(describe-group "CRDT undo — joiner (Client B)"
  (lambda ()

    (it-test "connects to state server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    ;; --- Wait for A to share and edit ---
    ;; A needs: 5s connect + ~3s setup + 3s share + 2s insert + signal = ~13s
    ;; Sharer also cleans signal files first, adding ~1s.
    ;; Use 20s static sleep to be safe.
    (it-test "waits for A to share and edit"
      (lambda ()
        (sleep-ms 20000)))

    (it-test "verifies A's signal file exists"
      (lambda ()
        (should (file-exists? "/sync/a-edit-done"))
        (should (string-contains?
                  (read-file "/sync/a-edit-done")
                  "ready"))))

    ;; --- Join the shared document ---
    (it-test "joins the shared document"
      (lambda ()
        (execute-ex "collab-join undo-test.txt")
        (sleep-ms 5000)))

    (it-test "verifies join succeeded"
      (lambda ()
        (should (get-buffer-by-name "undo-test.txt"))))

    (it-test "switches to joined buffer"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "undo-test.txt"))))

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
        (sleep-ms 3000)))

    (it-test "verifies B's edit is in buffer"
      (lambda ()
        (should (string-contains? (buffer-string) "from-B"))))

    (it-test "signals B edit done"
      (lambda ()
        (write-file "/sync/b-edit-done" "done")))

    ;; --- Wait for A's undo to propagate ---
    ;; A: sees B's signal after ~30s wait, verifies, undoes (+3s), signals.
    ;; B signals at ~35s, A's 30s wait started at ~15s, so A sees it at ~35s.
    ;; A then undoes + signals by ~38s.  We're at ~35s now.
    ;; Use 20s sleep to wait for the undo propagation.
    (it-test "waits for A's undo"
      (lambda ()
        (sleep-ms 20000)))

    (it-test "verifies A's undo signal"
      (lambda ()
        (should (file-exists? "/sync/a-undo-done"))))

    ;; Allow time for the undo CRDT update to apply locally.
    (it-test "allows CRDT propagation"
      (lambda ()
        (sleep-ms 3000)))

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
        (sleep-ms 2000)))

    (it-test "verifies B's undo removed only B's text"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should-not (string-contains? text "from-B"))
          ;; A's text was already undone by A
          (should-not (string-contains? text "from-A")))))

    (it-test "saves B's final state"
      (lambda ()
        (execute-ex "saveas /workspace/undo-test.txt")
        (sleep-ms 500)))

    ;; Wait for A's redo + final save + signal before exiting.
    ;; A needs ~18s after this point (15s wait + 3s redo + signal).
    (it-test "waits for A to finish"
      (lambda ()
        (sleep-ms 25000)))

    (it-test "verifies A finished"
      (lambda ()
        (should (file-exists? "/sync/a-all-done"))))))
