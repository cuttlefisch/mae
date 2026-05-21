;;; test_undo.scm — CRDT undo test (single-client)
;;;
;;; Verifies per-user undo via yrs UndoManager:
;;; 1. Insert multiple lines
;;; 2. Undo one line — only that line reversed
;;; 3. Redo — line restored
;;; 4. Verify sync updates generated (pending_sync_updates non-empty)
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(describe-group "CRDT per-user undo"
  (lambda ()

    (it-test "creates a synced buffer"
      (lambda ()
        (open-file "/workspace/undo-test.txt")))

    (it-test "enters insert mode and adds line 1"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "line one\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (sleep-ms 500)))

    (it-test "adds line 2"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "line two\n")
        (run-command "enter-normal-mode")
        (sleep-ms 200)))

    (it-test "verifies both lines present"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "line one"))
          (should (string-contains? text "line two")))))

    (it-test "undoes last edit"
      (lambda ()
        (run-command "undo")
        (sleep-ms 200)))

    (it-test "verifies undo removed line 2 only"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "line one"))
          (should-not (string-contains? text "line two")))))

    (it-test "redoes the edit"
      (lambda ()
        (run-command "redo")
        (sleep-ms 200)))

    (it-test "verifies redo restored line 2"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "line one"))
          (should (string-contains? text "line two")))))))
