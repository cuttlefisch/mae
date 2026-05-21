;;; test_undo_sharer.scm — Client A (sharer) for CRDT undo E2E test
;;;
;;; Scenario: A shares a buffer, both A and B make edits, A undoes its
;;; own edit, verifies B's edit is preserved, then checks final convergence.
;;;
;;; Coordination via /sync volume (file-based signaling with client B).
;;; No (run-tests) — uses Rust-side iteration.

(describe-group "CRDT undo — sharer (Client A)"
  (lambda ()

    (it-test "connects to state server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "verifies connection"
      (lambda ()
        (let ((status (collab-status)))
          (should (pair? status)))))

    (it-test "creates and saves file"
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
        (sleep-ms 2000)))

    (it-test "verifies sync is active"
      (lambda ()
        (should (buffer-sync-enabled?))))

    ;; --- Round 1: A edits ---
    (it-test "A inserts 'from-A'"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "from-A\n")
        (run-command "enter-normal-mode")
        (sleep-ms 1000)))

    (it-test "signals A edit done"
      (lambda ()
        (write-file "/sync/a-edit-done" "1")))

    ;; --- Wait for B's edit ---
    (it-test "waits for B's edit"
      (lambda ()
        (sleep-ms 15000)))

    (it-test "verifies B's edit arrived via CRDT"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "from-B")))))

    ;; --- Round 2: A undoes its own edit ---
    (it-test "A undoes its own edit"
      (lambda ()
        (run-command "undo")
        (sleep-ms 2000)))

    (it-test "verifies A's undo preserved B's content"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-B"))
          (should-not (string-contains? text "from-A")))))

    (it-test "signals undo done"
      (lambda ()
        (write-file "/sync/a-undo-done" "1")))

    ;; --- Wait for B to verify convergence ---
    (it-test "waits for B to finish"
      (lambda ()
        (sleep-ms 10000)))

    ;; --- Round 3: A redoes ---
    (it-test "A redoes its edit"
      (lambda ()
        (run-command "redo")
        (sleep-ms 2000)))

    (it-test "verifies redo restored A's content alongside B's"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "base"))
          (should (string-contains? text "from-A"))
          (should (string-contains? text "from-B")))))

    (it-test "saves final state"
      (lambda ()
        (run-command "save")
        (sleep-ms 500)))

    (it-test "signals all done"
      (lambda ()
        (write-file "/sync/a-all-done" "1")))))
