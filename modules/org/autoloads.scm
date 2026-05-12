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

;; Narrow/widen
(define-key "org" "SPC m s n" "org-narrow-subtree")
(define-key "org" "SPC m s N" "org-widen")
(define-key "org" "SPC m s w" "org-widen")

;; Link editing
(define-key "org" "SPC m l" "edit-link")

;; Tags
(define-key "org" "SPC m t" "org-set-tags")

;; Babel commands (gated by +babel flag)
(define-key "org" "SPC m x" "babel-execute")
(define-key "org" "SPC m X" "babel-execute-all")
(define-key "org" "SPC m T" "babel-tangle")
(define-key "org" "SPC m '" "babel-edit-special")
(define-key "org" "C-c C-c" "babel-execute")

;; Export commands (gated by +export flag)
(define-key "org" "SPC m e h" "org-export-html")
(define-key "org" "SPC m e m" "org-export-markdown")
(define-key "org" "SPC m e s" "org-export-subtree")

(provide-feature "org-autoloads")
