;;; debug/autoloads.scm — Debug panel keybindings and SPC d leader
;;;
;;; Registers the debug panel keymap and SPC d leader bindings.
;;; The DAP dispatch logic is implemented in the kernel.

;;; @module: debug
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: debug-autoloads

;; Create debug keymap inheriting from normal
(define-keymap "debug" "navigation")

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
(define-key "leader" "d d" "debug-start")
(define-key "leader" "d s" "debug-self")
(define-key "leader" "d q" "debug-stop")
(define-key "leader" "d c" "debug-continue")
(define-key "leader" "d n" "debug-step-over")
(define-key "leader" "d i" "debug-step-into")
(define-key "leader" "d o" "debug-step-out")
(define-key "leader" "d b" "debug-toggle-breakpoint")
(define-key "leader" "d v" "debug-inspect")
(define-key "leader" "d p" "debug-panel")
(define-key "leader" "d w" "debug-add-watch")
(define-key "leader" "d W" "debug-remove-watch")
(define-key "leader" "d e" "debug-exceptions")

(provide-feature "debug-autoloads")
