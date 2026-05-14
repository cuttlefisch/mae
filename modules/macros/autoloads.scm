;;; macros/autoloads.scm — macro recording/replay keybindings
;;; Commands remain kernel builtins; this module owns the keybindings.

;;; @module: macros
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: macros-autoloads

;; q<letter> — start/stop recording macro
(define-key "normal" "q" "start-recording-await")
;; @<letter> — replay macro (@@ repeats last, handled by dispatch)
(define-key "normal" "@" "replay-macro-await")

(provide-feature "macros-autoloads")
