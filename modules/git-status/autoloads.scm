;;; git-status/autoloads.scm — Magit-style git status keybindings
;;;
;;; Registers the git-status keymap and SPC g leader bindings.
;;; The git dispatch logic is implemented in the kernel.

;;; @module: git-status
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: git-status-autoloads

;; Create git-status keymap inheriting from normal
(define-keymap "git-status" "normal")

;; Navigation
(define-key "git-status" "j" "move-down")
(define-key "git-status" "k" "move-up")
(define-key "git-status" "n" "git-next-hunk")
(define-key "git-status" "p" "git-prev-hunk")
(define-key "git-status" "G" "move-to-last-line")
(define-key "git-status" "g g" "move-to-first-line")

;; Stage/Unstage
(define-key "git-status" "s" "git-stage")
(define-key "git-status" "u" "git-unstage")
(define-key "git-status" "S" "git-stage-all")
(define-key "git-status" "U" "git-unstage-all")

;; Commit
(define-key "git-status" "c c" "git-commit")
(define-key "git-status" "c a" "git-amend")

;; Log
(define-key "git-status" "l l" "git-log")

;; Discard
(define-key "git-status" "x" "git-discard")

;; Fold/unfold
(define-key "git-status" "Tab" "git-toggle-fold")

;; Open file
(define-key "git-status" "Enter" "git-status-open")

;; Push/Pull/Fetch
(define-key "git-status" "P p" "git-push")
(define-key "git-status" "P u" "git-push")
(define-key "git-status" "f f" "git-fetch")
(define-key "git-status" "F p" "git-pull")
(define-key "git-status" "F u" "git-pull")

;; Branch
(define-key "git-status" "b b" "git-branch-switch")
(define-key "git-status" "b n" "git-branch-create")
(define-key "git-status" "b d" "git-branch-delete")

;; Stash
(define-key "git-status" "z z" "git-stash-push")
(define-key "git-status" "z p" "git-stash-pop")
(define-key "git-status" "z a" "git-stash-apply")
(define-key "git-status" "z d" "git-stash-drop")

;; Misc
(define-key "git-status" "q" "kill-buffer")
(define-key "git-status" "Escape" "kill-buffer")
(define-key "git-status" "?" "show-buffer-keys")
(define-key "git-status" "g r" "git-status")

;; SPC m — mode menu (Doom-style)
(define-key "git-status" "SPC m s" "git-stage")
(define-key "git-status" "SPC m u" "git-unstage")
(define-key "git-status" "SPC m S" "git-stage-all")
(define-key "git-status" "SPC m U" "git-unstage-all")
(define-key "git-status" "SPC m x" "git-discard")
(define-key "git-status" "SPC m c" "git-commit")
(define-key "git-status" "SPC m a" "git-amend")
(define-key "git-status" "SPC m p" "git-push")
(define-key "git-status" "SPC m f" "git-fetch")
(define-key "git-status" "SPC m F" "git-pull")
(define-key "git-status" "SPC m r" "git-status")

;; SPC g leader bindings (normal mode)
(define-key "normal" "SPC g s" "git-status")
(define-key "normal" "SPC g g" "git-status")
(define-key "normal" "SPC g b" "git-blame")
(define-key "normal" "SPC g d" "git-diff")
(define-key "normal" "SPC g l" "git-log")
(define-key "normal" "SPC g c" "git-commit")
(define-key "normal" "SPC g S" "git-stage-all")
(define-key "normal" "SPC g U" "git-unstage-all")

(provide-feature "git-status-autoloads")
