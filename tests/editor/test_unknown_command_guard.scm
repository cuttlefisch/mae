;; #787: `(execute-ex ...)` on an unknown command must NOT report `ok`.
;;
;; It is the backbone of this suite — the way a scenario reaches any editor
;; command that has no Scheme primitive — and an unknown name was
;; indistinguishable from success. Found by a real dogfood:
;; `(execute-ex "kb-daily-today")` reported `ok` while doing nothing, because
;; the command is actually `daily-goto-today`. Six steps "passed" that way.
;;
;; This file asserts the POSITIVE half — a real command still dispatches
;; cleanly. The negative half cannot be asserted from inside a scenario (a step
;; that names a bad command must FAIL, and a failing step is not a passing
;; test), so it is covered by `test_runner`'s own logic and verified by
;; falsification instead.

(describe-group "unknown-command guard"
  (lambda ()
    (it-test "a real command dispatches and does not trip the guard"
      (lambda ()
        (execute-ex "nohlsearch")))
    (it-test "a second real command, with an argument"
      (lambda ()
        (execute-ex "set ignorecase true")))
    (it-test "the editor is still usable afterwards"
      (lambda ()
        (create-buffer "*guard-check*")))

    ;; A command that RUNS and then reports failure in its status must not read
    ;; as ok either — the other half of #787, found by a Phase 7 rehearsal where
    ;; `(execute-ex "kb-detach X")` passed while the detach found no such KB.
    ;;
    ;; Only the positive half is assertable from inside a scenario (a step naming
    ;; a failing command must FAIL, and a failing step is not a passing test), so
    ;; this pins that an ordinary informational status still passes — without
    ;; which the guard could fire on innocent messages and get switched off.
    (it-test "an informational status does not trip the failure guard"
      (lambda ()
        (execute-ex "nohlsearch")))))
