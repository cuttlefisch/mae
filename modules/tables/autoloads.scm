;;; tables/autoloads.scm — table editing keybindings for org + markdown
;;; Commands remain kernel builtins; this module owns the keybindings.

;;; @module: tables
;;; @version: 0.1.0
;;; @stability: stable
;;; @provides: tables-autoloads

;; Org-mode table bindings — the `SPC m b` group. Bound into the kernel-created
;; `org-leader` local-leader keymap (as `m b …`), NOT into the `org` keymap with
;; a literal `SPC` prefix (which shadowed the global leader). Kernel-created, so
;; this works regardless of module load order.
(define-key "org-leader" "m b a" "table-align")
(define-key "org-leader" "m b i r" "table-insert-row")
(define-key "org-leader" "m b d r" "table-delete-row")
(define-key "org-leader" "m b i c" "table-insert-column")
(define-key "org-leader" "m b d c" "table-delete-column")
(set-group-name "org-leader" "m b" "+table")

;; Markdown table bindings (same local-leader structure).
(define-key "markdown-leader" "m b a" "table-align")
(define-key "markdown-leader" "m b i r" "table-insert-row")
(define-key "markdown-leader" "m b d r" "table-delete-row")
(define-key "markdown-leader" "m b i c" "table-insert-column")
(define-key "markdown-leader" "m b d c" "table-delete-column")
(set-group-name "markdown-leader" "m b" "+table")

(provide-feature "tables-autoloads")
