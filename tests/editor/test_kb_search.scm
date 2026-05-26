;;; test_kb_search.scm — KB search sort option round-trip
;;;
;;; Verifies that the kb_search_sort option can be set and read back.

(describe-group "KB search sort option"
  (lambda ()
    (it-test "kb_search_sort option round-trip"
      (lambda ()
        (should-equal (get-option "kb_search_sort") "relevance")
        (set-option! "kb_search_sort" "activity")
        (should-equal (get-option "kb_search_sort") "activity")
        (set-option! "kb_search_sort" "alphabetical")
        (should-equal (get-option "kb_search_sort") "alphabetical")
        (set-option! "kb_search_sort" "relevance")
        (should-equal (get-option "kb_search_sort") "relevance")))))
