;;; test_state_vector.scm — Incremental sync via state-vector / diff
;;;
;;; A inserts text and seeds B with a full state snapshot. A then
;;; inserts more text that B has not seen. B requests a state vector,
;;; A computes a diff from that vector, and B applies the diff.

(define *sv-state-a-initial* #f)
(define *sv-state-vector-b* #f)
(define *sv-diff-from-b* #f)

(describe-group "Incremental sync via state vector"
  (lambda ()
    (it-test "setup A and B with shared initial state"
      (lambda ()
        (create-buffer "*sv-a*")
        (buffer-enable-sync 1)
        (buffer-insert "paragraph one")
        (should-equal (buffer-string) "paragraph one")
        (set! *sv-state-a-initial* (buffer-encode-state))
        (should *sv-state-a-initial*)
        (create-buffer "*sv-b*")
        (buffer-load-sync-state *sv-state-a-initial* 2)
        (should-equal (buffer-string) "paragraph one")))

    (it-test "A inserts additional content"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-a*"))
        (goto-char 13)
        (buffer-insert " paragraph two")
        (should-equal (buffer-string) "paragraph one paragraph two")))

    (it-test "B computes state vector and A computes diff"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-b*"))
        (buffer-encode-state-vector)
        (set! *sv-state-vector-b* (buffer-get-state-vector))
        (should *sv-state-vector-b*)
        (switch-to-buffer (get-buffer-by-name "*sv-a*"))
        (buffer-compute-diff *sv-state-vector-b*)
        (set! *sv-diff-from-b* (buffer-get-diff))
        (should *sv-diff-from-b*)))

    (it-test "B applies diff and converges with A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-b*"))
        (buffer-apply-update "*sv-b*" *sv-diff-from-b*)
        (should-equal (buffer-text "*sv-b*") "paragraph one paragraph two")
        (should-equal (buffer-text "*sv-a*") (buffer-text "*sv-b*"))))))
