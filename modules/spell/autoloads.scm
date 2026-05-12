;;; spell/autoloads.scm — spell check keybindings

;;; @module: spell
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: spell-autoloads

(define-key "normal" "z=" "spell-suggest")
(define-key "normal" "]s" "spell-next")
(define-key "normal" "[s" "spell-prev")
(define-key "normal" "SPC t s" "spell-toggle")

(provide-feature "spell-autoloads")
