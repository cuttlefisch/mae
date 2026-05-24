;;; test_search.scm — Buffer search forward operations
;;;
;;; Verifies buffer-search-forward returns correct char offsets for known
;;; patterns and returns #f for patterns not present in the buffer.

(define *search-offset* #f)

(describe-group "Buffer search"
  (lambda ()
    (it-test "setup search buffer"
      (lambda ()
        (create-buffer "*test-search*")))

    (it-test "insert multi-line text with patterns"
      (lambda ()
        (buffer-insert "the quick brown fox\njumps over the lazy dog\nfoo bar baz\n")))

    (it-test "verify buffer content"
      (lambda ()
        (should-contain (buffer-string) "quick")))

    (it-test "search for 'quick' from start"
      (lambda ()
        (goto-char 0)))

    (it-test "search-forward returns an offset"
      (lambda ()
        (set! *search-offset* (buffer-search-forward "quick"))
        (should *search-offset*)))

    (it-test "offset for 'quick' is correct (position 4)"
      (lambda ()
        (should-equal *search-offset* 4)))

    (it-test "search for 'fox' returns an offset"
      (lambda ()
        (set! *search-offset* (buffer-search-forward "fox"))
        (should *search-offset*)))

    (it-test "offset for 'fox' is after 'quick brown ' (position 16)"
      (lambda ()
        (should-equal *search-offset* 16)))

    (it-test "search for 'jumps' returns an offset"
      (lambda ()
        (set! *search-offset* (buffer-search-forward "jumps"))
        (should *search-offset*)))

    (it-test "'jumps' is on the second line (offset 20)"
      (lambda ()
        (should-equal *search-offset* 20)))

    (it-test "search for nonexistent pattern returns false"
      (lambda ()
        (set! *search-offset* (buffer-search-forward "nonexistent-pattern-xyz"))
        (should-not *search-offset*)))

    (it-test "search for 'baz' near end returns an offset"
      (lambda ()
        (set! *search-offset* (buffer-search-forward "baz"))
        (should *search-offset*)))))
