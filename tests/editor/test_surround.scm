;;; test_surround.scm — Vim-surround operations
;;;
;;; Tests surround commands: add, delete, change surrounding delimiters.
;;; Uses the command API since surround commands are await-based
;;; (they wait for a character input).

(describe-group "Surround operations"
  (lambda ()
    (it-test "setup buffer with quoted text"
      (lambda ()
        (create-buffer "*surround-test*")
        (buffer-insert "hello \"world\" end")))

    ;; Verify surround commands are registered
    (it-test "delete-surround command exists"
      (lambda ()
        (should (command-exists? "delete-surround-await"))))

    (it-test "change-surround command exists"
      (lambda ()
        (should (command-exists? "change-surround-await"))))

    (it-test "surround-visual command exists"
      (lambda ()
        (should (command-exists? "surround-visual-await"))))

    (it-test "surround-line command exists"
      (lambda ()
        (should (command-exists? "surround-line-await"))))

    (it-test "operator-surround command exists"
      (lambda ()
        (should (command-exists? "operator-surround"))))))
