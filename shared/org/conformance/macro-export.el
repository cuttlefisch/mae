;;; macro-export.el --- ox-html body exports for the macro oracle  -*- lexical-binding: t; -*-
;;
;; Usage: emacs -Q --batch -l macro-export.el IN-DIR OUT-DIR
;;
;; Exports every IN-DIR/*.org as `org-export-as' 'html, body only, into
;; OUT-DIR/NAME.html, or writes the error to OUT-DIR/NAME.error. No file is
;; visited (so `input-file' and `modification-time' are undefined, as for
;; any buffer), and no code is ever evaluated: `org-babel-execute-src-block'
;; is replaced and every block is `:eval never-export'. IDs are not
;; tracked. Used by tests/render_macros_oracle.rs.

(setq org-id-track-globally nil)
(require 'org)
(require 'ox-html)
(require 'ob-exp)
(advice-add 'org-babel-execute-src-block :override (lambda (&rest _) nil))
(setq org-export-use-babel t
      org-confirm-babel-evaluate nil
      org-export-with-broken-links t
      org-html-validate-link nil
      org-export-with-toc nil
      org-export-with-section-numbers nil)
(setq org-babel-default-header-args
      (cons '(:eval . "never-export") org-babel-default-header-args))
(setq org-babel-default-inline-header-args
      (cons '(:eval . "never-export") org-babel-default-inline-header-args))

(let* ((in (nth 0 command-line-args-left))
       (out (nth 1 command-line-args-left)))
  (setq command-line-args-left nil)
  (dolist (f (directory-files in t "\\.org\\'"))
    (let* ((base (file-name-sans-extension (file-name-nondirectory f)))
           (res (with-temp-buffer
                  (insert-file-contents f)
                  (org-mode)
                  (condition-case err
                      (cons 'ok (org-export-as 'html nil nil t nil))
                    (error (cons 'error (format "%S" err)))))))
      (with-temp-file (expand-file-name
                       (concat base (if (eq (car res) 'ok) ".html" ".error"))
                       out)
        (insert (cdr res))))))

;;; macro-export.el ends here
