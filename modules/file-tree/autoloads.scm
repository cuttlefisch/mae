;; file-tree/autoloads.scm — NERDTree-style file browser keybindings
;; Commands remain kernel builtins; this module owns the keymap + bindings.

;; Create the file-tree keymap with normal as parent (for fallback)
(define-keymap "file-tree" "normal")

;; Normal mode: toggle binding
(define-key "normal" "SPC f t" "file-tree-toggle")

;; Navigation
(define-key "file-tree" "j" "file-tree-down")
(define-key "file-tree" "k" "file-tree-up")
(define-key "file-tree" "gg" "file-tree-first")
(define-key "file-tree" "G" "file-tree-last")

;; Scroll
(define-key "file-tree" "C-e" "file-tree-scroll-down")
(define-key "file-tree" "C-y" "file-tree-scroll-up")
(define-key "file-tree" "C-d" "file-tree-half-page-down")
(define-key "file-tree" "C-u" "file-tree-half-page-up")

;; Actions
(define-key "file-tree" "<CR>" "file-tree-open")
(define-key "file-tree" "o" "file-tree-open")
(define-key "file-tree" "s" "file-tree-open-vsplit")
(define-key "file-tree" "i" "file-tree-open-hsplit")
(define-key "file-tree" "<Tab>" "file-tree-expand")
(define-key "file-tree" "<S-Tab>" "file-tree-global-cycle")
(define-key "file-tree" "x" "file-tree-close-parent")
(define-key "file-tree" "u" "file-tree-parent")
(define-key "file-tree" "C" "file-tree-cd")
(define-key "file-tree" "R" "file-tree-refresh")
(define-key "file-tree" "m a" "file-tree-create")
(define-key "file-tree" "d" "file-tree-delete")
(define-key "file-tree" "r" "file-tree-rename")
(define-key "file-tree" "q" "file-tree-toggle")
(define-key "file-tree" "?" "show-buffer-keys")

(provide-feature "file-tree-autoloads")
