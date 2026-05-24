;;; test_replica.scm — Replicated repo test
;;;
;;; Both clients have a local file with the same name but different content.
;;; One shares, other joins. Joiner's content must be replaced by the shared version.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(describe-group "Replicated repo (both have local files)"
  (lambda ()

    (it-test "connects to server"
      (lambda ()
        (sleep-ms 5000)))

    (it-test "creates local file with unique content"
      (lambda ()
        (write-file "/workspace/replica.txt" "local-only content\n")
        (sleep-ms 200)
        (open-file "/workspace/replica.txt")
        (sleep-ms 200)))

    (it-test "verifies local content loaded"
      (lambda ()
        (should (string-contains? (buffer-string) "local-only content"))))

    (it-test "shares the local file"
      (lambda ()
        (run-command "collab-share")
        (sleep-ms 4000)))

    (it-test "buffer still has correct content after share"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "local-only content")))))))
