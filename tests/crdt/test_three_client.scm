;;; test_three_client.scm — Three-client CRDT convergence test
;;;
;;; Three buffers A, B, C are seeded from the same initial state and
;;; each edit independently. All updates are exchanged and all three
;;; must converge to byte-identical content.

(define *three-state-a* #f)
(define *three-updates-a* (list))
(define *three-updates-b* (list))
(define *three-updates-c* (list))

(describe-group "Three-client CRDT convergence"
  (lambda ()
    ;; --- Seed buffer A ---
    (it-test "setup buffer A"
      (lambda ()
        (create-buffer "*three-a*")))

    (it-test "enable sync on A (client 1)"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "insert shared initial text into A"
      (lambda ()
        (buffer-insert "shared")))

    (it-test "A has correct initial content"
      (lambda ()
        (should-equal (buffer-string) "shared")))

    (it-test "encode A's state for seeding B and C"
      (lambda ()
        (set! *three-state-a* (buffer-encode-state))
        (should *three-state-a*)))

    ;; --- Seed buffer B ---
    (it-test "setup buffer B"
      (lambda ()
        (create-buffer "*three-b*")))

    (it-test "load A's state into B (client 2)"
      (lambda ()
        (buffer-load-sync-state *three-state-a* 2)))

    (it-test "B has the shared initial content"
      (lambda ()
        (should-equal (buffer-string) "shared")))

    ;; --- Seed buffer C ---
    (it-test "setup buffer C"
      (lambda ()
        (create-buffer "*three-c*")))

    (it-test "load A's state into C (client 3)"
      (lambda ()
        (buffer-load-sync-state *three-state-a* 3)))

    (it-test "C has the shared initial content"
      (lambda ()
        (should-equal (buffer-string) "shared")))

    ;; --- Independent edits ---
    (it-test "switch to A for independent edit"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*three-a*"))))

    (it-test "A moves to end"
      (lambda ()
        (goto-char 6)))

    (it-test "A inserts its tag"
      (lambda ()
        (buffer-insert "-editA")))

    ;; Drain A's updates (two-step)
    (it-test "request drain of A's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve A's updates"
      (lambda ()
        (set! *three-updates-a* (buffer-drain-updates))
        (should (> (length *three-updates-a*) 0))))

    (it-test "switch to B for independent edit"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*three-b*"))))

    (it-test "B moves to end"
      (lambda ()
        (goto-char 6)))

    (it-test "B inserts its tag"
      (lambda ()
        (buffer-insert "-editB")))

    ;; Drain B's updates (two-step)
    (it-test "request drain of B's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve B's updates"
      (lambda ()
        (set! *three-updates-b* (buffer-drain-updates))
        (should (> (length *three-updates-b*) 0))))

    (it-test "switch to C for independent edit"
      (lambda ()
        (switch-to-buffer (get-buffer-by-name "*three-c*"))))

    (it-test "C moves to end"
      (lambda ()
        (goto-char 6)))

    (it-test "C inserts its tag"
      (lambda ()
        (buffer-insert "-editC")))

    ;; Drain C's updates (two-step)
    (it-test "request drain of C's updates"
      (lambda ()
        (buffer-drain-updates)))

    (it-test "retrieve C's updates"
      (lambda ()
        (set! *three-updates-c* (buffer-drain-updates))
        (should (> (length *three-updates-c*) 0))))

    ;; --- Exchange all updates ---
    (it-test "apply B's updates to A"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-b* "*three-a*")))

    (it-test "apply C's updates to A"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-c* "*three-a*")))

    (it-test "apply A's updates to B"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-a* "*three-b*")))

    (it-test "apply C's updates to B"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-c* "*three-b*")))

    (it-test "apply A's updates to C"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-a* "*three-c*")))

    (it-test "apply B's updates to C"
      (lambda ()
        (define (apply-all lst buf)
          (if (null? lst) #t
              (begin
                (buffer-apply-update buf (car lst))
                (apply-all (cdr lst) buf))))
        (apply-all *three-updates-b* "*three-c*")))

    ;; --- Convergence assertions ---
    (it-test "A and B have identical content"
      (lambda ()
        (should-equal (buffer-text "*three-a*")
                      (buffer-text "*three-b*"))))

    (it-test "A and C have identical content"
      (lambda ()
        (should-equal (buffer-text "*three-a*")
                      (buffer-text "*three-c*"))))

    (it-test "converged content contains all three edits"
      (lambda ()
        (let ((content (buffer-text "*three-a*")))
          (should-contain content "editA")
          (should-contain content "editB")
          (should-contain content "editC"))))))
