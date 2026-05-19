;;; test_smoke.scm — Minimal smoke test for the mae-test framework
;;;
;;; Verifies that the test library loads, assertions work, and the
;;; test runner produces TAP output. No collab server needed.

(describe-group "mae-test framework"
  (lambda ()

    (it-test "should passes on truthy value"
      (lambda ()
        (should #t)))

    (it-test "should-not passes on falsy value"
      (lambda ()
        (should-not #f)))

    (it-test "should-equal compares values"
      (lambda ()
        (should-equal 42 42)
        (should-equal "hello" "hello")))

    (it-test "string-contains? works"
      (lambda ()
        (should (string-contains? "hello world" "world"))
        (should-not (string-contains? "hello" "xyz"))))

    (it-test "write-file works"
      (lambda ()
        (write-file "/tmp/mae-test-write-check" "test-content")
        ;; write-file is a pending operation; can't verify in same eval.
        ;; Just verify it doesn't error.
        (should #t)))

    (it-test "editor state is accessible"
      (lambda ()
        ;; *buffer-name* is injected before test loading
        (should (string? *buffer-name*))
        (should (number? *buffer-count*))
        (should (>= *buffer-count* 1))))))

(run-tests)
