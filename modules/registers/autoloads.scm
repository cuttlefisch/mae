;;; registers/autoloads.scm — register + yank keybindings
;;; Commands remain kernel builtins; this module owns the keybindings.

;;; @module: registers
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: registers-autoloads

;; Register prompt: "<char> selects register for next yank/delete/paste
(define-key "normal" "\"" "prompt-register")
(define-key "visual" "\"" "prompt-register")

;; Leader register group
(define-key "leader" "r r" "show-registers")
(define-key "leader" "r y" "paste-from-yank")

(provide-feature "registers-autoloads")
