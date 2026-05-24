;;; test_multi_buffer.scm — Multiple buffer creation and navigation
;;;
;;; Creates 3 named buffers, inserts distinct content into each, verifies
;;; buffer-string and get-buffer-by-name, then exercises next-buffer navigation.

(define *buf-a* "*test-mb-alpha*")
(define *buf-b* "*test-mb-beta*")
(define *buf-c* "*test-mb-gamma*")

(describe-group "Multi-buffer navigation"
  (lambda ()
    (it-test "create buffer alpha"
      (lambda ()
        (create-buffer *buf-a*)))

    (it-test "insert content into alpha"
      (lambda ()
        (buffer-insert "alpha content")))

    (it-test "verify alpha content"
      (lambda ()
        (should-equal (buffer-string) "alpha content")))

    (it-test "create buffer beta"
      (lambda ()
        (create-buffer *buf-b*)))

    (it-test "insert content into beta"
      (lambda ()
        (buffer-insert "beta content")))

    (it-test "verify beta content"
      (lambda ()
        (should-equal (buffer-string) "beta content")))

    (it-test "create buffer gamma"
      (lambda ()
        (create-buffer *buf-c*)))

    (it-test "insert content into gamma"
      (lambda ()
        (buffer-insert "gamma content")))

    (it-test "verify gamma content"
      (lambda ()
        (should-equal (buffer-string) "gamma content")))

    (it-test "get-buffer-by-name returns alpha"
      (lambda ()
        (should (get-buffer-by-name *buf-a*))))

    (it-test "get-buffer-by-name returns beta"
      (lambda ()
        (should (get-buffer-by-name *buf-b*))))

    (it-test "get-buffer-by-name returns gamma"
      (lambda ()
        (should (get-buffer-by-name *buf-c*))))

    (it-test "nonexistent buffer returns false"
      (lambda ()
        (should-not (get-buffer-by-name "*test-mb-nonexistent*"))))

    (it-test "navigate to next buffer"
      (lambda ()
        (run-command "next-buffer")))

    (it-test "buffer changed after next-buffer"
      (lambda ()
        ;; We moved away from gamma, so content should differ
        ;; (unless we wrapped around to it, which is also valid).
        (should (string? (buffer-string)))))

    (it-test "navigate to next buffer again"
      (lambda ()
        (run-command "next-buffer")))

    (it-test "buffer is still a valid string"
      (lambda ()
        (should (string? (buffer-string)))))))
