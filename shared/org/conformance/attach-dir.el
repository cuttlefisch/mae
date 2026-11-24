;;; attach-dir.el --- org-attach's directory for every attachment link  -*- lexical-binding: t; -*-
;;
;; Usage: emacs -Q --batch -l attach-dir.el IN-DIR OUT-FILE
;;
;; For every IN-DIR/*.org, in `string<' order of the names, and every link
;; of Org's `attachment' type in it, in document order, writes one line to
;; OUT-FILE:
;;
;;   FILE TAB INDEX TAB DIR-PROP TAB ID TAB RESULT
;;
;; DIR-PROP is "t" when `DIR' or `ATTACH_DIR' is set at the link (with
;; inheritance), else "nil"; ID is `org-entry-get' of ID with inheritance
;; (or "nil"); RESULT is `org-attach-dir' with NO-FS-CHECK, relative to the
;; file's directory with a trailing slash, or "nil", or "ERROR". Nothing
;; exists under data/, so `org-attach-dir-from-id' returns its first
;; candidate: the directory MAE derives (ADR-115 E2). `org-attach-use-inheritance'
;; is t, as E2 fixes it. Used by tests/attach_dir.rs (run locally, ignored).

(require 'org)
(require 'org-attach)
(setq org-attach-use-inheritance t
      org-id-track-globally nil)

(defun attach-dir--line (file index)
  "The line for the attachment link at point in FILE, the INDEXth."
  (let* ((dir-prop (or (org-entry-get nil "DIR" t)
                       (org-entry-get nil "ATTACH_DIR" t)))
         (id (org-entry-get nil "ID" t))
         (result (condition-case nil
                     (let ((d (org-attach-dir nil t)))
                       (if d
                           (file-name-as-directory
                            (file-relative-name d (file-name-directory file)))
                         "nil"))
                   (error "ERROR"))))
    (format "%s\t%d\t%s\t%s\t%s\n"
            (file-name-nondirectory file) index (if dir-prop "t" "nil")
            (or id "nil") result)))

(let* ((in (nth 0 command-line-args-left))
       (out (nth 1 command-line-args-left))
       (lines nil))
  (setq command-line-args-left nil)
  (dolist (file (sort (directory-files in t "\\.org\\'") #'string<))
    (with-current-buffer (find-file-noselect file)
      (let ((index 0))
        (org-element-map (org-element-parse-buffer) 'link
          (lambda (l)
            (when (equal (org-element-property :type l) "attachment")
              (goto-char (org-element-begin l))
              (push (attach-dir--line file index) lines)
              (setq index (1+ index))))))
      (kill-buffer)))
  (with-temp-file out
    (apply #'insert (nreverse lines))))
