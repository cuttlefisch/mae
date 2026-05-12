;;; markdown/autoloads.scm — Markdown keybindings
;;;
;;; Registers markdown-mode keybindings. The markdown dispatch logic is
;;; implemented in the kernel. This module makes the keybindings disableable.

;; Create markdown keymap inheriting from normal
(define-keymap "markdown" "normal")

;; Fold/cycle
(define-key "markdown" "Tab" "md-cycle")
(define-key "markdown" "S-Tab" "md-global-cycle")

;; Promote/demote headings
(define-key "markdown" "M-Left" "md-promote")
(define-key "markdown" "M-Right" "md-demote")
(define-key "markdown" "M-h" "md-promote")
(define-key "markdown" "M-l" "md-demote")

;; Move subtrees
(define-key "markdown" "M-j" "md-move-subtree-down")
(define-key "markdown" "M-k" "md-move-subtree-up")
(define-key "markdown" "M-Up" "md-move-subtree-up")
(define-key "markdown" "M-Down" "md-move-subtree-down")

;; Insert heading
(define-key "markdown" "M-Enter" "md-insert-heading")

;; Smart enter
(define-key "markdown" "Enter" "smart-enter")

;; Narrow/widen
(define-key "markdown" "SPC m s n" "md-narrow-subtree")
(define-key "markdown" "SPC m s N" "md-widen")
(define-key "markdown" "SPC m s w" "md-widen")

;; Link editing
(define-key "markdown" "SPC m l" "edit-link")

(provide-feature "markdown-autoloads")
