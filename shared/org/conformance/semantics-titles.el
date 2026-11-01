;;; semantics-titles.el --- an Emacs oracle for semantics() titles and levels  -*- lexical-binding: t -*-
;;
;; The independent oracle of tests/semantics_differential.rs (ADR-112 D5):
;; for every KB node of every listed file, its id, level and plain-text
;; title, computed by Org's own parser (org-element, Org 9.7.11) with no
;; mae-org code involved, so a bug in mae-org's plain-text reading cannot
;; agree with itself.
;;
;; Usage (what gen-semantics-titles.sh runs):
;;   emacs -Q --batch -l semantics-titles.el --manifest MANIFEST > OUT.tsv
;; where each MANIFEST line is "ABSOLUTE-PATH<TAB>REPO-RELATIVE-PATH".
;; Output: one line per node, "FILE<TAB>ID<TAB>LINE<TAB>LEVEL<TAB>TITLE",
;; LINE being the 1-based line the element starts on (the file node's is 1).
;;
;; Where Org has a reading, the oracle calls Org's own function: a file's
;; title is `org-get-title', and a headline's or the file's id is
;; `org-entry-get' (the last :ID: line, then every :ID+: value). The rest
;; restates mae-org's documented rules (src/semantics/plain.rs and items.rs)
;; on Org's tree, for what Org does not define:
;; - nodes: the file (an ID in the zeroth section's property drawer), every
;;   headline with an ID property, and every list item whose first element,
;;   or second after a paragraph, is a drawer named PROPERTIES holding only
;;   `:KEY: value' lines, one of them ID or ID+. Every such element is
;;   listed, rendered or not, duplicates included, in document order. An
;;   item's id reads its drawer as `org-entry-get' reads a headline's.
;; - level: 0 for the file, the headline's stars, and for an item its
;;   headline's level plus the items it is nested in, itself included.
;; - title: the file's `org-get-title' (every #+title line, joined); a
;;   headline's title; an item's "TAG :: FIRST-PARAGRAPH" (either alone if
;;   the other is empty), all read as plain text. Plain text: bold, italic,
;;   underline and strike-through give
;;   their contents, verbatim and code their value; a link its description,
;;   else (an id or fuzzy link that is not a heading search) the text after
;;   the first `|' of its path (MAE's store form), else its path as Org
;;   normalises it (`:raw-link'); every other object exactly as written.
;;   Then runs of space, tab, LF, CR and form feed become one space, trimmed.
;;
;; mae-org does no I/O; neither does this oracle (setup files are not read).
(require 'org)
(require 'org-element)
(require 'cl-lib)

(advice-add 'org-file-contents :override (lambda (&rest _) ""))

(defun mae-titles--collapse (s)
  "Runs of blanks and line endings in S as one space, trimmed."
  (let ((s (replace-regexp-in-string "[ \t\n\r\f]+" " " s)))
    (string-trim s "[ ]+" "[ ]+")))

(defun mae-titles--org-trim (s)
  "Org's trim: space, tab, LF and CR."
  (string-trim s "[ \t\n\r]+" "[ \t\n\r]+"))

(defun mae-titles--blanks (obj)
  "The blanks after OBJ."
  (make-string (or (org-element-property :post-blank obj) 0) ?\s))

(defun mae-titles--kb-p (link)
  "Whether LINK is a KB link to MAE (an id or fuzzy link, not a heading
search and not one of MAE's non-KB schemes Org does not know)."
  (let ((type (org-element-property :type link))
        (raw (org-element-property :raw-link link)))
    (or (string= type "id")
        (and (string= type "fuzzy")
             (not (string-prefix-p "*" raw))
             (not (string-match-p
                   "\\`\\(roam\\|cite\\|zotero\\|attachment\\|man\\|ftps\\):" raw))))))

(defun mae-titles--link (link)
  "How LINK reads as plain text."
  (let ((desc (and (org-element-contents link)
                   (mae-titles--collapse (mae-titles--plain (org-element-contents link)))))
        (raw (org-element-property :raw-link link)))
    (cond
     ((and desc (not (string-empty-p desc))) desc)
     ((and (mae-titles--kb-p link)
           (string-match "|" raw)
           (not (string-empty-p (mae-titles--org-trim (substring raw (match-end 0))))))
      (mae-titles--org-trim (substring raw (match-end 0))))
     (t (mae-titles--collapse (mae-titles--org-trim raw))))))

(defun mae-titles--plain (objects)
  "The plain text of OBJECTS (a secondary string or contents)."
  (mapconcat
   (lambda (o)
     (pcase (org-element-type o)
       ('plain-text (substring-no-properties o))
       ((or 'bold 'italic 'underline 'strike-through)
        (concat (mae-titles--plain (org-element-contents o)) (mae-titles--blanks o)))
       ((or 'verbatim 'code)
        (concat (org-element-property :value o) (mae-titles--blanks o)))
       ('link (concat (mae-titles--link o) (mae-titles--blanks o)))
       (_ (let ((post (or (org-element-property :post-blank o) 0)))
            (concat (buffer-substring-no-properties
                     (org-element-property :begin o)
                     (- (org-element-property :end o) post))
                    (mae-titles--blanks o))))))
   objects ""))

(defun mae-titles--string (s)
  "The plain text of S parsed as a paragraph's objects, in a buffer of its own."
  (with-temp-buffer
    (insert s)
    (let ((org-inhibit-startup t)) (org-mode))
    (mae-titles--collapse
     (mae-titles--plain
      (org-element--parse-objects (point-min) (point-max) nil
                                  (org-element-restriction 'paragraph))))))

(defun mae-titles--headline-level (el)
  "The level of the nearest headline around EL, 0 if none."
  (let ((h (org-element-lineage el '(headline))))
    (if h (org-element-property :level h) 0)))

(defun mae-titles--item-depth (item)
  "How many items ITEM is nested in, itself included."
  (let ((n 0) (el item))
    (while el
      (when (org-element-type-p el 'item) (setq n (1+ n)))
      (setq el (org-element-parent el)))
    n))

(defun mae-titles--item-props (drawer)
  "The ID of DRAWER's lines if every non-blank one is `:KEY: value'.
Read as `org-entry-get' reads a headline's: the last :ID: value, then
every :ID+: value in order, joined with a space; \"nil\" is no value."
  (let* ((beg (org-element-property :contents-begin drawer))
         (end (org-element-property :contents-end drawer))
         (lines (if beg (split-string (buffer-substring-no-properties beg end) "[\n\r]") nil))
         (base nil) (extra nil) (ok t))
    (dolist (l lines)
      (unless (string-match-p "\\`[ \t]*\\'" l)
        (if (string-match "\\`[ \t]*:\\([^ \t]+\\):\\(?:[ \t]\\(.*\\)\\)?\\'" l)
            (let ((key (upcase (match-string 1 l)))
                  (value (string-trim (or (match-string 2 l) "") "[ \t]+" "[ \t]+")))
              (cond ((string= key "ID") (setq base value))
                    ((string= key "ID+") (push value extra))))
          (setq ok nil))))
    (let ((id (mapconcat #'identity
                         (delq nil (mapcar #'org-not-nil (cons base (nreverse extra))))
                         " ")))
      (and ok (not (string-empty-p id)) id))))

(defun mae-titles--item (item)
  "(ID LEVEL TITLE) if ITEM has its own property drawer, else nil."
  (let* ((kids (org-element-contents item))
         (first (nth 0 kids))
         (second (nth 1 kids))
         (drawer (cond ((org-element-type-p first 'drawer) first)
                       ((and (org-element-type-p first 'paragraph)
                             (org-element-type-p second 'drawer))
                        second)))
         (id (and drawer
                  (string= (upcase (org-element-property :drawer-name drawer)) "PROPERTIES")
                  (mae-titles--item-props drawer))))
    (when id
      (let* ((tag (org-element-property :tag item))
             (tag (and tag (mae-titles--collapse (mae-titles--plain tag))))
             (para (and (not (eq drawer first)) first))
             (text (and para (mae-titles--collapse
                              (mae-titles--plain (org-element-contents para)))))
             (tag (and tag (not (string-empty-p tag)) tag))
             (text (and text (not (string-empty-p text)) text))
             (depth (mae-titles--item-depth item)))
        (list id
              (+ depth (mae-titles--headline-level item))
              (cond ((and tag text) (concat tag " :: " text))
                    (t (or tag text ""))))))))

(defun mae-titles--file (data)
  "(ID 0 TITLE) of the file node of DATA, or nil.
The id is `org-entry-get' before the first headline (the zeroth section's
property drawer); the title is `org-get-title', read as plain text."
  (let ((section (car (org-element-contents data))))
    (when (org-element-type-p section 'section)
      (let* ((kids (org-element-contents section))
             (drawer (cl-find-if (lambda (e) (org-element-type-p e 'property-drawer)) kids))
             (id (and drawer (org-entry-get (org-element-begin drawer) "ID")))
             (title (org-get-title)))
        (when (and id (not (string-empty-p (mae-titles--org-trim id))))
          (list (mae-titles--org-trim id) 0
                (if title (mae-titles--string title) "")))))))

(defun mae-titles--emit (rel line row)
  (princ (format "%s\t%s\t%d\t%d\t%s\n" rel (nth 0 row) line (nth 1 row) (nth 2 row))))

(defun mae-titles--line (el)
  (line-number-at-pos (org-element-property :begin el)))

(defun mae-titles-file (path rel)
  "Print the node rows of the file at PATH, named REL."
  (with-temp-buffer
    (let ((coding-system-for-read 'utf-8))
      (insert-file-contents path))
    (let ((org-inhibit-startup t)) (org-mode))
    (let ((data (org-element-parse-buffer)))
      (let ((f (mae-titles--file data)))
        (when f (mae-titles--emit rel 1 f)))
      (org-element-map data '(headline item)
        (lambda (el)
          (pcase (org-element-type el)
            ('headline
             (let ((id (org-entry-get (org-element-begin el) "ID")))
               (when (and id (not (string-empty-p (mae-titles--org-trim id))))
                 (mae-titles--emit
                  rel (mae-titles--line el)
                  (list (mae-titles--org-trim id)
                            (org-element-property :level el)
                            (mae-titles--collapse
                             (mae-titles--plain (org-element-property :title el))))))))
            ('item
             (let ((row (mae-titles--item el)))
               (when row (mae-titles--emit rel (mae-titles--line el) row))))))))))

(let ((args command-line-args-left))
  (when (equal (car args) "--manifest")
    (with-temp-buffer
      (insert-file-contents (cadr args))
      (dolist (line (split-string (buffer-string) "\n" t))
        (let ((parts (split-string line "\t")))
          (mae-titles-file (nth 0 parts) (nth 1 parts)))))
    (setq command-line-args-left nil)))
