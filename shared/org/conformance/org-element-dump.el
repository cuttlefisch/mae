;;; org-element-dump.el --- dump org-element's parse of a file as JSON lines  -*- lexical-binding: t -*-
;;
;; The conformance oracle for mae-org (ADR-112 D5).
;;
;; Usage 1: emacs -Q --batch -l org-element-dump.el FILE.org > FILE.jsonl
;; Usage 2 (what regen.sh uses; one Emacs for many files):
;;   emacs -Q --batch -l org-element-dump.el --manifest MANIFEST
;; where each MANIFEST line is "SOURCE<TAB>DESTINATION".
;;
;; One JSON object per ELEMENT (objects are O2's business): its type, its
;; parent's type and begin, and 0-based BYTE offsets INTO THE FILE AS STORED
;; (begin/end, contents_begin/contents_end), plus the properties mae-org's
;; typed views must agree on.
;;
;; Decoding is what `find-file' does: UTF-8 with the line-ending convention
;; detected from the whole file (dos only if every LF follows a CR; mac if
;; there is no LF but a CR; otherwise unix, where a CR is content) and a
;; leading BOM stripped. Offsets are then mapped back to the file's bytes: the
;; BOM's 3 bytes, plus, in a dos file, one CR per line ending before the
;; offset. So the oracle sees what an Org user sees, and its numbers are still
;; exact bytes. The first output line is a self-check (see `mae-dump-file').
(require 'org)
(require 'org-element)
(require 'json)
(require 'cl-lib)

;; mae-org does no I/O (ADR-112 D1): `#+SETUPFILE' and `#+INCLUDE' are for a
;; caller-supplied resolver in a later phase. The oracle must not read them
;; either, or a setup file's `#+TODO' would make it disagree for a reason
;; mae-org cannot see. Org reads setup files through `org-file-contents'.
(advice-add 'org-file-contents :override (lambda (&rest _) ""))

(defvar mae-dump--bom 0 "Bytes of BOM stripped from the current file.")
(defvar mae-dump--crs nil
  "In a dos file, the sorted buffer positions of every LF (each was CRLF).")

(defun mae-dump--crs-before (pos)
  "How many CRs the decoder removed before buffer position POS."
  (if (not mae-dump--crs) 0
    (let ((lo 0) (hi (length mae-dump--crs)))
      (while (< lo hi)
        (let ((mid (/ (+ lo hi) 2)))
          (if (< (aref mae-dump--crs mid) pos) (setq lo (1+ mid)) (setq hi mid))))
      lo)))

(defun mae-dump--byte (pos)
  "Byte offset in the stored file of buffer position POS (nil stays nil)."
  (and pos (+ mae-dump--bom (1- (position-bytes pos)) (mae-dump--crs-before pos))))

(defun mae-dump--parent (el)
  (let ((p (org-element-property :parent el)))
    (list (cons 'parent_type (if p (symbol-name (org-element-type p)) "none"))
          (cons 'parent_begin (if (and p (not (eq (org-element-type p) 'org-data)))
                                  (mae-dump--byte (org-element-property :begin p))
                                0)))))

(defun mae-dump--props (el)
  (let ((type (org-element-type el)))
    (append
     (list (cons 'type (symbol-name type))
           (cons 'begin (mae-dump--byte (org-element-property :begin el)))
           (cons 'end (mae-dump--byte (org-element-property :end el)))
           (cons 'contents_begin (mae-dump--byte (org-element-property :contents-begin el)))
           (cons 'contents_end (mae-dump--byte (org-element-property :contents-end el))))
     (mae-dump--parent el)
     (pcase type
       ('headline
        (list (cons 'level (org-element-property :level el))
              (cons 'true_level (org-element-property :true-level el))
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

(defun mae-dump--decode (file)
  "Insert FILE as `find-file' decodes it; set the offset-mapping state.
Returns the line-ending convention: 0 unix, 1 dos, 2 mac."
  (let ((coding-system-for-read 'utf-8-auto))
    (insert-file-contents file))
  ;; An eol type that is still a vector means the decoder never had to
  ;; decide (e.g. no line ending at all), so nothing was converted: unix.
  ;; The #file self-check (byte count, eol) would catch a wrong guess.
  (let ((eol (let ((e (coding-system-eol-type last-coding-system-used))) (if (integerp e) e 0))))
    (setq mae-dump--bom (if (eq (coding-system-get last-coding-system-used :bom) t) 3 0))
    (setq mae-dump--crs
          (and (eql eol 1)
               (let (acc)
                 (save-excursion
                   (goto-char (point-min))
                   (while (search-forward "\n" nil t) (push (1- (point)) acc)))
                 (vconcat (nreverse acc)))))
    eol))

(defun mae-dump-file (file)
  "Return org-element's parse of FILE as JSON lines.
The first line is a self-check, {\"type\":\"#file\",...}: the size of the
stored file as the offset mapping computes it, the detected line-ending
convention, and whether the buffer still equals the decoded file after
`org-mode' ran. The Rust harness rejects a golden whose size does not
match the file or whose buffer was modified, so an oracle that parsed
something other than the file cannot pass."
  (with-temp-buffer
    (let* ((eol (mae-dump--decode file))
           (original (buffer-string)))
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
                       (cons 'bytes (mae-dump--byte (point-max)))
                       (cons 'eol (aref ["unix" "dos" "mac"] eol))
                       (cons 'unmodified (if (string= original (buffer-string)) t :json-false))))
                "\n"
                (apply #'concat (nreverse out)))))))

(defun mae-dump-file-safely (file)
  "Like `mae-dump-file', but an error becomes a #error line, so one bad
file cannot abort a manifest run."
  (condition-case err
      (mae-dump-file file)
    (error (concat (json-encode (list (cons 'type "#error")
                                      (cons 'message (error-message-string err))))
                   "\n"))))

(defun mae-dump-manifest (manifest)
  "Dump every SOURCE<TAB>DESTINATION pair listed in MANIFEST."
  (with-temp-buffer
    (insert-file-contents manifest)
    (dolist (line (split-string (buffer-string) "\n" t))
      (let* ((pair (split-string line "\t"))
             (json (mae-dump-file-safely (car pair))))
        (make-directory (file-name-directory (cadr pair)) t)
        (let ((coding-system-for-write 'utf-8-unix))
          (with-temp-file (cadr pair) (insert json)))))))

(let ((args command-line-args-left))
  (setq command-line-args-left nil)
  (if (equal (car args) "--manifest")
      (mae-dump-manifest (cadr args))
    (princ (mae-dump-file-safely (car args)))))
