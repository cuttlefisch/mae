;;; test_kb.scm — Knowledge base help node access via ex-commands
;;;
;;; KB nodes are seeded at editor startup. This test verifies that built-in
;;; concept nodes are reachable via :help and that the resulting buffer
;;; contains expected content.

(describe-group "Knowledge base help"
  (lambda ()
    (it-test "open scheme-api help and verify content"
      (lambda ()
        (execute-ex "help concept:scheme-api")
        (should (> (string-length (buffer-string)) 0))
        (should-contain (buffer-string) "scheme")))

    (it-test "open hooks help and verify content"
      (lambda ()
        (execute-ex "help concept:hooks")
        (should (> (string-length (buffer-string)) 0))
        (should-contain (buffer-string) "hook")))

    (it-test "open scheme primitive help"
      (lambda ()
        (execute-ex "help scheme:buffer-insert")
        (should (> (string-length (buffer-string)) 0))))

    (it-test "nonexistent topic does not crash"
      (lambda ()
        (execute-ex "help nonexistent-topic-xyz-abc")
        (should (string? (buffer-string)))
        (run-command "enter-normal-mode")
        (should-mode "normal")))))
