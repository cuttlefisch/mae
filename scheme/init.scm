;; mae init.scm — bootstrap for the Modern AI Editor Scheme runtime
;; This file is loaded when the editor starts.
;;
;; MAE is a lisp machine: every command the user or AI can run is
;; callable from Scheme. The same primitives are available to:
;;   - init.scm (this file, loaded on startup)
;;   - :eval <code> (interactive evaluation from command mode)
;;   - SPC e l / SPC e b / SPC e r (eval line/buffer/region)
;;   - AI agent (via the tool-calling interface)
;;   - Scheme-defined commands (define-command + define-key)

;; ── Available Scheme primitives ─────────────────────────────────
;;
;; Configuration:
;;   (define-key MAP KEY COMMAND)    — bind a key in a keymap
;;   (define-command NAME DOC FN)    — register a user command
;;   (set-theme NAME)                — switch color theme
;;   (set-status MSG)                — display in status bar
;;   (set-option! KEY VALUE)         — set an editor option at runtime
;;
;; Live editing:
;;   (buffer-insert TEXT)            — insert at cursor
;;   (cursor-goto ROW COL)          — move cursor (0-indexed)
;;   (buffer-line N)                 — read line N (0-indexed)
;;   (open-file PATH)               — open file in new buffer
;;   (run-command NAME)              — dispatch any registered command
;;   (message TEXT)                  — append to *Messages* log
;;
;; Read-only state (injected before each eval):
;;   *buffer-name*                   — current buffer name
;;   *buffer-text*                   — full buffer contents
;;   *buffer-modified?*              — unsaved changes?
;;   *buffer-line-count*             — number of lines
;;   *buffer-count*                  — number of open buffers
;;   *cursor-row*                    — cursor row (0-indexed)
;;   *cursor-col*                    — cursor column (0-indexed)
;;   *mode*                          — current mode name string

;; ── Example: custom commands ────────────────────────────────────

;; Insert a timestamp at the cursor.
(define (insert-timestamp)
  (buffer-insert
    (string-append "["
      (number->string 2026) "-04-17"  ; Steel lacks date libs
      "]")))

(define-command "insert-timestamp"
  "Insert a timestamp at the cursor"
  "insert-timestamp")

;; Show buffer info in the status bar.
(define (buffer-info)
  (set-status
    (string-append *buffer-name*
      " | " (number->string *buffer-line-count*) " lines"
      " | " (if *buffer-modified?* "[+]" "")
      " | row " (number->string *cursor-row*)
      ":" (number->string *cursor-col*))))

(define-command "buffer-info"
  "Show buffer name, line count, and cursor position"
  "buffer-info")

;; ── AI agent configuration ───────────────────────────────────
;; Set the command for SPC a a (open-ai-agent).
;; Default: "claude" (Claude Code). Other examples: "aider", "copilot".
;; (set-option! "ai_editor" "claude")

;; ── Example: keybinding customization ───────────────────────────

;; Uncomment to remap:
;; (define-key "normal" "Q" "quit")
;; (define-key "normal" "SPC i t" "insert-timestamp")
;; (define-key "normal" "SPC i i" "buffer-info")
