;;; keymap-leader/autoloads.scm — the shared mae which-key leader tree
;;;
;;; @module: keymap-leader
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: keymap-leader-autoloads
;;;
;;; This is the SINGLE source of truth for mae's leader menu (buffer/file/
;;; window/ai/help/project/notes/code/eval/toggle/open/collab/…). It binds into
;;; a dedicated `leader` keymap — NOT under an "SPC " prefix — which the
;;; transient keypad layer (`leader-dispatch`) resolves against. Every keymap
;;; flavor depends on this module and only adds its own ENTRY into the leader
;;; tree:
;;;   - keymap-doom:     SPC (normal/visual) -> leader-dispatch
;;;   - keymap-nonmodal: C-; (insert)         -> leader-dispatch
;;; so both flavors share one mae-tailored which-key menu (no duplication).

(define-keymap "leader" "normal")

;; Top-level group labels (shown in the which-key popup)
(set-group-name "leader" "a" "+ai")
(set-group-name "leader" "b" "+buffer")
(set-group-name "leader" "c" "+code")
(set-group-name "leader" "e" "+eval")
(set-group-name "leader" "f" "+file")
(set-group-name "leader" "h" "+help")
(set-group-name "leader" "l" "+peek")
(set-group-name "leader" "n" "+notes")
(set-group-name "leader" "o" "+open")
(set-group-name "leader" "p" "+project")
(set-group-name "leader" "q" "+quit")
(set-group-name "leader" "s" "+select")
(set-group-name "leader" "t" "+toggle")
(set-group-name "leader" "w" "+window")

;; +buffer
(define-key "leader" "b s" "save")
(define-key "leader" "b b" "switch-buffer")
(define-key "leader" "b d" "kill-buffer")
(define-key "leader" "b n" "next-buffer")
(define-key "leader" "b p" "prev-buffer")
(define-key "leader" "b l" "alternate-file")
(define-key "leader" "b a" "alternate-file")
(define-key "leader" "b m" "view-messages")
(define-key "leader" "b N" "new-buffer")
(define-key "leader" "b D" "force-kill-buffer")
(define-key "leader" "b k" "kill-buffer")
(define-key "leader" "b i" "file-info")
(define-key "leader" "b o" "kill-other-buffers")
(define-key "leader" "b S" "save-all-buffers")
(define-key "leader" "b r" "revert-buffer")

;; +file
(define-key "leader" "f f" "find-file")
(define-key "leader" "f d" "file-browser")
(define-key "leader" "f s" "save")
(define-key "leader" "f r" "recent-files")
(define-key "leader" "f y" "yank-file-path")
(define-key "leader" "f R" "rename-file")
(define-key "leader" "f n" "new-buffer")
(define-key "leader" "f c" "edit-config")
(define-key "leader" "f C" "copy-this-file")
(define-key "leader" "f P" "edit-settings")
(define-key "leader" "f S" "save-as")
(define-key "leader" "f D" "delete-this-file")

;; +window
(define-key "leader" "w v" "split-vertical")
(define-key "leader" "w s" "split-horizontal")
(define-key "leader" "w q" "close-window")
(define-key "leader" "w h" "focus-left")
(define-key "leader" "w j" "focus-down")
(define-key "leader" "w k" "focus-up")
(define-key "leader" "w l" "focus-right")
(define-key "leader" "w +" "window-grow")
(define-key "leader" "w -" "window-shrink")
(define-key "leader" "w >" "window-grow-width")
(define-key "leader" "w <" "window-shrink-width")
(define-key "leader" "w =" "window-balance")
(define-key "leader" "w m" "window-maximize")
(define-key "leader" "w H" "window-move-left")
(define-key "leader" "w J" "window-move-down")
(define-key "leader" "w K" "window-move-up")
(define-key "leader" "w L" "window-move-right")
(define-key "leader" "w w" "focus-next-window")
(define-key "leader" "w d" "close-window")

;; +ai
(define-key "leader" "a a" "open-ai-agent")
(define-key "leader" "a p" "ai-prompt")
(define-key "leader" "a c" "ai-cancel")
(define-key "leader" "a m" "ai-set-mode")
(define-key "leader" "a P" "ai-set-profile")
(define-key "leader" "a n" "ai-ping")
(define-key "leader" "a v" "verify")

;; +help
(define-key "leader" "h h" "help")
(define-key "leader" "h k" "describe-key")
(define-key "leader" "h c" "describe-command")
(define-key "leader" "h o" "describe-option")
(define-key "leader" "h t" "tutor")
(define-key "leader" "h s" "help-search")
(define-key "leader" "h b" "help-back")
(define-key "leader" "h f" "help-forward")
(define-key "leader" "h q" "help-close")
(define-key "leader" "h l" "help-reopen")
(define-key "leader" "h d" "dashboard")
(define-key "leader" "h B" "describe-bindings")
(define-key "leader" "h m" "describe-mode")
(define-key "leader" "h D" "describe-display-policy")

;; +scratch
(define-key "leader" "x" "toggle-scratch-buffer")

;; +theme/toggle
(define-key "leader" "t t" "cycle-theme")
(define-key "leader" "t S" "set-theme")
(define-key "leader" "t l" "toggle-line-numbers")
(define-key "leader" "t r" "toggle-relative-line-numbers")
(define-key "leader" "t w" "toggle-word-wrap")
(define-key "leader" "t i" "toggle-inline-images")
(define-key "leader" "t s" "toggle-scrollbar")
(define-key "leader" "t F" "toggle-fps")
(define-key "leader" "t D" "debug-mode")
(define-key "leader" "t d" "toggle-lsp-diagnostics-inline")
(define-key "leader" "t K" "keymap-set-flavor")

;; +quit
(define-key "leader" "q q" "quit")
(define-key "leader" "q Q" "force-quit")
(define-key "leader" "q s" "save-and-quit")
(define-key "leader" "q S" "save-all-and-quit")

;; +select/syntax
(define-key "leader" "s n" "syntax-select-node")
(define-key "leader" "s e" "syntax-expand-selection")
(define-key "leader" "s c" "syntax-contract-selection")

;; +eval
(define-key "leader" "e l" "eval-line")
(define-key "leader" "e b" "eval-buffer")
(define-key "leader" "e o" "open-scheme-repl")
(define-key "leader" "e s" "send-to-shell")
(define-key "leader" "e r" "eval-region")
(define-key "leader" "e S" "send-region-to-shell")

;; +project
(define-key "leader" "p f" "project-find-file")
(define-key "leader" "p s" "project-search")
(define-key "leader" "p d" "project-browse")
(define-key "leader" "p r" "project-recent-files")
(define-key "leader" "p p" "project-switch")
(define-key "leader" "p a" "add-project")
(define-key "leader" "p D" "project-forget")
(define-key "leader" "p c" "project-clean")

;; +notes / +dailies
(define-key "leader" "n d t" "daily-goto-today")
(define-key "leader" "n d y" "daily-goto-yesterday")
(define-key "leader" "n d d" "daily-goto-date")
(define-key "leader" "n d p" "daily-prev")
(define-key "leader" "n d n" "daily-next")
(define-key "leader" "n f" "kb-find")
(define-key "leader" "n v" "kb-view")
(define-key "leader" "n e" "kb-edit-source")
(define-key "leader" "n c" "kb-create")
(define-key "leader" "n D" "kb-delete")
(define-key "leader" "n r" "kb-register")
(define-key "leader" "n R" "kb-reimport")
(define-key "leader" "n i" "kb-insert-link")
(define-key "leader" "n s" "capture-finalize")
(define-key "leader" "n k" "capture-abort")
(define-key "leader" "n C" "kb-cleanup-orphans")
(define-key "leader" "n I" "kb-instances")
(define-key "leader" "n h" "kb-health")
(define-key "leader" "n N" "kb-narrow")
(define-key "leader" "n W" "kb-widen")
(define-key "leader" "n S" "kb-set-search-scope")

;; +code (LSP)
(define-key "leader" "c d" "lsp-goto-definition")
(define-key "leader" "c r" "lsp-find-references")
(define-key "leader" "c k" "lsp-hover")
(define-key "leader" "c x" "lsp-show-diagnostics")
(define-key "leader" "c a" "lsp-code-action")
(define-key "leader" "c R" "lsp-rename")
(define-key "leader" "c F" "lsp-range-format")
(define-key "leader" "c s" "lsp-status")
(define-key "leader" "c o" "lsp-symbol-outline")
(define-key "leader" "l p" "lsp-peek-definition")
(define-key "leader" "l r" "lsp-peek-references")

;; +open
(define-key "leader" "o t" "terminal")
(define-key "leader" "o T" "terminal-here")
(define-key "leader" "o r" "terminal-reset")
(define-key "leader" "o c" "terminal-close")

;; +command palette / ex-command (leader SPC / leader :)
(define-key "leader" "SPC" "command-palette")
(define-key "leader" ":" "enter-command-mode")

;; +collaboration
(set-group-name "leader" "C" "collaboration")
(define-key "leader" "C s" "collab-start")
(define-key "leader" "C c" "collab-connect")
(define-key "leader" "C d" "collab-disconnect")
(define-key "leader" "C i" "collab-status")
(define-key "leader" "C S" "collab-share")
(define-key "leader" "C y" "collab-sync")
(define-key "leader" "C D" "collab-doctor")
(define-key "leader" "C l" "collab-list")
(define-key "leader" "C j" "collab-join")
(define-key "leader" "C P" "collab-discover")

;; +identity (key rotation / recovery — ADR-040). Sensitive, infrequent ops kept under
;; a dedicated subgroup. `collab-recover-identity` needs args, so it stays `:`-only
;; (`:collab-recover-identity <recovery-key-dir> <old-fingerprint>`).
(set-group-name "leader" "C I" "identity")
(define-key "leader" "C I r" "collab-rotate-identity")
(define-key "leader" "C I k" "collab-register-recovery-key")

;; +kb-collaboration
(set-group-name "leader" "C K" "KB sharing")
(define-key "leader" "C K s" "kb-share")
(define-key "leader" "C K j" "kb-join")
(define-key "leader" "C K l" "kb-leave")
(define-key "leader" "C K r" "kb-list-remote")
(define-key "leader" "C K p" "kb-share-p2p")

;;; ── Shared `navigation` context ──────────────────────────────────────────
;;; Flavor-independent bindings for read-only nav buffers (dashboard, file tree,
;;; modules, help/KB, git-status, agenda, debug, shell). These buffers force
;;; Normal mode regardless of flavor, which left non-modal users facing vi keys
;;; they don't know and split leader access (doom: SPC only; non-modal: C-; only).
;;; The `navigation` keymap is kernel-created (parent `normal`) and the nav
;;; overlays parent onto it (chain: overlay → navigation → normal), so motions
;;; (j/k/gg/G/arrows) come free from `normal` while we add CUA movement and BOTH
;;; leader entries here — identical behavior in every flavor.
(define-key "navigation" "C-n" "move-down")
(define-key "navigation" "C-p" "move-up")
(define-key "navigation" "SPC" "leader-dispatch")
(define-key "navigation" "C-;" "leader-dispatch")

;; Route the dashboard (no dedicated overlay keymap) through `navigation`. The
;; overlay buffers (file-tree/modules/help/git-status/agenda/debug/shell-*) reach
;; it via their keymap parent instead.
(bind-context-keymap "kind" "dashboard" "navigation")

(provide-feature "keymap-leader-autoloads")
