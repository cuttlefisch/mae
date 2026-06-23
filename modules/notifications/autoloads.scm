;;; notifications/autoloads.scm — Attention bus *Notifications* buffer (ADR-024)
;;;
;;; Registers the notifications keymap (buffer-local, parented on navigation)
;;; and the SPC n leader entry. The dispatch logic lives in the kernel
;;; (editor/dispatch/notify.rs).

;;; @module: notifications
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: notifications-autoloads

;; Buffer-local keymap inheriting read-only navigation.
(define-keymap "notifications" "navigation")

;; Navigation
(define-key "notifications" "j" "move-down")
(define-key "notifications" "k" "move-up")
(define-key "notifications" "G" "move-to-last-line")
(define-key "notifications" "g g" "move-to-first-line")

;; At-point actions
(define-key "notifications" "Enter" "notify-run-action")
(define-key "notifications" "d" "notify-dismiss")
(define-key "notifications" "Tab" "notify-toggle-fold")

;; Refresh / close / help
(define-key "notifications" "g r" "notifications-open")
(define-key "notifications" "q" "kill-buffer")
(define-key "notifications" "Escape" "kill-buffer")
(define-key "notifications" "?" "show-buffer-keys")

;; SPC n leader entry (appears in every flavor's keypad).
(define-key "leader" "n n" "notifications-open")
(set-group-name "leader" "n" "+notify")

(provide-feature "notifications-autoloads")
