;;; test_multi_buffer.scm — Multiple buffer creation and navigation
;;;
;;; Creates 3 named buffers, inserts distinct content into each, verifies
;;; buffer-string and get-buffer-by-name, then exercises next-buffer navigation.

(define *buf-a* "*test-mb-alpha*")
(define *buf-b* "*test-mb-beta*")
(define *buf-c* "*test-mb-gamma*")

(describe-group "Multi-buffer navigation"
  (lambda ()
    (it-test "create and populate three buffers"
      (lambda ()
        (create-buffer *buf-a*)
        (buffer-insert "alpha content")
        (should-equal (buffer-string) "alpha content")
        (create-buffer *buf-b*)
        (buffer-insert "beta content")
        (should-equal (buffer-string) "beta content")
        (create-buffer *buf-c*)
        (buffer-insert "gamma content")
        (should-equal (buffer-string) "gamma content")))

    (it-test "get-buffer-by-name finds all buffers"
      (lambda ()
        (should (get-buffer-by-name *buf-a*))
        (should (get-buffer-by-name *buf-b*))
        (should (get-buffer-by-name *buf-c*))
        (should-not (get-buffer-by-name "*test-mb-nonexistent*"))))

    (it-test "next-buffer navigation cycles"
      (lambda ()
        (run-command "next-buffer")
        (should (string? (buffer-string)))
        (run-command "next-buffer")
        (should (string? (buffer-string)))))))
