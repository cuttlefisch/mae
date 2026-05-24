;;; dailies/autoloads.scm — org-dailies keybindings
;;; Daily journal notes with backward chain-linking (org-roam-dailies parity).

;;; @module: dailies
;;; @version: 0.2.0
;;; @stability: experimental
;;; @provides: dailies-autoloads

;; SPC n d — dailies prefix group
;; Keybindings live in keymap-doom; this module only adds the group label.
(set-group-name "normal" "SPC n d" "+dailies")

(provide-feature "dailies-autoloads")
