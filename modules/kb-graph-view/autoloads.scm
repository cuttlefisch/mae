;;; kb-graph-view/autoloads.scm — Native KB graph view keybindings (Part C Phase 1)
;;;
;;; Registers the "graph" buffer-local keymap (parented on navigation, like
;;; debug/kb-sharing) and a SPC h g leader entry to open it. The view itself
;;; (BufferKind::Graph, GraphView, force-directed layout) lives in the
;;; kernel — this module only wires human-facing keybindings onto the SAME
;;; Editor::kb_graph_view_* methods / (kb-graph-view-*) Scheme primitives the
;;; AI's kb_graph_view_* MCP tools call (CLAUDE.md principle #3: human and AI
;;; drive identical code paths, not parallel ones).

;;; @module: kb-graph-view
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: kb-graph-view-autoloads

;; Zero-arg command wrappers around the (kb-graph-view-navigate DIR) /
;; (kb-graph-view-open [id] [depth]) primitives — `define-key` binds a
;; COMMAND name, and these two primitives take arguments a keybinding can't
;; pass. The other four primitives (close/refresh/select-current) are
;; already zero-arg, so they're registered as commands directly, no wrapper
;; needed.
(define (kb-graph-view-navigate-up-fn) (kb-graph-view-navigate "up"))
(define (kb-graph-view-navigate-down-fn) (kb-graph-view-navigate "down"))
(define (kb-graph-view-navigate-left-fn) (kb-graph-view-navigate "left"))
(define (kb-graph-view-navigate-right-fn) (kb-graph-view-navigate "right"))
(define (kb-graph-view-open-default-fn) (kb-graph-view-open))

(define-command "kb-graph-view-navigate-up" "Move KB graph view selection up" "kb-graph-view-navigate-up-fn")
(define-command "kb-graph-view-navigate-down" "Move KB graph view selection down" "kb-graph-view-navigate-down-fn")
(define-command "kb-graph-view-navigate-left" "Move KB graph view selection left" "kb-graph-view-navigate-left-fn")
(define-command "kb-graph-view-navigate-right" "Move KB graph view selection right" "kb-graph-view-navigate-right-fn")
(define-command "kb-graph-view-open-default" "Open the native KB graph view" "kb-graph-view-open-default-fn")
(define-command "kb-graph-view-close" "Close the native KB graph view" "kb-graph-view-close")
(define-command "kb-graph-view-refresh" "Refresh the native KB graph view in place" "kb-graph-view-refresh")
(define-command "kb-graph-view-select-current" "Navigate the companion window to the selected graph node" "kb-graph-view-select-current")

;; Buffer-local keymap inheriting read-only navigation (base j/k/etc.).
(define-keymap "graph" "navigation")

(define-key "graph" "h" "kb-graph-view-navigate-left")
(define-key "graph" "j" "kb-graph-view-navigate-down")
(define-key "graph" "k" "kb-graph-view-navigate-up")
(define-key "graph" "l" "kb-graph-view-navigate-right")
(define-key "graph" "Enter" "kb-graph-view-select-current")
(define-key "graph" "g r" "kb-graph-view-refresh")
(define-key "graph" "q" "kb-graph-view-close")
(define-key "graph" "Escape" "kb-graph-view-close")
(define-key "graph" "?" "show-buffer-keys")

;; SPC h g leader entry (appears in every flavor's keypad's +help submenu).
(define-key "leader" "h g" "kb-graph-view-open-default")

(provide-feature "kb-graph-view-autoloads")
