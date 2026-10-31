;;; org-element-dump.el --- dump org-element's parse of a file as JSON lines  -*- lexical-binding: t -*-
;;
;; The conformance oracle for mae-org (ADR-112 D5).
;;
;; Usage 1: emacs -Q --batch -l org-element-dump.el FILE.org > FILE.jsonl
;;
;; One JSON object per ELEMENT (objects are O2's business): its type and
;; 0-based BYTE offsets (begin/end, contents_begin/contents_end), plus the
;; properties mae-org's typed views must agree on.
;;
;; Byte offsets are exact for the file as stored: the file is read with
;; `utf-8-unix', so no EOL conversion happens and a BOM is NOT stripped.
;; That makes the oracle honest about bytes, but it also means Emacs sees a
;; CR as content and a BOM as a character, which is not how Emacs decodes such
;; files for a user. mae-org treats CRLF as a line end and a BOM as trivia;
;; the Rust harness checks CRLF and BOM variants of every LF fixture against
;; the LF golden with shifted offsets instead (see tests/conformance.rs), and
;; fixtures stored with CR or a BOM are listed in divergences.toml.
(require 'org)
(require 'org-element)
(require 'json)

;; mae-org does no I/O (ADR-112 D1): `#+SETUPFILE' and `#+INCLUDE' are for a
;; caller-supplied resolver in a later phase. The oracle must not read them
;; either, or a setup file's `#+TODO' would make it disagree for a reason
;; mae-org cannot see. Org reads setup files through `org-file-contents'.
(advice-add 'org-file-contents :override (lambda (&rest _) ""))

(defun mae-dump--byte (pos)
  "0-based byte offset of buffer position POS (nil stays nil)."
  (and pos (1- (position-bytes pos))))

(defun mae-dump--props (el)
  (let ((type (org-element-type el)))
    (append
     (list (cons 'type (symbol-name type))
           (cons 'begin (mae-dump--byte (org-element-property :begin el)))
           (cons 'end (mae-dump--byte (org-element-property :end el)))
           (cons 'contents_begin (mae-dump--byte (org-element-property :contents-begin el)))
           (cons 'contents_end (mae-dump--byte (org-element-property :contents-end el))))
     (pcase type
       ('headline
        (list (cons 'level (org-element-property :level el))
              (cons 'raw_title (org-element-property :raw-value el))
              (cons 'todo (org-element-property :todo-keyword el))
              (cons 'priority (let ((p (org-element-property :priority el))) (and p (char-to-string p))))
              (cons 'tags (vconcat (org-element-property :tags el)))
              (cons 'commentedp (and (org-element-property :commentedp el) t))))
       ('drawer (list (cons 'name (org-element-property :drawer-name el))))
       ('node-property (list (cons 'key (org-element-property :key el))
                             (cons 'value (org-element-property :value el))))
       ('keyword (list (cons 'key (org-element-property :key el))
                       (cons 'value (org-element-property :value el))))
       ('special-block (list (cons 'block_type (org-element-property :type el))))
       ('src-block (list (cons 'language (org-element-property :language el))))
       ('item (list (cons 'bullet (org-element-property :bullet el))))))))

;; Usage 2 (what regen.sh uses; one Emacs for many files):
;;   emacs -Q --batch -l org-element-dump.el --manifest MANIFEST
;; where each MANIFEST line is "SOURCE<TAB>DESTINATION".

(defun mae-dump-file (file)
  "Return org-element's parse of FILE as JSON lines.
The first line is a self-check, {\"type\":\"#file\",...}: the buffer's
size in bytes and whether the buffer still equals the file after
`org-mode' ran. The Rust harness rejects a golden whose check fails, so
an oracle that parsed something other than the file cannot pass."
  (let ((coding-system-for-read 'utf-8-unix))
    (with-temp-buffer
      (insert-file-contents file)
      (let ((original (buffer-string)))
        ;; `org-inhibit-startup': no #+STARTUP actions. `align' would realign
        ;; tables IN THE BUFFER, so offsets would describe text that is not
        ;; the file (found on Worg). In-buffer #+TODO is still honoured.
        (let ((org-inhibit-startup t)) (org-mode))
        (let ((tree (org-element-parse-buffer 'element))
              (out '()))
          (org-element-map tree org-element-all-elements
            (lambda (el) (push (concat (json-encode (mae-dump--props el)) "\n") out))
            nil nil nil t)
          (concat (json-encode
                   (list (cons 'type "#file")
                         (cons 'bytes (1- (position-bytes (point-max))))
                         (cons 'unmodified (if (string= original (buffer-string)) t :json-false))))
                  "\n"
                  (apply #'concat (nreverse out))))))))

(defun mae-dump-manifest (manifest)
  "Dump every SOURCE<TAB>DESTINATION pair listed in MANIFEST."
  (with-temp-buffer
    (insert-file-contents manifest)
    (dolist (line (split-string (buffer-string) "\n" t))
      (let* ((pair (split-string line "\t"))
             (json (mae-dump-file (car pair))))
        (make-directory (file-name-directory (cadr pair)) t)
        (let ((coding-system-for-write 'utf-8-unix))
          (with-temp-file (cadr pair) (insert json)))))))

(let ((args command-line-args-left))
  (setq command-line-args-left nil)
  (if (equal (car args) "--manifest")
      (mae-dump-manifest (cadr args))
    (princ (mae-dump-file (car args)))))
