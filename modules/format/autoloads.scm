;; format module autoloads — formatter keybindings and on-save hook

(define-key "normal" "SPC c f" "format-buffer")

;; When +onsave flag is set, register the before-save hook
(when-flag "format" "onsave"
  (add-hook! "before-save" "format-before-save"))

(provide-feature "format-autoloads")
