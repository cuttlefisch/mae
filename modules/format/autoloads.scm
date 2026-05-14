;;; format/autoloads.scm — formatter keybindings and on-save hook

;;; @module: format
;;; @version: 0.1.0
;;; @stability: experimental
;;; @provides: format-autoloads

(define-key "normal" "SPC c f" "format-buffer")

;; When +onsave flag is set, register the before-save hook
(when-flag "format" "onsave"
  (add-hook! "before-save" "format-before-save"))

(provide-feature "format-autoloads")
