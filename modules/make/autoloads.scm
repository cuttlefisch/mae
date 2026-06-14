;;; make/autoloads.scm — build system keybindings
;;;
;;; Build system integration — compile, test, and navigate errors.
;;; Binds SPC c b/t for build/test, SPC c n/p for error navigation.

;;; @module: make
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: make-autoloads

(define-key "leader" "c b" "run-build")
(define-key "leader" "c t" "run-test")
(define-key "leader" "c n" "next-error")
(define-key "leader" "c p" "prev-error")

(provide-feature "make-autoloads")
