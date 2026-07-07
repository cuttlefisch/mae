;; init.scm — MAE (Modern AI Editor) user configuration
;;
;; MAE is a lisp machine. Every primitive is callable from Scheme by
;; both the human user and the AI agent — they are peer actors sharing
;; the same API surface. Customize this file, then reload with :source
;; or restart the editor.
;;
;; Evaluation entry points:
;;   init.scm        — loaded on startup (this file)
;;   :eval <expr>    — interactive eval from command mode
;;   SPC e l/b/r     — eval line / buffer / region
;;   AI agent        — tool-calling interface (same primitives)

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 1. UI
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

;; Font size for the GUI backend (6–72, ignored in TUI).
;; (set-option! "font-size" "14.0")

;; Font families for GUI.
;; (set-option! "font-family" "JetBrainsMono Nerd Font")
;; (set-option! "icon-font-family" "Symbols Nerd Font Mono")

;; Splash screen art variant.
;; (set-option! "splash-art" "bat")

;; Show FPS overlay (useful for profiling the GUI backend).
;; (set-option! "show-fps" "false")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 2. Theme
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Available: default, light-ansi, dracula, gruvbox-dark, gruvbox-light,
;;            catppuccin-mocha, solarized-dark, one-dark

(set-theme "catppuccin-mocha")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 3. Editor options
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Uncomment to override defaults. Use :describe-option <name> for docs.
;; Use :set-save <name> <value> to persist changes to config.toml.

;; Line numbers
(set-option! "line-numbers" "true")
;; (set-option! "relative-line-numbers" "false")

;; Word wrap
;; (set-option! "word-wrap" "false")
;; (set-option! "break-indent" "true")
;; (set-option! "show-break" "↪ ")
;; (set-option! "org-hide-emphasis-markers" "false")

;; Link and markup rendering (Emacs org-link-descriptive pattern)
;; (set-option! "link-descriptive" "true")  ; show labels instead of raw [label](url)
;; (set-option! "render-markup" "true")     ; apply inline bold/italic/code styling

;; Clipboard: "unnamed" (vim default) or "unnamedplus" (system)
;; (set-option! "clipboard-mode" "unnamed")

;; Search
;; (set-option! "ignorecase" "false")   ; case-insensitive search
;; (set-option! "smartcase" "false")    ; if ignorecase + uppercase in pattern → case-sensitive

;; Debug stats in status bar
;; (set-option! "debug-mode" "false")

;; Auto-restore previous session on startup
;; (set-option! "restore-session" "false")

;; Autosave: interval in seconds (0 = disabled).
;; Saves all modified file-backed buffers after idle timeout.
;; (set-option! "autosave-interval" "300")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 4. Keybindings
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Maps: "normal", "insert", "visual"
;; Keys: single chars, C-x (ctrl), or SPC-prefixed leader sequences.
;;
;; Insert-mode C-t/C-d: indent/dedent current line (vim default).
;; To restore Emacs C-d (delete-forward): (set-option! "insert-ctrl-d" "delete-forward")
;; (set-option! "insert-ctrl-d" "dedent")

;; (define-key "normal" "Q" "quit")
;; (define-key "normal" "SPC i t" "insert-timestamp")
;; (define-key "normal" "SPC i i" "buffer-info")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 5. AI configuration
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; AI API provider for SPC a p (chat): claude, openai, gemini, ollama, deepseek
;; (set-option! "ai-provider" "claude")
;; (set-option! "ai-model" "claude-sonnet-4-5")
;;
;; Shell command to retrieve the API key (e.g. from pass, 1Password):
;; (set-option! "ai-api-key-command" "pass show my-provider/api-key")
;;
;; Custom API base URL (for self-hosted / proxy endpoints):
;; (set-option! "ai-base-url" "http://localhost:8080/v1")
;;
;; AI agent command for SPC a a (shell). Default: "claude" (Claude Code).
;; This launches a CLI agent in a terminal — NOT the API chat.
;; Other examples: "gemini" (Gemini CLI), "aider", "copilot".
;; (set-option! "ai-editor" "claude")

;; AI operating mode: "standard" (manual), "plan" (drafting), "auto-accept" (hands-free)
;; (set-option! "ai-mode" "standard")
;;
;; AI prompt profile: "pair-programmer", "explorer", "planner", "reviewer"
;; (set-option! "ai-profile" "pair-programmer")
;;
;; Permission tier: ReadOnly, Write, Shell, Privileged
;; Controls what the AI agent is allowed to do. Can also be set via
;; MAE_AI_PERMISSIONS env var or config.toml.
;; (set-option! "ai-tier" "ReadOnly")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 6. Daemon (KB persistence & hosting)
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; ADR-035: the embedded, in-process KB store is the FLOOR — daemon_mode
;; "off" (the default) needs no daemon at all. The daemon (mae-daemon) is
;; opt-in and only worth it for a specific value: SHARED across multiple
;; frontends, OUTLIVES this editor session, COORDINATES peers, or adds
;; DURABILITY beyond the local store. This is a *separate* axis from
;; §7 Collaborative Editing below — that governs live buffer sync (yrs
;; CRDT) over the daemon's TCP port; this governs whether KB reads/writes
;; (kb-find, kb-search, notes) route through the daemon's Unix socket at
;; all. Run `:eval (daemon-status)` any time to see what's active and why.
;;
;; off        — in-process embedded KB only, no daemon (default)
;; on-demand  — attach to a running daemon, or auto-spawn a co-located
;;              one — persistence/collab without extra ceremony
;; shared     — attach to an existing OS-supervised/remote daemon only,
;;              never auto-spawn — for multi-session/multi-machine setups
;; (set-option! "daemon-mode" "off")

;; Unix socket path for daemon communication.
;; Empty = auto ($XDG_RUNTIME_DIR/mae-daemon.sock, matching the daemon's
;; default bind path). Only needed for a non-default daemon location.
;; (set-option! "daemon-socket" "")

;; Max nodes cached in the daemon-backed LRU query layer (0 = unbounded).
;; (set-option! "daemon-cache-size" "200")

;; EXPERIMENTAL (ADR-029): when a local daemon hosts the primary KB, host
;; + thin-start it there (CRDT source of truth) instead of the editor's
;; on-disk store. Known gaps: agenda + ranked KB search aren't yet
;; daemon-routed and read empty under a thin mirror.
;; (set-option! "daemon-default" "false")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 7. Collaborative Editing
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Connect to a collaborative daemon for multi-user buffer sync (yrs
;; CRDT) — independent of §6 above; requires a reachable mae-daemon.
;; See :help concept:collab-architecture for details.
;; (set-option! "collab-server-address" "127.0.0.1:9473")
;; (set-option! "collab-auto-connect" "true")
;; (set-option! "collab-user-name" "alice")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 8. Shell
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; The embedded shell (SPC o s) runs your $SHELL inside the editor.
;; Exit shell-insert mode with the configured exit sequence (default:
;; Ctrl-\ Ctrl-n). Remap via the shell-insert keymap if desired.
;;
;; Shell hooks and functions:
;;   (shell-send-input BUF-IDX TEXT) — send text to a shell buffer
;;   (shell-cwd BUF-IDX)            — get shell working directory
;;   (shell-read-output BUF-IDX)    — read recent shell output

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 9. Hooks
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Available hooks:
;;   before-save, after-save, buffer-open, buffer-close,
;;   mode-change, file-changed-on-disk, app-start, app-exit,
;;   focus-in, focus-out

;; Log every file open to *Messages*.
;; (define (on-buffer-open)
;;   (message (string-append "Opened: " *buffer-name*)))
;; (add-hook! "buffer-open" "on-buffer-open")

;; Notify on external file changes.
;; (define (on-file-changed)
;;   (message (string-append "File changed on disk: " *buffer-name*)))
;; (add-hook! "file-changed-on-disk" "on-file-changed")

;; Log mode transitions.
;; (define (on-mode-change)
;;   (message (string-append "Mode → " *mode*)))
;; (add-hook! "mode-change" "on-mode-change")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; 10. Custom commands
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

;; Insert a timestamp at the cursor.
;; NOTE: Static date — mae-scheme has no date/time library yet.
;; Workaround: use shell-read-output to get the real date, e.g.
;;   (buffer-insert (shell-read-output 0))  after sending "date +%Y-%m-%d"
(define (insert-timestamp)
  (buffer-insert
    (string-append "["
      (number->string 2026) "-04-26"
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

;; Scratch note — open a throw-away buffer for quick notes.
;; (define (scratch-note)
;;   (run-command "scratch"))
;; (define-command "scratch-note" "Open scratch buffer" "scratch-note")

;; ── Knowledge Base ──────────────────────────────────────────────────
;; Set a default directory for user-created notes (org-roam-directory).
;; Uncomment and adjust the path to enable file-backed notes via SPC n c.
;; (set-option! "kb-notes-dir" "~/RoamNotes")

;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
;; Local Variables — add machine-local overrides below this line.
;; ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
