;;; test_options.scm — Option registry get/set operations
;;;
;;; Verifies that set-option! and get-option round-trip correctly for
;;; several editor options, and that defaults are readable.

(describe-group "Options"
  (lambda ()
    (it-test "line_numbers option round-trip"
      (lambda ()
        (set-option! "line_numbers" "true")
        (should-equal (get-option "line_numbers") "true")
        (set-option! "line_numbers" "false")
        (should-equal (get-option "line_numbers") "false")
        (set-option! "line_numbers" "true")
        (should-equal (get-option "line_numbers") "true")))

    (it-test "word_wrap option round-trip"
      (lambda ()
        (should (get-option "word_wrap"))
        (should-equal (get-option "word_wrap") "false")
        (set-option! "word_wrap" "true")
        (should-equal (get-option "word_wrap") "true")
        (set-option! "word_wrap" "false")))

    (it-test "nonexistent option returns false"
      (lambda ()
        (should-not (get-option "nonexistent_option_xyz"))))))
