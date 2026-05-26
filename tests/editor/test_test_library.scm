;;; test_test_library.scm — Self-tests for the mae-test.scm library
;;;
;;; Meta-tests: verify that the testing assertions themselves work correctly.
;;; Covers should, should-not, should-equal, should-contain, should-error,
;;; should-match, should-mode, and utility functions.

(describe-group "Test library self-tests"
  (lambda ()
    ;; --- should ---
    (it-test "should passes on #t"
      (lambda ()
        (should #t)))

    (it-test "should passes on truthy integer"
      (lambda ()
        (should 42)))

    (it-test "should passes on truthy string"
      (lambda ()
        (should "non-empty")))

    ;; --- should-not ---
    (it-test "should-not passes on #f"
      (lambda ()
        (should-not #f)))

    ;; --- should-equal ---
    (it-test "should-equal passes for equal strings"
      (lambda ()
        (should-equal "hello" "hello")))

    (it-test "should-equal passes for equal numbers"
      (lambda ()
        (should-equal 42 42)))

    (it-test "should-equal passes for empty strings"
      (lambda ()
        (should-equal "" "")))

    ;; --- should-contain ---
    (it-test "should-contain finds substring at start"
      (lambda ()
        (should-contain "hello world" "hello")))

    (it-test "should-contain finds substring at end"
      (lambda ()
        (should-contain "hello world" "world")))

    (it-test "should-contain finds substring in middle"
      (lambda ()
        (should-contain "hello world" "lo wo")))

    (it-test "should-contain finds exact match"
      (lambda ()
        (should-contain "exact" "exact")))

    ;; --- should-error ---
    (it-test "should-error passes when error is raised"
      (lambda ()
        (should-error (lambda () (error "expected failure")))))

    (it-test "should-error catches division errors"
      (lambda ()
        (should-error (lambda () (/ 1 0)))))

    (it-test "should-error fails when no error raised"
      (lambda ()
        ;; Meta-test: should-error on a non-erroring thunk should itself error.
        ;; We wrap in another should-error to verify the expected failure.
        (should-error
          (lambda ()
            (should-error (lambda () 42))))))

    ;; --- should-match ---
    (it-test "should-match finds pattern in string"
      (lambda ()
        (should-match "the quick brown fox" "quick")))

    (it-test "should-match works with special chars"
      (lambda ()
        (should-match "file: /tmp/test.txt" "/tmp/")))

    ;; --- string-contains? ---
    (it-test "string-contains? returns #t for present substring"
      (lambda ()
        (should (string-contains? "abcdef" "cde"))))

    (it-test "string-contains? returns #f for absent substring"
      (lambda ()
        (should-not (string-contains? "abcdef" "xyz"))))

    (it-test "string-contains? handles empty needle"
      (lambda ()
        (should (string-contains? "abc" ""))))

    (it-test "string-contains? handles equal strings"
      (lambda ()
        (should (string-contains? "abc" "abc"))))

    ;; --- to-string ---
    (it-test "to-string converts number"
      (lambda ()
        (should-equal (to-string 42) "42")))

    (it-test "to-string converts boolean true"
      (lambda ()
        (should-equal (to-string #t) "#t")))

    (it-test "to-string converts boolean false"
      (lambda ()
        (should-equal (to-string #f) "#f")))

    (it-test "to-string passes through string"
      (lambda ()
        (should-equal (to-string "hello") "hello")))

    ;; --- should-greater-than ---
    (it-test "should-greater-than passes when a > b"
      (lambda ()
        (should-greater-than 10 5)))

    (it-test "should-greater-than fails when a <= b"
      (lambda ()
        (should-error (lambda () (should-greater-than 3 5)))))

    ;; --- should-less-than ---
    (it-test "should-less-than passes when a < b"
      (lambda ()
        (should-less-than 5 10)))

    (it-test "should-less-than fails when a >= b"
      (lambda ()
        (should-error (lambda () (should-less-than 10 5)))))

    ;; --- should-buffer-state ---
    (it-test "setup buffer for state check"
      (lambda ()
        (create-buffer "*test-buf-state*")))

    (it-test "insert content for state check"
      (lambda ()
        (buffer-insert "abc")))

    (it-test "goto known position"
      (lambda ()
        (cursor-goto 0 1)))

    (it-test "should-buffer-state passes with correct state"
      (lambda ()
        (should-buffer-state "abc" 0 1)))

    ;; --- cursor-row / cursor-col ---
    (it-test "cursor-row returns a number"
      (lambda ()
        (should (number? (test-cursor-row)))))

    (it-test "cursor-col returns a number"
      (lambda ()
        (should (number? (test-cursor-col)))))

    ;; --- status-message ---
    (it-test "status-message returns a string"
      (lambda ()
        (should (string? (test-status-message)))))))
