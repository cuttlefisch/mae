;;; test_search.scm — Buffer search forward operations
;;;
;;; Verifies test-search-forward returns correct char offsets for known
;;; patterns and returns #f for patterns not present in the buffer.

(describe-group "Buffer search"
  (lambda ()
    (it-test "search finds patterns at correct offsets"
      (lambda ()
        (create-buffer "*test-search*")
        (buffer-insert "the quick brown fox\njumps over the lazy dog\nfoo bar baz\n")
        (should-contain (buffer-string) "quick")
        (goto-char 0)
        ;; Search for 'quick'
        (let ((offset (test-search-forward "quick")))
          (should offset)
          (should-equal offset 4))
        ;; Search for 'fox'
        (let ((offset (test-search-forward "fox")))
          (should offset)
          (should-equal offset 16))
        ;; Search for 'jumps'
        (let ((offset (test-search-forward "jumps")))
          (should offset)
          (should-equal offset 20))))

    (it-test "search for nonexistent pattern returns false"
      (lambda ()
        (should-not (test-search-forward "nonexistent-pattern-xyz"))))

    (it-test "search for 'baz' near end returns an offset"
      (lambda ()
        (should (test-search-forward "baz"))))))
