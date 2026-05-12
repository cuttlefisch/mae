;; make module autoloads — build system keybindings

(define-key "normal" "SPC c b" "run-build")
(define-key "normal" "SPC c t" "run-test")
(define-key "normal" "SPC c n" "next-error")
(define-key "normal" "SPC c p" "prev-error")

(provide-feature "make-autoloads")
