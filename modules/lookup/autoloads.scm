;;; lookup/autoloads.scm — documentation lookup keybindings

;;; @module: lookup
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: lookup-autoloads

(define-key "normal" "SPC s o" "lookup-online")
(define-key "normal" "SPC s m" "lookup-man")

(provide-feature "lookup-autoloads")
