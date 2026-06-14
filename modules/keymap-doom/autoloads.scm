;;; keymap-doom/autoloads.scm — Doom Emacs-style (modal) keybind flavor
;;;
;;; @module: keymap-doom
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: keymap-doom-autoloads
;;;
;;; The doom flavor is vi-modal: editing happens in Normal mode and the SPC
;;; leader opens the mae which-key menu. The leader TREE itself lives in the
;;; shared `keymap-leader` module (the `leader` keymap) — this flavor only wires
;;; the ENTRY: SPC enters the transient keypad/leader layer (`leader-dispatch`),
;;; which resolves one command against the shared tree and returns to Normal.
;;; This keeps a single source of truth for the leader menu across flavors.
;;;
;;; Vi motions / operators / text objects are kernel primitives (keymaps.rs).
;;; Flavor selection: `keymap_flavor` option (default: "doom").

;; SPC opens the shared leader keypad from Normal and Visual.
(define-key "normal" "SPC" "leader-dispatch")
(define-key "visual" "SPC" "leader-dispatch")

(provide-feature "keymap-doom-autoloads")
