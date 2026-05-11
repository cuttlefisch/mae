;;; marks-jumps/autoloads.scm — Marks and jump list keybindings
;;;
;;; Registers vi mark/jump keybindings. The marks/jumps logic itself is
;;; implemented in the kernel (Rust dispatch/nav.rs). This module makes
;;; the keybindings disableable.

;; Normal mode — marks
(define-key "normal" "m" "set-mark-await")
(define-key "normal" "'" "jump-mark-await")

;; Normal mode — jump list
(define-key "normal" "C-o" "jump-backward")
(define-key "normal" "C-i" "jump-forward")

;; Normal mode — change list
(define-key "normal" "g ;" "change-backward")
(define-key "normal" "g ," "change-forward")

;; Visual mode — marks
(define-key "visual" "m" "set-mark-await")
(define-key "visual" "'" "jump-mark-await")

(provide-feature "marks-jumps-autoloads")
