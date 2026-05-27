;;; test_reconcile.scm — buffer-reconcile-to test
;;;
;;; Creates a sync-enabled buffer, inserts initial text, then calls
;;; buffer-reconcile-to with a different target string. Verifies that
;;; the buffer converges and the update applies to a peer.

(define *reconcile-update* #f)
(define *reconcile-state* #f)

(describe-group "buffer-reconcile-to generates CRDT update"
  (lambda ()
    (it-test "reconcile changes buffer content and produces update"
      (lambda ()
        (create-buffer "*reconcile-test*")
        (buffer-enable-sync 1)
        (buffer-insert "the quick brown fox")
        (should-equal (buffer-string) "the quick brown fox")
        (set! *reconcile-state* (buffer-encode-state))
        (should *reconcile-state*)
        (buffer-reconcile-to "the slow brown fox jumps")
        (set! *reconcile-update* (buffer-get-reconcile-result))
        (should *reconcile-update*)
        (should-equal (buffer-string) "the slow brown fox jumps")
        (should (> (string-length *reconcile-update*) 0))))

    (it-test "reconcile update applies to peer"
      (lambda ()
        (create-buffer "*reconcile-peer*")
        (buffer-load-sync-state *reconcile-state* 2)
        (should-equal (buffer-string) "the quick brown fox")
        (buffer-apply-update "*reconcile-peer*" *reconcile-update*)
        (should-equal (buffer-text "*reconcile-peer*")
                      "the slow brown fox jumps")))))
