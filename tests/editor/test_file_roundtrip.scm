;;; test_file_roundtrip.scm — Write buffer to disk and read it back
;;;
;;; Tests the file write + open roundtrip: create buffer, insert content,
;;; write to /tmp, open the file in a new buffer, verify content matches.

(define *rt-path* "/tmp/mae-test-rt.txt")
(define *rt-content* "line one\nline two\nline three\n")

(describe-group "File roundtrip"
  (lambda ()
    (it-test "write buffer to disk"
      (lambda ()
        (create-buffer "*test-rt-source*")
        (buffer-insert "line one\nline two\nline three\n")
        (should-equal (buffer-string) *rt-content*)
        (write-file *rt-path* (buffer-string))))

    (it-test "verify file was written"
      (lambda ()
        (should (file-exists? *rt-path*))))

    (it-test "open file and verify content"
      (lambda ()
        (execute-ex (string-append "e " *rt-path*))
        (should-equal (buffer-string) *rt-content*)
        (should-contain (buffer-string) "line one")
        (should-contain (buffer-string) "line two")
        (should-contain (buffer-string) "line three")
        (goto-char 0)
        (should-equal (substring (buffer-string) 0 1) "l")
        (should-equal (string-length (buffer-string))
                      (string-length *rt-content*))))))
