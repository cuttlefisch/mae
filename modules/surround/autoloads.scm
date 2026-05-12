;;; surround/autoloads.scm — Surround module keybindings
;;;
;;; Registers vim-surround keybindings. The surround logic itself is
;;; implemented in the kernel (Rust dispatch/edit.rs). This module makes
;;; the keybindings disableable by omitting the module from init.scm.

;;; @module: surround
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: surround-autoloads

;; Normal mode
(define-key "normal" "d s" "delete-surround-await")
(define-key "normal" "c s" "change-surround-await")
(define-key "normal" "y s" "operator-surround")
(define-key "normal" "y s s" "surround-line-await")

;; Visual mode
(define-key "visual" "S" "surround-visual-await")

(provide-feature "surround-autoloads")
