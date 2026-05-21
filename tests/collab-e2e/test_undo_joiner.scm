;;; test_undo_joiner.scm — Client B (joiner) for CRDT undo E2E test
;;;
;;; Scenario: B joins A's shared buffer, makes its own edit, then verifies
;;; that A's undo does NOT undo B's edit (per-user undo isolation).
;;;
;;; Coordination via /sync volume (file-based signaling with client A).
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
    (it-test "waits for A's edit signal"
      (lambda ()
        (sleep-ms 20000)))

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

    ;; --- Wait for A to undo ---
    (it-test "waits for A's undo"
      (lambda ()
        (sleep-ms 15000)))

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
        (sleep-ms 500)))))
