;;; dailies/autoloads.scm — org-dailies keybindings
;;; Daily journal notes with backward chain-linking (org-roam-dailies parity).

;;; @module: dailies
;;; @version: 0.2.0
;;; @stability: experimental
;;; @provides: dailies-autoloads

;; SPC n d — dailies prefix group
(set-group-name "normal" "SPC n d" "+dailies")
(define-key "normal" "SPC n d t" "daily-goto-today")
(define-key "normal" "SPC n d y" "daily-goto-yesterday")
(define-key "normal" "SPC n d d" "daily-goto-date")
(define-key "normal" "SPC n d p" "daily-prev")
(define-key "normal" "SPC n d n" "daily-next")

(provide-feature "dailies-autoloads")
