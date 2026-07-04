;;; org/autoloads.scm — Org-mode keybindings
;;;
;;; Registers org-mode keybindings. The org dispatch logic is implemented
;;; in the kernel (Rust dispatch/fold_org.rs). This module makes the
;;; keybindings disableable by omitting the module from init.scm.

;;; @module: org
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: org-autoloads

;; Create org keymap inheriting from normal
(define-keymap "org" "normal")

;; Fold/cycle
(define-key "org" "Tab" "org-cycle")
(define-key "org" "S-Tab" "org-global-cycle")

;; TODO/priority
(define-key "org" "S-Left" "org-todo-prev")
(define-key "org" "S-Right" "org-todo-next")
(define-key "org" "S-Up" "org-priority-up")
(define-key "org" "S-Down" "org-priority-down")

;; Smart enter
(define-key "org" "Enter" "smart-enter")

;; Promote/demote headings
(define-key "org" "M-Left" "org-promote")
(define-key "org" "M-Right" "org-demote")
(define-key "org" "M-h" "org-promote")
(define-key "org" "M-l" "org-demote")

;; Move subtrees
(define-key "org" "M-j" "org-move-subtree-down")
(define-key "org" "M-k" "org-move-subtree-up")
(define-key "org" "M-Up" "org-move-subtree-up")
(define-key "org" "M-Down" "org-move-subtree-down")

;; Insert heading
(define-key "org" "M-Enter" "org-insert-heading")

;; Direct babel run (not SPC-prefixed, so it stays on the buffer keymap).
(define-key "org" "C-c C-c" "babel-execute")

;; --- Org LOCAL LEADER (SPC m …) ---
;; These live in the kernel-created `org-leader` keymap (parent `leader`) that
;; the transient keypad consults FIRST in org buffers — so `SPC m` is org's
;; major-mode local leader while `SPC b/f/w/…` still reach the global leader.
;; Binding `SPC m …` directly into the `org` keymap (as before) made `SPC` a
;; prefix there, which shadowed the global `SPC → leader-dispatch` and left only
;; `m` visible. The keys are identical; only where they're registered changed.

;; Narrow/widen
(define-key "org-leader" "m s n" "org-narrow-subtree")
(define-key "org-leader" "m s N" "org-widen")
(define-key "org-leader" "m s w" "org-widen")

;; Link editing
(define-key "org-leader" "m l" "edit-link")

;; Tags
(define-key "org-leader" "m t" "org-set-tags")

;; Babel commands (gated by +babel flag)
(define-key "org-leader" "m x" "babel-execute")
(define-key "org-leader" "m X" "babel-execute-all")
(define-key "org-leader" "m T" "babel-tangle")
(define-key "org-leader" "m '" "babel-edit-special")

;; Export commands (gated by +export flag)
(define-key "org-leader" "m e h" "org-export-html")
(define-key "org-leader" "m e m" "org-export-markdown")
(define-key "org-leader" "m e s" "org-export-subtree")

;; which-key group labels for the local-leader submenus.
(set-group-name "org-leader" "m" "+org")
(set-group-name "org-leader" "m s" "+narrow")
(set-group-name "org-leader" "m e" "+export")

(provide-feature "org-autoloads")
