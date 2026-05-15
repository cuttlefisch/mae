;;; dailies/autoloads.scm — org-dailies keybindings
;;; Daily journal notes with backward chain-linking (org-roam-dailies parity).

;;; @module: dailies
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: dailies-autoloads

;; SPC n d — dailies prefix group
(define-key "normal" "SPC n d t" "daily-goto-today")
(define-key "normal" "SPC n d y" "daily-goto-yesterday")
(define-key "normal" "SPC n d d" "daily-goto-date")
(define-key "normal" "SPC n d p" "daily-prev")
(define-key "normal" "SPC n d n" "daily-next")

(provide-feature "dailies-autoloads")
