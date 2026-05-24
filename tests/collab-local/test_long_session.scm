;;; test_long_session.scm — Long-lived editing session simulation
;;;
;;; Simulates a realistic editing session: two buffers (peers) sharing
;;; state, making interleaved edits, undoing, and verifying convergence
;;; after each round.  This mirrors real user behavior where a session
;;; stays open for extended editing rather than connect-edit-disconnect.
;;;
;;; Covers gaps that transactional Docker E2E tests miss:
;;;   - State accumulation over many edit rounds
;;;   - Undo/redo interleaved with remote updates
;;;   - Convergence after asymmetric edit volumes
;;;   - Buffer content integrity after many operations

(define *session-state* #f)
(define *a-updates* (list))
(define *b-updates* (list))

;; Helper: apply a list of base64 updates to a named buffer.
(define (apply-updates-to buf-name updates)
  (if (null? updates) #t
      (begin
        (buffer-apply-update buf-name (car updates))
        (apply-updates-to buf-name (cdr updates)))))

(describe-group "Long-lived editing session"
  (lambda ()

    ;; === SETUP: Create two synced peers ===

    (it-test "create peer A"
      (lambda ()
        (create-buffer "*session-a*")
        (buffer-enable-sync 1)))

    (it-test "A writes initial content"
      (lambda ()
        (buffer-insert "# Session Notes\n\n")))

    (it-test "undo boundary after initial content"
      (lambda ()
        (buffer-undo-boundary)))

    (it-test "encode A state"
      (lambda ()
        (set! *session-state* (buffer-encode-state))
        (should *session-state*)))

    (it-test "create peer B from A's state"
      (lambda ()
        (create-buffer "*session-b*")
        (buffer-load-sync-state *session-state* 2)))

    (it-test "B has A's content"
      (lambda ()
        (should-equal (buffer-string) "# Session Notes\n\n")))

    ;; === ROUND 1: Both peers add content ===

    ;; A adds a paragraph
    (it-test "switch to A"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-a*"))))

    (it-test "A moves to end"
      (lambda ()
        (goto-char 18)))

    (it-test "A adds paragraph 1"
      (lambda ()
        (buffer-insert "## Tasks\n- Fix undo grouping\n")))

    (it-test "drain A round 1"
      (lambda ()
        (set! *a-updates* (buffer-drain-updates))
        (should (> (length *a-updates*) 0))))

    ;; B adds a paragraph (before seeing A's edit)
    (it-test "switch to B"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-b*"))))

    (it-test "B moves to end"
      (lambda ()
        (goto-char 18)))

    (it-test "B adds paragraph"
      (lambda ()
        (buffer-insert "## Notes\n- Session started\n")))

    (it-test "drain B round 1"
      (lambda ()
        (set! *b-updates* (buffer-drain-updates))
        (should (> (length *b-updates*) 0))))

    ;; Exchange round 1 updates
    (it-test "apply B's updates to A"
      (lambda ()
        (apply-updates-to "*session-a*" *b-updates*)))

    (it-test "apply A's updates to B"
      (lambda ()
        (apply-updates-to "*session-b*" *a-updates*)))

    ;; Convergence check round 1
    (it-test "round 1: A and B converge"
      (lambda ()
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))))

    (it-test "round 1: content has both sections"
      (lambda ()
        (should-contain (buffer-text "*session-a*") "Tasks")
        (should-contain (buffer-text "*session-a*") "Notes")))

    ;; === ROUND 2: A undoes, B keeps editing ===

    (it-test "switch to A for undo"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-a*"))))

    (it-test "undo boundary before undo"
      (lambda ()
        (buffer-undo-boundary)))

    (it-test "A undoes its paragraph"
      (lambda ()
        (buffer-undo)))

    (it-test "A no longer has Tasks"
      (lambda ()
        (should-not (string-contains? (buffer-string) "Tasks"))))

    (it-test "A still has Notes (B's edit)"
      (lambda ()
        (should-contain (buffer-string) "Notes")))

    (it-test "drain A undo updates"
      (lambda ()
        (set! *a-updates* (buffer-drain-updates))))

    ;; B adds more content
    (it-test "switch to B for more edits"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-b*"))))

    (it-test "B adds another note"
      (lambda ()
        (let ((len (string-length (buffer-string))))
          (goto-char len)
          (buffer-insert "- Undo grouping fixed\n"))))

    (it-test "drain B round 2"
      (lambda ()
        (set! *b-updates* (buffer-drain-updates))))

    ;; Exchange round 2 updates
    (it-test "apply A undo to B"
      (lambda ()
        (apply-updates-to "*session-b*" *a-updates*)))

    (it-test "apply B edits to A"
      (lambda ()
        (apply-updates-to "*session-a*" *b-updates*)))

    ;; Convergence check round 2
    (it-test "round 2: A and B converge"
      (lambda ()
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))))

    (it-test "round 2: no Tasks (A undid it)"
      (lambda ()
        (should-not (string-contains? (buffer-text "*session-a*") "Tasks"))))

    (it-test "round 2: has both Notes entries"
      (lambda ()
        (should-contain (buffer-text "*session-a*") "Session started")
        (should-contain (buffer-text "*session-a*") "Undo grouping fixed")))

    ;; === ROUND 3: A redoes, verify final convergence ===

    (it-test "switch to A for redo"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-a*"))))

    (it-test "A redoes its paragraph"
      (lambda ()
        (buffer-redo)))

    (it-test "A has Tasks again"
      (lambda ()
        (should-contain (buffer-string) "Tasks")))

    (it-test "drain A redo updates"
      (lambda ()
        (set! *a-updates* (buffer-drain-updates))))

    (it-test "apply A redo to B"
      (lambda ()
        (apply-updates-to "*session-b*" *a-updates*)))

    ;; Final convergence
    (it-test "final: A and B converge"
      (lambda ()
        (should-equal (buffer-text "*session-a*")
                      (buffer-text "*session-b*"))))

    (it-test "final: sync content matches buffer"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*session-a*"))
        (should-equal (buffer-sync-content) (buffer-string))))))
