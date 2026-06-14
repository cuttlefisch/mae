;;; keymap-nonmodal/autoloads.scm — non-modal (CUA-style) keybind flavor
;;;
;;; @module: keymap-nonmodal
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: keymap-nonmodal-autoloads
;;;
;;; For users not fluent in modal editing (VSCode/TextEdit muscle memory). The
;;; editor stays in Insert by default — typing always inserts. There is NO vi
;;; normal mode to learn; instead `C-;` opens the SAME mae which-key menu as the
;;; doom flavor, as a transient keypad (God-Mode / Meow-Keypad model): pick one
;;; command, it runs, you're back to typing. Esc / C-g cancels the keypad.
;;;
;;; The leader TREE is the shared `keymap-leader` module (dependency); this
;;; flavor only sets Insert as the default mode and wires the entry + CUA chords.

;; Boot into Insert — applied by bootstrap after modules + config load.
(set-option! "default_mode" "insert")

;; C-; opens the transient leader keypad (the mae which-key menu).
;; Chosen for cross-OS safety: C-SPC collides with macOS Spotlight, M-SPC with
;; the macOS Character Viewer. Rebind with (define-key "insert" "<key>" "leader-dispatch").
(define-key "insert" "C-;" "leader-dispatch")

;; CUA chords (insert mode). Plain keys still insert; these dispatch commands.
(define-key "insert" "C-s" "save")
(define-key "insert" "C-z" "undo")
(define-key "insert" "C-y" "redo")
(define-key "insert" "C-f" "search-forward-start")
(define-key "insert" "C-v" "paste-after")

;; NOTE: C-c / C-x (copy/cut) and shift-arrow selection need a non-modal
;; selection model (follow-up). Until then, selection-based commands remain
;; reachable via the leader keypad.

(provide-feature "keymap-nonmodal-autoloads")
