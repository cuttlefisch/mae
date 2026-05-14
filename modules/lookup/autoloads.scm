;;; lookup/autoloads.scm — documentation lookup keybindings
;;;
;;; Online documentation and man-page lookup commands. Binds SPC s o
;;; for web search and SPC s m for local man-page display.

;;; @module: lookup
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: lookup-autoloads

(define-key "normal" "SPC s o" "lookup-online")
(define-key "normal" "SPC s m" "lookup-man")

(provide-feature "lookup-autoloads")
