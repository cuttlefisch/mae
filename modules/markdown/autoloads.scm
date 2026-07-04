;;; markdown/autoloads.scm — Markdown keybindings
;;;
;;; Registers markdown-mode keybindings. The markdown dispatch logic is
;;; implemented in the kernel. This module makes the keybindings disableable.

;;; @module: markdown
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: markdown-autoloads

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

;; --- Markdown LOCAL LEADER (SPC m …) ---
;; Kernel-created `markdown-leader` keymap (parent `leader`) consulted first by
;; the keypad in markdown buffers, so `SPC m` is markdown's local leader without
;; shadowing the global `SPC → leader-dispatch` (see the org module for details).

;; Narrow/widen
(define-key "markdown-leader" "m s n" "md-narrow-subtree")
(define-key "markdown-leader" "m s N" "md-widen")
(define-key "markdown-leader" "m s w" "md-widen")

;; Link editing
(define-key "markdown-leader" "m l" "edit-link")

;; which-key group labels.
(set-group-name "markdown-leader" "m" "+markdown")
(set-group-name "markdown-leader" "m s" "+narrow")

(provide-feature "markdown-autoloads")
