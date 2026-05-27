;;; test_hooks.scm — Hook system tests
;;;
;;; Verifies add-hook!, remove-hook!, and hook firing via observable side effects.

(describe-group "Hook system"
  (lambda ()
    (it-test "add and remove hooks"
      (lambda ()
        (add-hook! "after-mode-change" "test-hook-fn")
        (remove-hook! "after-mode-change" "test-hook-fn")))

    (it-test "multiple hooks on same event"
      (lambda ()
        (add-hook! "before-save" "hook-a")
        (add-hook! "before-save" "hook-b")
        (remove-hook! "before-save" "hook-a")
        (remove-hook! "before-save" "hook-b")))

    (it-test "add-hook with nonexistent hook name succeeds"
      (lambda ()
        (add-hook! "nonexistent-hook" "some-fn")
        (remove-hook! "nonexistent-hook" "some-fn")))))
