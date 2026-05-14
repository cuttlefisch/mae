;;; snippets/autoloads.scm — snippet expansion keybindings
;;;
;;; Tab in insert mode: expand trigger or advance to next field
;;; S-Tab: go to previous field
;;; Snippet session auto-commits on Esc (handled in kernel)

;;; @module: snippets
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: snippets-autoloads

(define-key "insert" "Tab" "snippet-expand-or-next")
(define-key "insert" "S-Tab" "snippet-prev-field")

(provide-feature "snippets-autoloads")
