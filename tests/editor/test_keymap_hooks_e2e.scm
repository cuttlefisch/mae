;;; E2E: keypad + flavor lifecycle HOOKS fire (user-extensibility).
;;;
;;; Validates that user code can react to the transient leader keypad and to
;;; live flavor switches via hooks — the extensibility contract for building on
;;; the which-key/flavor pipeline without patching the kernel:
;;;   leader-open      — keypad activated
;;;   leader-execute   — a command resolved FROM the keypad (keypad-specific;
;;;                      the generic command-post also fires)
;;;   leader-cancel    — keypad dismissed (Esc / C-g / unbound key)
;;;   keymap-flavor-changed — live flavor switch settled
;;;
;;; Hook fns + flags are top-level (global) so the registered named functions
;;; can mutate observable state across test steps.

(define %leader-open-fired #f)
(define %leader-exec-fired #f)
(define %leader-cancel-fired #f)
(define %flavor-changed-fired #f)
(define (on-leader-open) (set! %leader-open-fired #t))
(define (on-leader-exec) (set! %leader-exec-fired #t))
(define (on-leader-cancel) (set! %leader-cancel-fired #t))
(define (on-flavor-changed) (set! %flavor-changed-fired #t))

(describe-group "keypad + flavor lifecycle hooks (user-extensibility)"
  (lambda ()
    (it-test "register lifecycle hooks"
      (lambda ()
        (add-hook! "leader-open" "on-leader-open")
        (add-hook! "leader-execute" "on-leader-exec")
        (add-hook! "leader-cancel" "on-leader-cancel")
        (add-hook! "keymap-flavor-changed" "on-flavor-changed")))
    (it-test "live flavor switch fires keymap-flavor-changed"
      (lambda () (execute-ex "keymap-set-flavor doom")))
    (it-test "flavor-changed hook fired"
      (lambda () (should %flavor-changed-fired)))
    (it-test "editable buffer"
      (lambda () (create-buffer "*hooks-e2e*")))
    (it-test "open keypad and resolve a command (SPC t l)"
      (lambda () (feed-keys "SPC t l")))
    (it-test "leader-open hook fired on activation"
      (lambda () (should %leader-open-fired)))
    (it-test "leader-execute hook fired on resolution"
      (lambda () (should %leader-exec-fired)))
    (it-test "cancel hasn't fired yet"
      (lambda () (should (not %leader-cancel-fired))))
    (it-test "open keypad again"
      (lambda () (feed-keys "SPC")))
    (it-test "cancel with escape"
      (lambda () (feed-keys "escape")))
    (it-test "leader-cancel hook fired"
      (lambda () (should %leader-cancel-fired)))))
