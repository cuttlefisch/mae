;;; make/autoloads.scm — build system keybindings

;;; @module: make
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: make-autoloads

(define-key "normal" "SPC c b" "run-build")
(define-key "normal" "SPC c t" "run-test")
(define-key "normal" "SPC c n" "next-error")
(define-key "normal" "SPC c p" "prev-error")

(provide-feature "make-autoloads")
