;;; debug/autoloads.scm — Debug panel keybindings and SPC d leader
;;;
;;; Registers the debug panel keymap and SPC d leader bindings.
;;; The DAP dispatch logic is implemented in the kernel.

;; Create debug keymap inheriting from normal
(define-keymap "debug" "normal")

(define-key "debug" "j" "debug-move-down")
(define-key "debug" "k" "debug-move-up")
(define-key "debug" "Enter" "debug-panel-select")
(define-key "debug" "q" "close-debug-panel")
(define-key "debug" "o" "debug-toggle-output")
(define-key "debug" "r" "dap-refresh")
(define-key "debug" "c" "debug-continue")
(define-key "debug" "n" "debug-step-over")
(define-key "debug" "s" "debug-step-into")
(define-key "debug" "S" "debug-step-out")
(define-key "debug" "?" "show-buffer-keys")

;; SPC d leader bindings (normal mode)
(define-key "normal" "SPC d d" "debug-start")
(define-key "normal" "SPC d s" "debug-self")
(define-key "normal" "SPC d q" "debug-stop")
(define-key "normal" "SPC d c" "debug-continue")
(define-key "normal" "SPC d n" "debug-step-over")
(define-key "normal" "SPC d i" "debug-step-into")
(define-key "normal" "SPC d o" "debug-step-out")
(define-key "normal" "SPC d b" "debug-toggle-breakpoint")
(define-key "normal" "SPC d v" "debug-inspect")
(define-key "normal" "SPC d p" "debug-panel")
(define-key "normal" "SPC d w" "debug-add-watch")
(define-key "normal" "SPC d W" "debug-remove-watch")
(define-key "normal" "SPC d e" "debug-exceptions")

(provide-feature "debug-autoloads")
