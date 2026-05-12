;;; agenda/autoloads.scm — Agenda buffer keybindings
;;;
;;; Registers the agenda keymap and SPC o a launcher.
;;; Depends on the org module.

;;; @module: agenda
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: agenda-autoloads

;; Create agenda keymap inheriting from normal
(define-keymap "agenda" "normal")

(define-key "agenda" "Enter" "agenda-goto")
(define-key "agenda" "q" "kill-buffer")
(define-key "agenda" "r" "agenda-refresh")
(define-key "agenda" "t" "agenda-filter-todo")
(define-key "agenda" "p" "agenda-filter-priority")
(define-key "agenda" "?" "show-buffer-keys")

;; SPC o a/A — open/add agenda from normal mode
(define-key "normal" "SPC o a" "open-agenda")
(define-key "normal" "SPC o A" "agenda-add")

(provide-feature "agenda-autoloads")
