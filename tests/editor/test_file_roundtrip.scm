;;; test_file_roundtrip.scm — Write buffer to disk and read it back
;;;
;;; Tests the file write + open roundtrip: create buffer, insert content,
;;; write to /tmp, open the file in a new buffer, verify content matches.

(define *rt-path* "/tmp/mae-test-rt.txt")
(define *rt-content* "line one\nline two\nline three\n")

(describe-group "File roundtrip"
  (lambda ()
    (it-test "setup source buffer"
      (lambda ()
        (create-buffer "*test-rt-source*")))

    (it-test "insert multi-line content"
      (lambda ()
        (buffer-insert "line one\nline two\nline three\n")))

    (it-test "verify source content"
      (lambda ()
        (should-equal (buffer-string) *rt-content*)))

    (it-test "write buffer to disk"
      (lambda ()
        (write-file *rt-path* (buffer-string))))

    (it-test "file exists on disk"
      (lambda ()
        (should (file-exists? *rt-path*))))

    (it-test "open file in editor"
      (lambda ()
        (execute-ex (string-append "e " *rt-path*))))

    (it-test "verify file content in new buffer"
      (lambda ()
        (should-equal (buffer-string) *rt-content*)))

    (it-test "content has three lines"
      (lambda ()
        (should-contain (buffer-string) "line one"))  )

    (it-test "content contains second line"
      (lambda ()
        (should-contain (buffer-string) "line two")))

    (it-test "content contains third line"
      (lambda ()
        (should-contain (buffer-string) "line three")))

    (it-test "go to beginning of buffer"
      (lambda ()
        (goto-char 0)))

    (it-test "first char is 'l'"
      (lambda ()
        (should-equal (substring (buffer-string) 0 1) "l")))

    (it-test "full content length matches"
      (lambda ()
        (should-equal (string-length (buffer-string))
                      (string-length *rt-content*))))))
