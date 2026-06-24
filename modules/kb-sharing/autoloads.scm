;;; kb-sharing/autoloads.scm — *KB Sharing* management buffer (collab UX)
;;;
;;; Registers the kb-sharing keymap (buffer-local, parented on navigation) and
;;; the SPC C K m leader entry. The dispatch logic lives in the kernel
;;; (editor/dispatch/kb_sharing.rs); the buffer is built from the KB-sharing
;;; introspection snapshot (the same one the MCP tool + Scheme primitive expose).

;;; @module: kb-sharing
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: kb-sharing-autoloads

;; Buffer-local keymap inheriting read-only navigation.
(define-keymap "kb-sharing" "navigation")

;; Navigation
(define-key "kb-sharing" "j" "move-down")
(define-key "kb-sharing" "k" "move-up")
(define-key "kb-sharing" "G" "move-to-last-line")
(define-key "kb-sharing" "g g" "move-to-first-line")

;; Fold / refresh
(define-key "kb-sharing" "Tab" "kb-sharing-toggle-fold")
(define-key "kb-sharing" "g r" "kb-sharing-refresh")

;; Pending-request actions (owner-only; on a pending row)
(define-key "kb-sharing" "a" "kb-sharing-approve")
(define-key "kb-sharing" "d" "kb-sharing-deny")

;; Member actions (role changes + remove owner-only; copy for anyone; on a member row)
(define-key "kb-sharing" "e" "kb-sharing-role-editor")
(define-key "kb-sharing" "v" "kb-sharing-role-viewer")
(define-key "kb-sharing" "o" "kb-sharing-role-owner")
(define-key "kb-sharing" "x" "kb-sharing-remove")
(define-key "kb-sharing" "y" "kb-sharing-copy-fingerprint")

;; KB-level actions (on a KB header / policy line)
(define-key "kb-sharing" "p" "kb-sharing-set-policy")
(define-key "kb-sharing" "L" "kb-sharing-leave")

;; Close / help
(define-key "kb-sharing" "q" "kill-buffer")
(define-key "kb-sharing" "Escape" "kill-buffer")
(define-key "kb-sharing" "?" "show-buffer-keys")

;; SPC C K m leader entry (appears in every flavor's keypad, alongside KB sharing).
(define-key "leader" "C K m" "kb-sharing")

(provide-feature "kb-sharing-autoloads")
