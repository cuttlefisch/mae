;;; test_three_client.scm — Three-client CRDT convergence test
;;;
;;; Three buffers A, B, C are seeded from the same initial state and
;;; each edit independently. All updates are exchanged and all three
;;; must converge to byte-identical content.

(define *three-state-a* #f)
(define *three-updates-a* (list))
(define *three-updates-b* (list))
(define *three-updates-c* (list))

(define (apply-all-updates lst buf)
  (if (null? lst) #t
      (begin
        (buffer-apply-update buf (car lst))
        (apply-all-updates (cdr lst) buf))))

(describe-group "Three-client CRDT convergence"
  (lambda ()
    (it-test "setup A, B, C with shared initial state"
      (lambda ()
        (create-buffer "*three-a*")
        (buffer-enable-sync 1)
        (buffer-insert "shared")
        (should-equal (buffer-string) "shared")
        (set! *three-state-a* (buffer-encode-state))
        (should *three-state-a*)
        (create-buffer "*three-b*")
        (buffer-load-sync-state *three-state-a* 2)
        (should-equal (buffer-string) "shared")
        (create-buffer "*three-c*")
        (buffer-load-sync-state *three-state-a* 3)
        (should-equal (buffer-string) "shared")))

    (it-test "each client edits independently"
      (lambda ()
        ;; A edits
        (switch-to-buffer (get-buffer-by-name "*three-a*"))
        (goto-char 6)
        (buffer-insert "-editA")
        (set! *three-updates-a* (buffer-drain-updates))
        (should (> (length *three-updates-a*) 0))
        ;; B edits
        (switch-to-buffer (get-buffer-by-name "*three-b*"))
        (goto-char 6)
        (buffer-insert "-editB")
        (set! *three-updates-b* (buffer-drain-updates))
        (should (> (length *three-updates-b*) 0))
        ;; C edits
        (switch-to-buffer (get-buffer-by-name "*three-c*"))
        (goto-char 6)
        (buffer-insert "-editC")
        (set! *three-updates-c* (buffer-drain-updates))
        (should (> (length *three-updates-c*) 0))))

    (it-test "exchange all updates and verify convergence"
      (lambda ()
        ;; A receives B and C
        (apply-all-updates *three-updates-b* "*three-a*")
        (apply-all-updates *three-updates-c* "*three-a*")
        ;; B receives A and C
        (apply-all-updates *three-updates-a* "*three-b*")
        (apply-all-updates *three-updates-c* "*three-b*")
        ;; C receives A and B
        (apply-all-updates *three-updates-a* "*three-c*")
        (apply-all-updates *three-updates-b* "*three-c*")
        ;; Convergence
        (should-equal (buffer-text "*three-a*")
                      (buffer-text "*three-b*"))
        (should-equal (buffer-text "*three-a*")
                      (buffer-text "*three-c*"))
        (let ((content (buffer-text "*three-a*")))
          (should-contain content "editA")
          (should-contain content "editB")
          (should-contain content "editC"))))))
