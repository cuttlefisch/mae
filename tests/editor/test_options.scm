;;; test_options.scm — Option registry get/set operations
;;;
;;; Verifies that set-option! and get-option round-trip correctly for
;;; several editor options, and that defaults are readable.

(describe-group "Options"
  (lambda ()
    (it-test "line_numbers has a default value"
      (lambda ()
        (should (get-option "line_numbers"))))

    (it-test "line_numbers default is true"
      (lambda ()
        (should-equal (get-option "line_numbers") "true")))

    (it-test "set line_numbers to false"
      (lambda ()
        (set-option! "line_numbers" "false")))

    (it-test "line_numbers reads back as false"
      (lambda ()
        (should-equal (get-option "line_numbers") "false")))

    (it-test "set line_numbers back to true"
      (lambda ()
        (set-option! "line_numbers" "true")))

    (it-test "line_numbers reads back as true"
      (lambda ()
        (should-equal (get-option "line_numbers") "true")))

    (it-test "word_wrap option is readable"
      (lambda ()
        (should (get-option "word_wrap"))))

    (it-test "word_wrap default is false"
      (lambda ()
        (should-equal (get-option "word_wrap") "false")))

    (it-test "set word_wrap to true"
      (lambda ()
        (set-option! "word_wrap" "true")))

    (it-test "word_wrap reads back as true"
      (lambda ()
        (should-equal (get-option "word_wrap") "true")))

    (it-test "set word_wrap back to false"
      (lambda ()
        (set-option! "word_wrap" "false")))

    (it-test "nonexistent option returns false"
      (lambda ()
        (should-not (get-option "nonexistent_option_xyz"))))))
