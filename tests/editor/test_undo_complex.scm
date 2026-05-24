;;; test_undo_complex.scm — Multi-step undo/redo with delete interleaved
;;;
;;; Verifies that undo walks back a sequence of inserts one step at a time,
;;; that redo replays them, and that a subsequent delete followed by undo
;;; restores the deleted content.

(describe-group "Complex undo/redo"
  (lambda ()
    (it-test "setup clean buffer"
      (lambda ()
        (create-buffer "*test-undo-complex*")))

    (it-test "insert 'aaa'"
      (lambda ()
        (buffer-insert "aaa")))

    (it-test "verify 'aaa' in buffer"
      (lambda ()
        (should-equal (buffer-string) "aaa")))

    (it-test "insert 'bbb'"
      (lambda ()
        (buffer-insert "bbb")))

    (it-test "verify 'aaabbb' in buffer"
      (lambda ()
        (should-equal (buffer-string) "aaabbb")))

    (it-test "insert 'ccc'"
      (lambda ()
        (buffer-insert "ccc")))

    (it-test "verify 'aaabbbccc' in buffer"
      (lambda ()
        (should-equal (buffer-string) "aaabbbccc")))

    (it-test "undo last insert"
      (lambda ()
        (buffer-undo)))

    (it-test "buffer is 'aaabbb' after one undo"
      (lambda ()
        (should-equal (buffer-string) "aaabbb")))

    (it-test "undo second insert"
      (lambda ()
        (buffer-undo)))

    (it-test "buffer is 'aaa' after two undos"
      (lambda ()
        (should-equal (buffer-string) "aaa")))

    (it-test "redo restores 'bbb'"
      (lambda ()
        (buffer-redo)))

    (it-test "buffer is 'aaabbb' after one redo"
      (lambda ()
        (should-equal (buffer-string) "aaabbb")))

    (it-test "redo restores 'ccc'"
      (lambda ()
        (buffer-redo)))

    (it-test "buffer is 'aaabbbccc' after two redos"
      (lambda ()
        (should-equal (buffer-string) "aaabbbccc")))

    (it-test "delete range 3-6 (removes 'bbb')"
      (lambda ()
        (buffer-delete-range 3 6)))

    (it-test "buffer is 'aaaccc' after delete"
      (lambda ()
        (should-equal (buffer-string) "aaaccc")))

    (it-test "undo the delete"
      (lambda ()
        (buffer-undo)))

    (it-test "buffer is 'aaabbbccc' after undo of delete"
      (lambda ()
        (should-equal (buffer-string) "aaabbbccc")))))
