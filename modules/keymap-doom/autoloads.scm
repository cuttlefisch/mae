;;; keymap-doom/autoloads.scm — Doom Emacs-style keybindings
;;; This module defines the SPC leader-key tree and vi-style editing bindings
;;; that make up the "doom" keymap flavor (the default).
;;;
;;; @module: keymap-doom
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: keymap-doom-autoloads
;;;
;;; Currently these bindings are also defined in Rust (keymaps.rs) as the
;;; kernel default. When alternative flavors (emacs, vim-pure, minimal) ship,
;;; the Rust kernel will be trimmed to a minimal base and this module will
;;; become the sole source of Doom-style bindings.
;;;
;;; Flavor selection: `keymap_flavor` option (default: "doom")
;;; Users can create custom flavors in ~/.config/mae/keymaps/<name>/

;; === Leader Key (SPC) Bindings ===

;; Top-level group labels (shown in which-key popup)
(set-group-name "normal" "SPC a" "+ai")
(set-group-name "normal" "SPC b" "+buffer")
(set-group-name "normal" "SPC c" "+code")
(set-group-name "normal" "SPC e" "+eval")
(set-group-name "normal" "SPC f" "+file")
(set-group-name "normal" "SPC h" "+help")
(set-group-name "normal" "SPC l" "+peek")
(set-group-name "normal" "SPC n" "+notes")
(set-group-name "normal" "SPC o" "+open")
(set-group-name "normal" "SPC p" "+project")
(set-group-name "normal" "SPC q" "+quit")
(set-group-name "normal" "SPC s" "+select")
(set-group-name "normal" "SPC t" "+toggle")
(set-group-name "normal" "SPC w" "+window")

;; +buffer
(define-key "normal" "SPC b s" "save")
(define-key "normal" "SPC b b" "switch-buffer")
(define-key "normal" "SPC b d" "kill-buffer")
(define-key "normal" "SPC b n" "next-buffer")
(define-key "normal" "SPC b p" "prev-buffer")
(define-key "normal" "SPC b l" "alternate-file")
(define-key "normal" "SPC b a" "alternate-file")
(define-key "normal" "SPC b m" "view-messages")
(define-key "normal" "SPC b N" "new-buffer")
(define-key "normal" "SPC b D" "force-kill-buffer")
(define-key "normal" "SPC b k" "kill-buffer")
(define-key "normal" "SPC b i" "file-info")
(define-key "normal" "SPC b o" "kill-other-buffers")
(define-key "normal" "SPC b S" "save-all-buffers")
(define-key "normal" "SPC b r" "revert-buffer")

;; +file
(define-key "normal" "SPC f f" "find-file")
(define-key "normal" "SPC f d" "file-browser")
(define-key "normal" "SPC f s" "save")
(define-key "normal" "SPC f r" "recent-files")
(define-key "normal" "SPC f y" "yank-file-path")
(define-key "normal" "SPC f R" "rename-file")
(define-key "normal" "SPC f n" "new-buffer")
(define-key "normal" "SPC f c" "edit-config")
(define-key "normal" "SPC f C" "copy-this-file")
(define-key "normal" "SPC f P" "edit-settings")
(define-key "normal" "SPC f S" "save-as")
(define-key "normal" "SPC f D" "delete-this-file")

;; +window
(define-key "normal" "SPC w v" "split-vertical")
(define-key "normal" "SPC w s" "split-horizontal")
(define-key "normal" "SPC w q" "close-window")
(define-key "normal" "SPC w h" "focus-left")
(define-key "normal" "SPC w j" "focus-down")
(define-key "normal" "SPC w k" "focus-up")
(define-key "normal" "SPC w l" "focus-right")
(define-key "normal" "SPC w +" "window-grow")
(define-key "normal" "SPC w -" "window-shrink")
(define-key "normal" "SPC w >" "window-grow-width")
(define-key "normal" "SPC w <" "window-shrink-width")
(define-key "normal" "SPC w =" "window-balance")
(define-key "normal" "SPC w m" "window-maximize")
(define-key "normal" "SPC w H" "window-move-left")
(define-key "normal" "SPC w J" "window-move-down")
(define-key "normal" "SPC w K" "window-move-up")
(define-key "normal" "SPC w L" "window-move-right")
(define-key "normal" "SPC w w" "focus-next-window")
(define-key "normal" "SPC w d" "close-window")

;; +ai
(define-key "normal" "SPC a a" "open-ai-agent")
(define-key "normal" "SPC a p" "ai-prompt")
(define-key "normal" "SPC a c" "ai-cancel")
(define-key "normal" "SPC a m" "ai-set-mode")
(define-key "normal" "SPC a P" "ai-set-profile")
(define-key "normal" "SPC a n" "ai-ping")
(define-key "normal" "SPC a v" "verify")

;; +help
(define-key "normal" "SPC h h" "help")
(define-key "normal" "SPC h k" "describe-key")
(define-key "normal" "SPC h c" "describe-command")
(define-key "normal" "SPC h o" "describe-option")
(define-key "normal" "SPC h t" "tutor")
(define-key "normal" "SPC h s" "help-search")
(define-key "normal" "SPC h b" "help-back")
(define-key "normal" "SPC h f" "help-forward")
(define-key "normal" "SPC h q" "help-close")
(define-key "normal" "SPC h l" "help-reopen")
(define-key "normal" "SPC h d" "dashboard")
(define-key "normal" "SPC h B" "describe-bindings")
(define-key "normal" "SPC h m" "describe-mode")
(define-key "normal" "SPC h D" "describe-display-policy")

;; +scratch
(define-key "normal" "SPC x" "toggle-scratch-buffer")

;; +theme/toggle
(define-key "normal" "SPC t t" "cycle-theme")
(define-key "normal" "SPC t S" "set-theme")
(define-key "normal" "SPC t l" "toggle-line-numbers")
(define-key "normal" "SPC t r" "toggle-relative-line-numbers")
(define-key "normal" "SPC t w" "toggle-word-wrap")
(define-key "normal" "SPC t i" "toggle-inline-images")
(define-key "normal" "SPC t s" "toggle-scrollbar")
(define-key "normal" "SPC t F" "toggle-fps")
(define-key "normal" "SPC t D" "debug-mode")
(define-key "normal" "SPC t d" "toggle-lsp-diagnostics-inline")

;; +quit
(define-key "normal" "SPC q q" "quit")
(define-key "normal" "SPC q Q" "force-quit")
(define-key "normal" "SPC q s" "save-and-quit")
(define-key "normal" "SPC q S" "save-all-and-quit")

;; +search/syntax
(define-key "normal" "SPC s n" "syntax-select-node")
(define-key "normal" "SPC s e" "syntax-expand-selection")
(define-key "normal" "SPC s c" "syntax-contract-selection")

;; +eval
(define-key "normal" "SPC e l" "eval-line")
(define-key "normal" "SPC e b" "eval-buffer")
(define-key "normal" "SPC e o" "open-scheme-repl")
(define-key "normal" "SPC e s" "send-to-shell")

;; +project
(define-key "normal" "SPC p f" "project-find-file")
(define-key "normal" "SPC p s" "project-search")
(define-key "normal" "SPC p d" "project-browse")
(define-key "normal" "SPC p r" "project-recent-files")
(define-key "normal" "SPC p p" "project-switch")
(define-key "normal" "SPC p a" "add-project")
(define-key "normal" "SPC p D" "project-forget")
(define-key "normal" "SPC p c" "project-clean")

;; +notes
;; +dailies
(define-key "normal" "SPC n d t" "daily-goto-today")
(define-key "normal" "SPC n d y" "daily-goto-yesterday")
(define-key "normal" "SPC n d d" "daily-goto-date")
(define-key "normal" "SPC n d p" "daily-prev")
(define-key "normal" "SPC n d n" "daily-next")
(define-key "normal" "SPC n f" "kb-find")
(define-key "normal" "SPC n v" "kb-view")
(define-key "normal" "SPC n e" "kb-edit-source")
(define-key "normal" "SPC n c" "kb-create")
(define-key "normal" "SPC n D" "kb-delete")
(define-key "normal" "SPC n r" "kb-register")
(define-key "normal" "SPC n R" "kb-reimport")
(define-key "normal" "SPC n i" "kb-insert-link")
(define-key "normal" "SPC n s" "capture-finalize")
(define-key "normal" "SPC n k" "capture-abort")
(define-key "normal" "SPC n C" "kb-cleanup-orphans")
(define-key "normal" "SPC n I" "kb-instances")
(define-key "normal" "SPC n h" "kb-health")

;; +code (LSP)
(define-key "normal" "SPC c d" "lsp-goto-definition")
(define-key "normal" "SPC c r" "lsp-find-references")
(define-key "normal" "SPC c k" "lsp-hover")
(define-key "normal" "SPC c x" "lsp-show-diagnostics")
(define-key "normal" "SPC c a" "lsp-code-action")
(define-key "normal" "SPC c R" "lsp-rename")
;; SPC c f owned by format module (format-buffer)
(define-key "normal" "SPC c F" "lsp-range-format")
(define-key "normal" "SPC c s" "lsp-status")
(define-key "normal" "SPC c o" "lsp-symbol-outline")
(define-key "normal" "SPC l p" "lsp-peek-definition")
(define-key "normal" "SPC l r" "lsp-peek-references")

;; +open
(define-key "normal" "SPC o t" "terminal")
(define-key "normal" "SPC o T" "terminal-here")
(define-key "normal" "SPC o r" "terminal-reset")
(define-key "normal" "SPC o c" "terminal-close")

;; +command palette
(define-key "normal" "SPC SPC" "command-palette")
(define-key "normal" "SPC :" "enter-command-mode")

;; +collaboration
(set-group-name "normal" "SPC C" "collaboration")
(define-key "normal" "SPC C s" "collab-start")
(define-key "normal" "SPC C c" "collab-connect")
(define-key "normal" "SPC C d" "collab-disconnect")
(define-key "normal" "SPC C i" "collab-status")
(define-key "normal" "SPC C S" "collab-share")
(define-key "normal" "SPC C y" "collab-sync")
(define-key "normal" "SPC C D" "collab-doctor")
(define-key "normal" "SPC C l" "collab-list")
(define-key "normal" "SPC C j" "collab-join")
(define-key "normal" "SPC C P" "collab-discover")

;; +kb-collaboration
(set-group-name "normal" "SPC C K" "KB sharing")
(define-key "normal" "SPC C K s" "kb-share")
(define-key "normal" "SPC C K j" "kb-join")
(define-key "normal" "SPC C K l" "kb-leave")
(define-key "normal" "SPC C K r" "kb-list-remote")

;; Visual mode SPC bindings
(define-key "visual" "SPC s n" "syntax-select-node")
(define-key "visual" "SPC s e" "syntax-expand-selection")
(define-key "visual" "SPC s c" "syntax-contract-selection")
(define-key "visual" "SPC e r" "eval-region")
(define-key "visual" "SPC e S" "send-region-to-shell")

(provide-feature "keymap-doom-autoloads")
