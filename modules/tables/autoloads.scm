;; tables/autoloads.scm — table editing keybindings for org + markdown
;; Commands remain kernel builtins; this module owns the keybindings.

;; Org-mode table bindings (SPC m b group)
(define-key "org" "SPC m b a" "table-align")
(define-key "org" "SPC m b i r" "table-insert-row")
(define-key "org" "SPC m b d r" "table-delete-row")
(define-key "org" "SPC m b i c" "table-insert-column")
(define-key "org" "SPC m b d c" "table-delete-column")

;; Markdown table bindings (same leader structure)
(define-key "markdown" "SPC m b a" "table-align")
(define-key "markdown" "SPC m b i r" "table-insert-row")
(define-key "markdown" "SPC m b d r" "table-delete-row")
(define-key "markdown" "SPC m b i c" "table-insert-column")
(define-key "markdown" "SPC m b d c" "table-delete-column")

(provide-feature "tables-autoloads")
