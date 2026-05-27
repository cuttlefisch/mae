;;; test_replica.scm — Replicated repo test
;;;
;;; Both clients have a local file with the same name but different content.
;;; One shares, other joins. Joiner's content must be replaced by the shared version.
;;;
;;; No (run-tests) — uses Rust-side iteration for inject/apply between tests.

(load "/tests/lib/test-helpers.scm")

(describe-group "Replicated repo (both have local files)"
  (lambda ()

    (it-test "connects to server"
      (lambda ()
        (wait-connected 30000)))

    (it-test "creates local file with unique content"
      (lambda ()
        (write-file "/workspace/replica.txt" "local-only content\n")))

    (it-test "opens local file"
      (lambda ()
        (open-file "/workspace/replica.txt")
        (sleep-ms 200)))

    (it-test "verifies local content loaded"
      (lambda ()
        (should (string-contains? (buffer-string) "local-only content"))))

    (it-test "shares the local file"
      (lambda ()
        (run-command "collab-share")
        (wait-synced "replica.txt" 15000)))

    (it-test "buffer still has correct content after share"
      (lambda ()
        (let ((text (buffer-string)))
          (should (string-contains? text "local-only content")))))))
