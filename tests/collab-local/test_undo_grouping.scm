;;; test_undo_grouping.scm — CRDT undo must respect undo groups
;;;
;;; When sync is enabled, multiple sequential inserts without an explicit
;;; undo boundary should merge into one undo item.  This mirrors vim's
;;; insert-mode behavior where typing "hello" then pressing Esc undoes
;;; all five characters at once.
;;;
;;; The test runner does NOT call undo_reset() between test steps, so
;;; with capture_timeout_millis = u64::MAX all inserts merge.

(describe-group "CRDT undo grouping"
  (lambda ()
    (it-test "setup synced buffer"
      (lambda ()
        (create-buffer "*undo-group-test*")
        (buffer-enable-sync 1)))

    ;; Simulate typing "hello" as individual inserts (each is a
    ;; separate yrs transaction via insert_text_at).
    (it-test "insert h"
      (lambda ()
        (buffer-insert "h")))
    (it-test "insert e"
      (lambda ()
        (buffer-insert "e")))
    (it-test "insert l"
      (lambda ()
        (buffer-insert "l")))
    (it-test "insert l2"
      (lambda ()
        (buffer-insert "l")))
    (it-test "insert o"
      (lambda ()
        (buffer-insert "o")))

    (it-test "buffer has hello"
      (lambda ()
        (should-equal (buffer-string) "hello")))

    ;; A single undo should revert ALL five inserts because they're
    ;; in the same undo group (no undo_reset between them).
    (it-test "single undo reverts entire group"
      (lambda ()
        (buffer-undo)))

    (it-test "buffer is empty after one undo"
      (lambda ()
        (should-equal (buffer-string) "")))

    (it-test "sync content matches after undo"
      (lambda ()
        (should-equal (buffer-sync-content) "")))))
