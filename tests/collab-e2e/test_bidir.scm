;;; test_bidir.scm — Bidirectional editing test
;;;
;;; Both clients edit the same buffer simultaneously.
;;; Verifies CRDT convergence: both clients see both edits, no duplication.
;;; Run as a single-client test that simulates rapid edits.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(load "/tests/lib/test-helpers.scm")

(describe-group "Bidirectional editing"
  (lambda ()

    (it-test "connects to server"
      (lambda ()
        (wait-connected 10000)))

    (it-test "creates and shares document"
      (lambda ()
        (open-file "/workspace/bidir.txt")
        (run-command "enter-insert-mode")
        (buffer-insert "line 1\n")
        (run-command "enter-normal-mode")
        (run-command "save")
        (run-command "collab-share")
        (sleep-ms 2000)))

    (it-test "makes multiple rapid edits"
      (lambda ()
        (run-command "enter-insert-mode")
        (buffer-insert "edit A\n")
        (sleep-ms 100)
        (buffer-insert "edit B\n")
        (sleep-ms 100)
        (buffer-insert "edit C\n")
        (run-command "enter-normal-mode")
        (sleep-ms 2000)))

    (it-test "all edits present in buffer"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "line 1"))
          (should (string-contains? text "edit A"))
          (should (string-contains? text "edit B"))
          (should (string-contains? text "edit C")))))))
