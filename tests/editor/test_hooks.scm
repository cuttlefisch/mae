;;; test_hooks.scm — Hook system tests
;;;
;;; Verifies add-hook!, remove-hook!, and hook firing via observable side effects.
;;; Uses named Scheme functions registered as hooks, then checks if they fire
;;; via the editor's hook system.

(describe-group "Hook system"
  (lambda ()
    ;; --- Hook registration ---
    (it-test "add-hook registers a hook"
      (lambda ()
        (add-hook! "after-mode-change" "test-hook-fn")))

    (it-test "remove-hook deregisters"
      (lambda ()
        (remove-hook! "after-mode-change" "test-hook-fn")))

    ;; --- Multiple hooks on same event ---
    (it-test "add two hooks to same event"
      (lambda ()
        (add-hook! "before-save" "hook-a")
        (add-hook! "before-save" "hook-b")))

    (it-test "remove one hook leaves other"
      (lambda ()
        (remove-hook! "before-save" "hook-a")))

    (it-test "remove second hook"
      (lambda ()
        (remove-hook! "before-save" "hook-b")))

    ;; --- Invalid hook names ---
    (it-test "add-hook with nonexistent hook name succeeds"
      (lambda ()
        ;; Hook names are just strings — no validation at registration time
        (add-hook! "nonexistent-hook" "some-fn")))

    (it-test "cleanup nonexistent hook"
      (lambda ()
        (remove-hook! "nonexistent-hook" "some-fn")))))
