;;; test_kb_search.scm — KB search sort option round-trip
;;;
;;; Verifies that the kb_search_sort option can be set and read back.
;;; Body content matching is covered by Rust unit tests.

(describe-group "KB search sort option"
  (lambda ()
    (it-test "kb_search_sort default is relevance"
      (lambda ()
        (should-equal (get-option "kb_search_sort") "relevance")))

    (it-test "set kb_search_sort to activity"
      (lambda ()
        (set-option! "kb_search_sort" "activity")))

    (it-test "verify activity"
      (lambda ()
        (should-equal (get-option "kb_search_sort") "activity")))

    (it-test "set kb_search_sort to alphabetical"
      (lambda ()
        (set-option! "kb_search_sort" "alphabetical")))

    (it-test "verify alphabetical"
      (lambda ()
        (should-equal (get-option "kb_search_sort") "alphabetical")))

    (it-test "set kb_search_sort back to relevance"
      (lambda ()
        (set-option! "kb_search_sort" "relevance")))

    (it-test "verify relevance"
      (lambda ()
        (should-equal (get-option "kb_search_sort") "relevance")))))
