;;; test_advice.scm — Advice system tests
;;;
;;; Verifies advice-add! and advice-remove! for command advice.

(describe-group "Advice system"
  (lambda ()
    (it-test "add and remove before/after advice"
      (lambda ()
        (advice-add! "save" "before" "my-before-save")
        (advice-add! "save" "after" "my-after-save")
        (advice-remove! "save" "my-before-save")
        (advice-remove! "save" "my-after-save")))

    (it-test "multiple advice on same command"
      (lambda ()
        (advice-add! "delete-line" "before" "advice-fn-1")
        (advice-add! "delete-line" "after" "advice-fn-2")
        (advice-remove! "delete-line" "advice-fn-1")
        (advice-remove! "delete-line" "advice-fn-2")))))
