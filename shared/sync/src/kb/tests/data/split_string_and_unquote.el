;;; split_string_and_unquote.el --- regenerate the ROAM_ALIASES oracle  -*- lexical-binding: t -*-
;;
;; Writes split_string_and_unquote.jsonl next to this file: one line per input,
;; with what Emacs' `split-string-and-unquote' returns for it in an `org-mode'
;; buffer (where org-roam calls it on a node's ROAM_ALIASES value).
;;
;;   emacs --batch -Q -l shared/sync/src/kb/tests/data/split_string_and_unquote.el
;;
;; Each line is {"in": INPUT, "ok": [[CODE...]...]} or {"in": INPUT, "err": MSG}.
;; Results are code-point lists so nothing is lost in JSON; a raw byte (what a
;; unibyte string holds above 127) is written as its Emacs character code,
;; #x3FFF80-#x3FFFFF, which no Unicode scalar can collide with.
;; Recorded with Emacs 30.2 / Org 9.7.11 (the "emacs" line at the end).

(require 'org)
(require 'json)

(defconst sstu-inputs
  (list
   ;; splitting
   "foo \"bar baz\"" "" "   " "a  b\tc" "a\nb" "a\fb\rc\vd" "\u0085x y"
   "a\u00a0b c" "a\u3000b" "a\u2003b" "a\u200bb" "a\u202fb\u205fc" "a\u1680b"
   "'a' b" "x,y z" "é ü"
   ;; quoting
   "\"a b\"\"c d\"" "foo\"bar\"baz" "\"\" x" "x\"\"" "\"a\"b\"c\"" "\"x\" \"y\""
   "\"é ü\" ñ" "  \"lead\"  trail  "
   ;; escapes Emacs and MAE both read
   "\"esc \\\" q\"" "\"back\\\\slash\"" "\"nl\\nx\"" "\"tab\\tx\"" "\"q\\qx\""
   "\"oct\\101x\"" "\"\\1234\"" "\"\\0\"" "\"\\777\"" "\"x\\8y\""
   "\"hex\\x41;x\"" "\"hex\\x41x\"" "\"\\x00000041\"" "\"\\x0\"" "\"\\X41\""
   "\"u\\u00e9x\"" "\"U\\U0001F600x\"" "\"\\U00000041\""
   "\"named\\N{U+41}x\"" "\"\\N{U+0041}\""
   "\"sp\\ x\"" "\"cont\\\nx\"" "\"s\\sx\"" "\"\\s-a\""
   "\"\\e\\a\\d\\f\\r\\v\\b\"" "\"\\é\"" "\"\\\t\"" "\"a\\\"\"b"
   "\"\\Sx\"" "\"\\Cx\"" "\"\\Mx\"" "\"\\Hx\"" "\"\\Ax\""
   ;; errors in Emacs
   "\"unterminated" "a \"unterminated rest" "a\"" "a \"b\\\"" "\"trailing\\"
   "\"bad\\x\"" "\"\\xg\"" "\"\\u12\"" "\"\\U0041\""
   "\"\\N{U+D800}\"" "\"\\N{U+110000}\"" "\"\\N{U+}\"" "\"\\N{u+41}\""
   "\"\\N{ U+41}\"" "\"\\N{U+41\"" "\"\\N41\"" "\"\\H-a\"" "\"\\A-a\""
   ;; accepted by Emacs, refused by MAE (the divergence register in the test)
   "\"\\C-a\"" "\"\\^I\"" "\"\\M-a\"" "\"\\S-a\"" "\"\\200\"" "\"\\xe9\""
   "\"é\\xe9\"" "\"\\x80\"" "\"\\ud800\"" "\"\\x110000\""
   "\"\\N{LATIN SMALL LETTER E WITH ACUTE}\""))

(defun sstu-codes (s)
  (append (string-to-multibyte s) nil))

(let ((out (expand-file-name "split_string_and_unquote.jsonl"
                             (file-name-directory load-file-name)))
      (json-encoding-pretty-print nil))
  (with-temp-file out
    (dolist (s sstu-inputs)
      (let ((r (condition-case err
                   (with-temp-buffer
                     (org-mode)
                     (list (cons "in" s)
                           (cons "ok" (vconcat (mapcar (lambda (x) (vconcat (sstu-codes x)))
                                                       (split-string-and-unquote s))))))
                 (error (list (cons "in" s) (cons "err" (format "%S" (car err))))))))
        (insert (json-encode r) "\n")))
    (insert (json-encode (list (cons "emacs" emacs-version) (cons "org" (org-version)))) "\n")))
