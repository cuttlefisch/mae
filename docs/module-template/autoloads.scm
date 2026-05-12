;;; my-module/autoloads.scm — Module description
;;;
;;; @module: my-module
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: my-module-autoloads

;; Keybindings — use define-key to bind commands to keys.
;; Keymap names: "normal", "insert", "visual", or a custom keymap.
;; (define-key "normal" "SPC m x" "my-command")

;; Commands — register editor commands callable from : prompt and AI.
;; (define-command "my-command" "Description" "my-handler-fn")
;; (define (my-handler-fn) (run-command "some-builtin"))

;; Hooks — run code on editor events.
;; (add-hook! "after-save" "my-after-save-hook")
;; (define (my-after-save-hook) ...)

;; AI tools — register tools the AI agent can call.
;; (register-ai-tool! "my-tool"
;;   "Tool description"
;;   '(("param" "string" "Param description"))  ; (name type desc)
;;   '("param")                                  ; required params
;;   "my-tool-handler"                           ; Scheme function
;;   "read")                                     ; permission tier

;; Flag-gated features — only run when user enables +flag.
;; (when-flag "my-module" "extra"
;;   (define-key "normal" "SPC m e" "my-extra-command"))

(provide-feature "my-module-autoloads")
