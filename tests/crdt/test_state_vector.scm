;;; test_state_vector.scm — Incremental sync via state-vector / diff
;;;
;;; A inserts text and seeds B with a full state snapshot. A then
;;; inserts more text that B has not seen. B requests a state vector,
;;; A computes a diff from that vector, and B applies the diff.
;;; The test verifies that B ends up with A's complete content
;;; without needing a second full-state transfer.
;;;
;;; Primitives used:
;;;   buffer-encode-state-vector  — request SV encoding (async; result
;;;                                  available on next step via
;;;                                  buffer-get-state-vector)
;;;   buffer-get-state-vector     — retrieve the encoded SV (b64 string)
;;;   buffer-compute-diff SV-B64  — request diff from SV (async; result
;;;                                  available via buffer-get-diff)
;;;   buffer-get-diff             — retrieve the encoded diff (b64 string)

(define *sv-state-a-initial* #f)
(define *sv-state-vector-b* #f)
(define *sv-diff-from-b* #f)

(describe-group "Incremental sync via state vector"
  (lambda ()
    ;; --- A writes initial content and seeds B ---
    (it-test "setup buffer A"
      (lambda ()
        (create-buffer "*sv-a*")))

    (it-test "enable sync on A (client 1)"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "A inserts first paragraph"
      (lambda ()
        (buffer-insert "paragraph one")))

    (it-test "A has correct initial content"
      (lambda ()
        (should-equal (buffer-string) "paragraph one")))

    (it-test "encode A's state for seeding B"
      (lambda ()
        (set! *sv-state-a-initial* (buffer-encode-state))
        (should *sv-state-a-initial*)))

    (it-test "setup buffer B"
      (lambda ()
        (create-buffer "*sv-b*")))

    (it-test "load A's state into B (client 2)"
      (lambda ()
        (buffer-load-sync-state *sv-state-a-initial* 2)))

    (it-test "B has A's initial content"
      (lambda ()
        (should-equal (buffer-string) "paragraph one")))

    ;; --- A inserts additional content that B has not seen ---
    (it-test "switch back to A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-a*"))))

    (it-test "A moves cursor to end"
      (lambda ()
        (goto-char 13)))

    (it-test "A inserts second paragraph"
      (lambda ()
        (buffer-insert " paragraph two")))

    (it-test "A has both paragraphs"
      (lambda ()
        (should-equal (buffer-string) "paragraph one paragraph two")))

    ;; --- B computes its state vector ---
    (it-test "switch to B"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-b*"))))

    (it-test "B requests its state vector encoding"
      (lambda ()
        (buffer-encode-state-vector)))

    (it-test "retrieve B's state vector"
      (lambda ()
        (set! *sv-state-vector-b* (buffer-get-state-vector))
        (should *sv-state-vector-b*)))

    ;; --- A computes the diff relative to B's state vector ---
    (it-test "switch to A to compute diff"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-a*"))))

    (it-test "A requests diff from B's state vector"
      (lambda ()
        (buffer-compute-diff *sv-state-vector-b*)))

    (it-test "retrieve diff from A"
      (lambda ()
        (set! *sv-diff-from-b* (buffer-get-diff))
        (should *sv-diff-from-b*)))

    ;; --- B applies the incremental diff ---
    (it-test "switch to B to apply diff"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*sv-b*"))))

    (it-test "B applies incremental diff from A"
      (lambda ()
        (buffer-apply-update "*sv-b*" *sv-diff-from-b*)))

    ;; --- Verify convergence ---
    (it-test "B now has A's full content"
      (lambda ()
        (should-equal (buffer-text "*sv-b*") "paragraph one paragraph two")))

    (it-test "A and B have identical content"
      (lambda ()
        (should-equal (buffer-text "*sv-a*")
                      (buffer-text "*sv-b*"))))))
