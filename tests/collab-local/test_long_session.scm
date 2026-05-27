;;; test_long_session.scm — Long-lived editing session simulation
;;;
;;; Simulates a realistic editing session: two buffers (peers) sharing
;;; state, making interleaved edits, undoing, and verifying convergence
;;; after each round.

(define *session-state* #f)
(define *a-updates* (list))
(define *b-updates* (list))

(define (apply-updates-to buf-name updates)
  (if (null? updates) #t
      (begin
        (buffer-apply-update buf-name (car updates))
        (apply-updates-to buf-name (cdr updates)))))

(describe-group "Long-lived editing session"
  (lambda ()

    (it-test "setup two synced peers"
      (lambda ()
        (create-buffer "*session-a*")
        (buffer-enable-sync 1)
        (buffer-insert "# Session Notes\n\n")
        (buffer-undo-boundary)
        (set! *session-state* (buffer-encode-state))
        (should *session-state*)
        (create-buffer "*session-b*")
        (buffer-load-sync-state *session-state* 2)
        (should-equal (buffer-string) "# Session Notes\n\n")))

    (it-test "round 1: both peers add content"
      (lambda ()
        ;; A adds a paragraph
        (switch-to-buffer (get-buffer-by-name "*session-a*"))
        (goto-char 18)
        (buffer-insert "## Tasks\n- Fix undo grouping\n")
        (set! *a-updates* (buffer-drain-updates))
        (should (> (length *a-updates*) 0))
        ;; B adds a paragraph (before seeing A's edit)
        (switch-to-buffer (get-buffer-by-name "*session-b*"))
        (goto-char 18)
        (buffer-insert "## Notes\n- Session started\n")
        (set! *b-updates* (buffer-drain-updates))
        (should (> (length *b-updates*) 0))
        ;; Exchange updates
        (apply-updates-to "*session-a*" *b-updates*)
        (apply-updates-to "*session-b*" *a-updates*)
        ;; Convergence check
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))
        (should-contain (buffer-text "*session-a*") "Tasks")
        (should-contain (buffer-text "*session-a*") "Notes")))

    (it-test "round 2: A undoes, B keeps editing"
      (lambda ()
        ;; A undoes its paragraph
        (switch-to-buffer (get-buffer-by-name "*session-a*"))
        (buffer-undo-boundary)
        (buffer-undo)
        (should-not (string-contains? (buffer-string) "Tasks"))
        (should-contain (buffer-string) "Notes")
        (set! *a-updates* (buffer-drain-updates))
        ;; B adds more content
        (switch-to-buffer (get-buffer-by-name "*session-b*"))
        (let ((len (string-length (buffer-string))))
          (goto-char len)
          (buffer-insert "- Undo grouping fixed\n"))
        (set! *b-updates* (buffer-drain-updates))
        ;; Exchange
        (apply-updates-to "*session-b*" *a-updates*)
        (apply-updates-to "*session-a*" *b-updates*)
        ;; Convergence
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))
        (should-not (string-contains? (buffer-text "*session-a*") "Tasks"))
        (should-contain (buffer-text "*session-a*") "Session started")
        (should-contain (buffer-text "*session-a*") "Undo grouping fixed")))

    (it-test "round 3: A redoes, final convergence"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-a*"))
        (buffer-redo)
        (should-contain (buffer-string) "Tasks")
        (set! *a-updates* (buffer-drain-updates))
        (apply-updates-to "*session-b*" *a-updates*)
        ;; Final convergence
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))
        (should-equal (buffer-sync-content) (buffer-string))))))
