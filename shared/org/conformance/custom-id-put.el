;;; custom-id-put.el --- where Org puts a headline's CUSTOM_ID  -*- lexical-binding: t -*-
;;
;; Usage (from the repository root):
;;   emacs -Q --batch -l shared/org/conformance/custom-id-put.el
;;
;; For every shared/org/tests/custom-id/NAME.org (not *.expected.org), writes
;; NAME.expected.org: the file after `org-entry-put' sets CUSTOM_ID on every
;; headline that has neither a non-empty CUSTOM_ID nor a non-empty ID, in
;; document order, the N-th (from 1) getting the UUID
;; 00000000-0000-4000-8000-NNNNNNNNNNNN (N in hex, 12 digits).
;; tests/custom_id.rs gives mae_org::custom_id_edits the same values and
;; requires the same bytes. The file is read and written with its own line
;; endings (`last-coding-system-used'), as visiting and saving it would.

(require 'org)

(defun custom-id-put--uuid (n)
  (format "00000000-0000-4000-8000-%012x" n))

(defun custom-id-put--file (in out)
  (with-temp-buffer
    (let ((coding-system-for-read 'utf-8))
      (insert-file-contents in))
    (let ((coding last-coding-system-used)
          (n 0))
      (org-mode)
      (org-map-entries
       (lambda ()
         (unless (or (org-string-nw-p (org-entry-get nil "CUSTOM_ID"))
                     (org-string-nw-p (org-entry-get nil "ID")))
           (setq n (1+ n))
           (org-entry-put nil "CUSTOM_ID" (custom-id-put--uuid n))))
       nil nil)
      (let ((coding-system-for-write coding))
        (write-region nil nil out)))))

(let ((dir (expand-file-name "shared/org/tests/custom-id/")))
  (dolist (in (directory-files dir t "\\.org\\'"))
    (unless (string-suffix-p ".expected.org" in)
      (custom-id-put--file
       in (concat (file-name-sans-extension in) ".expected.org"))
      (message "%s" (file-name-nondirectory in)))))
