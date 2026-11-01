;;; leak-export.el --- ox-html body exports for the leak oracle  -*- lexical-binding: t; -*-
;;
;; Usage: emacs -Q --batch -l leak-export.el IN-DIR OUT-DIR
;;
;; Exports every IN-DIR/*.org, in `string<' order of the names, as
;; `org-export-as' 'html, body only, into OUT-DIR/NAME.html, or writes the
;; error to OUT-DIR/NAME.error. Each result is written as soon as it is
;; known, so a caller that kills a hung run knows which document hung: the
;; first one with no result. No file is visited, and no code is ever
;; evaluated: `org-babel-execute-src-block' is replaced and every block is
;; `:eval never-export'. IDs are not tracked, and every file Org may write
;; goes under HOME (the caller's scratch directory). Links Org cannot
;; resolve (KB links) are exported as below rather than failing the
;; export. Used by tests/render_leak_oracle.rs and conformance/cmp.sh.

(setq org-id-track-globally nil
      org-id-locations-file (expand-file-name "org-id-locations" (getenv "HOME")))
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
;; A KB link (`id:') is MAE semantics (ADR-030): Org cannot export one
;; without global ID tracking. It is exported as a link to an opaque
;; target with the same description, which is what MAE's KB link must not
;; exceed: nothing of the path (the id, `?rel=', `&w=', a store-form
;; description) shows. The custom :export runs before ox-html resolves the
;; link, and only for a link Org parsed: id text inside code stays text.
;; `org-id-find' (which ox calls to collect the file's ids, and which
;; fails without tracking) finds nothing: no ID database is read.
(advice-add 'org-id-find :override (lambda (&rest _) nil))
;; A fuzzy link that resolves to nothing in the file (`[[name][desc]]' with
;; no such target or headline, or one in a pruned subtree) is a KB link to
;; MAE (ADR-030), and a `[[#custom-id]]' that resolves to nothing is MAE's
;; unresolved link. With `org-export-with-broken-links' t Org would drop it,
;; description and all; it is exported as ox-html exports a fuzzy link that
;; points nowhere, `<i>DESC</i>', except that a link without a description
;; shows a placeholder, never its path (the path names what it points at).
(defun leak-export-link (orig link desc info)
  "Call ORIG on LINK, DESC and INFO unless LINK is local and resolves nowhere."
  (if (condition-case nil
          (pcase (org-element-property :type link)
            ("fuzzy" (org-export-resolve-fuzzy-link link info) nil)
            ("custom-id" (org-export-resolve-id-link link info) nil)
            (_ nil))
        (org-link-broken t))
      (format "<i>%s</i>" (or desc "[?]"))
    (funcall orig link desc info)))
(advice-add 'org-html-link :around #'leak-export-link)
(org-link-set-parameters
 "id" :export (lambda (_path desc _backend _info)
                (format "<a href=\"https://kb.invalid/\">%s</a>"
                        (or desc "https://kb.invalid/"))))
(setq org-babel-default-header-args
      (cons '(:eval . "never-export") org-babel-default-header-args))
(setq org-babel-default-inline-header-args
      (cons '(:eval . "never-export") org-babel-default-inline-header-args))

(defun leak-export-one (file)
  "Export FILE; return (ok . HTML) or (error . MESSAGE)."
  (with-temp-buffer
    (insert-file-contents file)
    (org-mode)
    (condition-case err
        (cons 'ok (org-export-as 'html nil nil t nil))
      (error (cons 'error (format "%S" err))))))

(let* ((in (nth 0 command-line-args-left))
       (out (nth 1 command-line-args-left)))
  (setq command-line-args-left nil)
  (dolist (f (sort (directory-files in t "\\.org\\'") #'string<))
    (let* ((base (file-name-sans-extension (file-name-nondirectory f)))
           (res (leak-export-one f)))
      (with-temp-file (expand-file-name
                       (concat base (if (eq (car res) 'ok) ".html" ".error"))
                       out)
        (set-buffer-file-coding-system 'utf-8-unix)
        (insert (cdr res))))))

;;; leak-export.el ends here
