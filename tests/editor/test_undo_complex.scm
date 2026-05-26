;;; test_undo_complex.scm — Multi-step undo/redo with delete interleaved
;;;
;;; Verifies that undo walks back a sequence of inserts one step at a time,
;;; that redo replays them, and that a subsequent delete followed by undo
;;; restores the deleted content.

(describe-group "Complex undo/redo"
  (lambda ()
    (it-test "multi-step undo and redo"
      (lambda ()
        (create-buffer "*test-undo-complex*")
        (buffer-insert "aaa")
        (should-equal (buffer-string) "aaa")
        (buffer-insert "bbb")
        (should-equal (buffer-string) "aaabbb")
        (buffer-insert "ccc")
        (should-equal (buffer-string) "aaabbbccc")
        ;; Undo last two inserts
        (buffer-undo)
        (should-equal (buffer-string) "aaabbb")
        (buffer-undo)
        (should-equal (buffer-string) "aaa")
        ;; Redo both
        (buffer-redo)
        (should-equal (buffer-string) "aaabbb")
        (buffer-redo)
        (should-equal (buffer-string) "aaabbbccc")))

    (it-test "delete then undo restores content"
      (lambda ()
        (buffer-delete-range 3 6)
        (should-equal (buffer-string) "aaaccc")
        (buffer-undo)
        (should-equal (buffer-string) "aaabbbccc")))))
