;;; test_links.scm — KB link graph operations
;;;
;;; Verifies that links between nodes are created from body content
;;; and that backlink queries work.

(describe-group "KB link graph"
  (lambda ()
    (it-test "create source node with links"
      (lambda ()
        (kb-create "link-src" "Source Node" "note"
          "This links to [[id:link-dst][target]] and [[id:link-other][other]].")))

    (it-test "create target nodes"
      (lambda ()
        (kb-create "link-dst" "Target Node" "note" "I am the target.")
        (kb-create "link-other" "Other Node" "note" "I am the other.")))

    (it-test "links-from returns outgoing links"
      (lambda ()
        (define links (kb-links-from "link-src"))
        (should (>= (length links) 1))))

    (it-test "links-to returns backlinks"
      (lambda ()
        (define backlinks (kb-links-to "link-dst"))
        (should (>= (length backlinks) 1))))

    (it-test "cleanup"
      (lambda ()
        (kb-delete "link-src")
        (kb-delete "link-dst")
        (kb-delete "link-other")))))
