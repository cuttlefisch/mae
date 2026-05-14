;;; multicursor/autoloads.scm — multi-cursor keybindings
;;; Commands remain kernel builtins; this module owns the keybindings.

;;; @module: multicursor
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: multicursor-autoloads

(define-key "normal" "SPC m j" "mc-add-cursor-below")
(define-key "normal" "SPC m k" "mc-add-cursor-above")
(define-key "normal" "SPC m d" "mc-add-at-next-word")
(define-key "normal" "SPC m a" "mc-add-all-word")
(define-key "normal" "SPC m s" "mc-skip-next")
(define-key "normal" "SPC m c" "mc-clear")
(define-key "normal" "SPC m l" "mc-align")

(provide-feature "multicursor-autoloads")
