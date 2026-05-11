;; registers/autoloads.scm — register + yank keybindings
;; Commands remain kernel builtins; this module owns the keybindings.

;; Register prompt: "<char> selects register for next yank/delete/paste
(define-key "normal" "\"" "prompt-register")
(define-key "visual" "\"" "prompt-register")

;; Leader register group
(define-key "normal" "SPC r r" "show-registers")
(define-key "normal" "SPC r y" "paste-from-yank")

(provide-feature "registers-autoloads")
