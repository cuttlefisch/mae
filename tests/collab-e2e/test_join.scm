;;; test_join.scm — Client B: Join workflow
;;;
;;; Waits for Client A to share, joins the document, edits,
;;; verifies round-trip CRDT convergence.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.
;;; Uses sleep-ms instead of wait-until (sleep is processed between test steps).

(describe-group "Client B: Join workflow"
  (lambda ()

    (it-test "connects to state server"
      (lambda ()
        ;; Give collab bridge time to connect.
        (sleep-ms 5000)))

    (it-test "waits for Client A to share"
      (lambda ()
        ;; Fixed delay — Client A signals via /sync/a-shared.
        ;; In docker, Client A should be ready within ~15s.
        (sleep-ms 15000)))

    (it-test "joins the shared document"
      (lambda ()
        (execute-ex "collab-join test.txt")
        (sleep-ms 3000)))

    (it-test "verifies join succeeded"
      (lambda ()
        (should (get-buffer-by-name "test.txt"))))

    (it-test "has Client A's content"
      (lambda ()
        (let ((text (buffer-text "test.txt")))
          (should (string-contains? text "Hello from Client A")))))

    (it-test "edits and syncs back"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "test.txt"))
        (run-command "move-to-last-line")
        (run-command "enter-insert-mode")
        (buffer-insert "Hello from Client B\n")
        (run-command "enter-normal-mode")
        (sleep-ms 3000)))

    (it-test "saves to local disk"
      (lambda ()
        (execute-ex "w /workspace/test.txt")
        (sleep-ms 500)))))
