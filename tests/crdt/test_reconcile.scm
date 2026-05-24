;;; test_reconcile.scm — buffer-reconcile-to test
;;;
;;; Creates a sync-enabled buffer, inserts initial text, then calls
;;; buffer-reconcile-to with a different target string. Verifies that:
;;;   1. The buffer content matches the target after reconciliation.
;;;   2. A CRDT update was generated (non-empty base64).
;;;   3. The update is well-formed and can be applied to a peer.

(define *reconcile-update* #f)
(define *reconcile-state* #f)

(describe-group "buffer-reconcile-to generates CRDT update"
  (lambda ()
    (it-test "setup reconcile buffer"
      (lambda ()
        (create-buffer "*reconcile-test*")))

    (it-test "enable sync (client 1)"
      (lambda ()
        (buffer-enable-sync 1)))

    (it-test "insert initial text"
      (lambda ()
        (buffer-insert "the quick brown fox")))

    (it-test "buffer has correct initial content"
      (lambda ()
        (should-equal (buffer-string) "the quick brown fox")))

    ;; Save state before reconcile for seeding the peer later
    (it-test "encode state before reconcile"
      (lambda ()
        (set! *reconcile-state* (buffer-encode-state))
        (should *reconcile-state*)))

    (it-test "request reconcile to target text"
      (lambda ()
        (buffer-reconcile-to "the slow brown fox jumps")))

    (it-test "retrieve reconcile result"
      (lambda ()
        (set! *reconcile-update* (buffer-get-reconcile-result))
        (should *reconcile-update*)))

    (it-test "buffer content matches reconcile target"
      (lambda ()
        (should-equal (buffer-string) "the slow brown fox jumps")))

    (it-test "reconcile produced a non-empty CRDT update"
      (lambda ()
        (should (> (string-length *reconcile-update*) 0))))

    ;; Create a peer seeded from the pre-reconcile state
    (it-test "create peer buffer"
      (lambda ()
        (create-buffer "*reconcile-peer*")))

    (it-test "seed peer from pre-reconcile state"
      (lambda ()
        (buffer-load-sync-state *reconcile-state* 2)))

    (it-test "peer has original text"
      (lambda ()
        (should-equal (buffer-string) "the quick brown fox")))

    (it-test "apply reconcile update to peer"
      (lambda ()
        (buffer-apply-update "*reconcile-peer*" *reconcile-update*)))

    (it-test "peer content matches reconcile target"
      (lambda ()
        (should-equal (buffer-text "*reconcile-peer*")
                      "the slow brown fox jumps")))))
