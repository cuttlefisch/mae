;;; multicursor/autoloads.scm — multi-cursor keybindings
;;; Commands remain kernel builtins; this module owns the keybindings.

;;; @module: multicursor
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: multicursor-autoloads

(define-key "leader" "m j" "mc-add-cursor-below")
(define-key "leader" "m k" "mc-add-cursor-above")
(define-key "leader" "m d" "mc-add-at-next-word")
(define-key "leader" "m a" "mc-add-all-word")
(define-key "leader" "m s" "mc-skip-next")
(define-key "leader" "m c" "mc-clear")
(define-key "leader" "m l" "mc-align")

(provide-feature "multicursor-autoloads")
