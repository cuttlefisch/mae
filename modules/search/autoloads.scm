;; search/autoloads.scm — search keybindings
;; Commands remain kernel builtins; this module owns the keybindings.

;; Normal mode: core search
(define-key "normal" "/" "search-forward-start")
(define-key "normal" "?" "search-backward-start")
(define-key "normal" "n" "search-next")
(define-key "normal" "N" "search-prev")
(define-key "normal" "*" "search-word-under-cursor")
(define-key "normal" "#" "search-word-under-cursor-backward")

;; gn/gN — visual select search match (Practical Vim tip 86)
(define-key "normal" "gn" "visual-select-next-match")
(define-key "normal" "gN" "visual-select-prev-match")
;; Operator variants: dgn, cgn, ygn
(define-key "normal" "dgn" "delete-next-match")
(define-key "normal" "dgN" "delete-prev-match")
(define-key "normal" "cgn" "change-next-match")
(define-key "normal" "cgN" "change-prev-match")
(define-key "normal" "ygn" "yank-next-match")
(define-key "normal" "ygN" "yank-prev-match")

;; Leader search group
(define-key "normal" "SPC s s" "search-buffer")
(define-key "normal" "SPC s p" "project-search")
(define-key "normal" "SPC s h" "clear-search-highlight")
(define-key "normal" "SPC /" "project-search")

(provide-feature "search-autoloads")
