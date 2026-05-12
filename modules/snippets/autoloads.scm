;; snippets module autoloads — snippet expansion keybindings
;;
;; Tab in insert mode: expand trigger or advance to next field
;; S-Tab: go to previous field
;; Snippet session auto-commits on Esc (handled in kernel)

(define-key "insert" "Tab" "snippet-expand-or-next")
(define-key "insert" "S-Tab" "snippet-prev-field")

(provide-feature "snippets-autoloads")
