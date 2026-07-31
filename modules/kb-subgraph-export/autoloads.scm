;;; kb-subgraph-export/autoloads.scm — Export a KB subgraph to a standalone
;;; bilingual (EN/ES) HTML file (kb/adrs/0002 in bilingual-kb-export).
;;;
;;; The export logic itself is compiled Rust (mae-export's html_graph
;;; module, wired through mae-ai's execute_kb_export_subgraph_html) — per
;;; concept-design-philosophy.org's "no framework/no SDK" principle, this
;;; module is ONLY command/keybinding wiring over the already-registered
;;; (kb-export-subgraph-html ID PATH [DEPTH] [TRANSLATIONS] [TITLE])
;;; primitive, mirroring kb-graph-view's own shape exactly. That primitive
;;; queues onto the SAME code path the kb_export_subgraph_html MCP tool
;;; calls (CLAUDE.md principle #3: human and AI drive identical code paths).
;;;
;;; ID and PATH are required by the export itself, and mae's Scheme runtime
;;; has no minibuffer/read-string primitive to prompt for free text — so
;;; this module does not invent one. Its one convenience command instead
;;; composes two already-registered primitives: it reads whatever node is
;;; currently centered in the native KB graph view via (kb-graph-view-state)
;;; and exports THAT node/depth, to a predictable path derived from the
;;; node id. Direct, arbitrary ID/PATH/TRANSLATIONS export stays available
;;; by calling (kb-export-subgraph-html ...) itself from Scheme config or
;;; the AI's MCP tool — this command is a shortcut, not the only entry point.

;;; @module: kb-subgraph-export
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: kb-subgraph-export-autoloads

(define (kb-subgraph-export-current-fn)
  (let ((gv (kb-graph-view-state)))
    (if (not gv)
        (set-status "kb-subgraph-export: open the KB graph view first (SPC h g), then retry")
        (let* ((center (car gv))
               (depth (cadr gv))
               (path (string-append "kb-export-" center ".html")))
          (kb-export-subgraph-html center path depth)))))

(define-command "kb-subgraph-export-current"
  "Export the KB graph view's currently centered node's subgraph to a standalone HTML file"
  "kb-subgraph-export-current-fn")

;; Companion to kb-graph-view's own h/j/k/l/Enter/g r/o/q/Escape/? bindings
;; on the "graph" keymap (defined by that module, a declared dependency
;; above) — "e" for export, following the graph view since a subgraph
;; export is naturally "export what I'm currently looking at."
(define-key "graph" "e" "kb-subgraph-export-current")

(provide-feature "kb-subgraph-export-autoloads")
