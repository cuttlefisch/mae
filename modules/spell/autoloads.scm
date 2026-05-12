;; spell module autoloads — spell check keybindings

(define-key "normal" "z=" "spell-suggest")
(define-key "normal" "]s" "spell-next")
(define-key "normal" "[s" "spell-prev")
(define-key "normal" "SPC t s" "spell-toggle")

(provide-feature "spell-autoloads")
