;;; spell/autoloads.scm — spell check keybindings
;;;
;;; Spell checking commands — suggest corrections, navigate misspellings,
;;; and toggle spell checking per buffer.

;;; @module: spell
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: spell-autoloads

(define-key "normal" "z=" "spell-suggest")
(define-key "normal" "]s" "spell-next")
(define-key "normal" "[s" "spell-prev")
(define-key "leader" "t Z" "spell-toggle")

(provide-feature "spell-autoloads")
