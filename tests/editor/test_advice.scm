;;; test_advice.scm — Advice system tests
;;;
;;; Verifies advice-add! and advice-remove! for command advice.
;;; The advice system allows wrapping commands with before/after behavior.

(describe-group "Advice system"
  (lambda ()
    ;; --- Basic advice add/remove ---
    (it-test "advice-add! before advice"
      (lambda ()
        (advice-add! "save" "before" "my-before-save")))

    (it-test "advice-add! after advice"
      (lambda ()
        (advice-add! "save" "after" "my-after-save")))

    (it-test "advice-remove! before advice"
      (lambda ()
        (advice-remove! "save" "my-before-save")))

    (it-test "advice-remove! after advice"
      (lambda ()
        (advice-remove! "save" "my-after-save")))

    ;; --- Multiple advice on same command ---
    (it-test "add multiple advice functions"
      (lambda ()
        (advice-add! "delete-line" "before" "advice-fn-1")
        (advice-add! "delete-line" "after" "advice-fn-2")))

    (it-test "remove all advice cleanly"
      (lambda ()
        (advice-remove! "delete-line" "advice-fn-1")
        (advice-remove! "delete-line" "advice-fn-2")))))
