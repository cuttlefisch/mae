;;; test_search.scm — KB full-text search
;;;
;;; Verifies that FTS5 search finds nodes by title and body content.

(describe-group "KB full-text search"
  (lambda ()
    (it-test "create searchable nodes"
      (lambda ()
        (kb-create "search-a" "Quantum Entanglement" "concept"
          "Spooky action at a distance describes quantum correlations.")
        (kb-create "search-b" "Classical Mechanics" "concept"
          "Newton's laws describe the motion of macroscopic objects.")))

    (it-test "search by title keyword"
      (lambda ()
        (should-contain (kb-search "quantum") "search-a")))

    (it-test "search by body keyword"
      (lambda ()
        (should-contain (kb-search "Newton") "search-b")))

    (it-test "search excludes non-matching"
      (lambda ()
        (should-not-contain (kb-search "quantum") "search-b")))

    (it-test "cleanup"
      (lambda ()
        (kb-delete "search-a")
        (kb-delete "search-b")))))
