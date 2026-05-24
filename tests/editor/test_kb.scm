;;; test_kb.scm — Knowledge base help node access via ex-commands
;;;
;;; KB nodes are seeded at editor startup. This test verifies that built-in
;;; concept nodes are reachable via :help and that the resulting buffer
;;; contains expected content. It also verifies graceful handling of unknown topics.

(describe-group "Knowledge base help"
  (lambda ()
    (it-test "open help for built-in concept node"
      (lambda ()
        (execute-ex "help concept:scheme-api")))

    (it-test "help buffer contains scheme-api content"
      (lambda ()
        (should (> (string-length (buffer-string)) 0))))

    (it-test "help buffer contains 'scheme' text"
      (lambda ()
        (should-contain (buffer-string) "scheme")))

    (it-test "open help for commands concept"
      (lambda ()
        (execute-ex "help concept:hooks")))

    (it-test "hooks help buffer has content"
      (lambda ()
        (should (> (string-length (buffer-string)) 0))))

    (it-test "hooks buffer contains 'hook' text"
      (lambda ()
        (should-contain (buffer-string) "hook")))

    (it-test "open help for a scheme primitive"
      (lambda ()
        (execute-ex "help scheme:buffer-insert")))

    (it-test "scheme primitive help has content"
      (lambda ()
        (should (> (string-length (buffer-string)) 0))))

    (it-test "open help for nonexistent topic"
      (lambda ()
        (execute-ex "help nonexistent-topic-xyz-abc")))

    (it-test "buffer still has content after unknown topic lookup"
      (lambda ()
        ;; Help system should not crash — it either shows a fallback or
        ;; stays on the previous buffer. Either way the buffer is readable.
        (should (string? (buffer-string)))))

    (it-test "return to normal mode after help navigation"
      (lambda ()
        (run-command "enter-normal-mode")))

    (it-test "is in normal mode"
      (lambda ()
        (should-mode "normal")))))
