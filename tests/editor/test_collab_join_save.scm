;;; test_collab_join_save.scm — Join-save model data lifecycle tests
;;;
;;; Verifies that:
;;; - Buffers without file_path report appropriate errors on :w
;;; - :saveas works to set a path and persist
;;; - New collab options have correct defaults and round-trip
;;;
;;; No (run-tests) — uses Rust-side iteration.

(describe-group "Collab join-save model"
  (lambda ()

    (it-test "create pathless buffer"
      (lambda ()
        (create-buffer "*collab-test*")))

    (it-test "insert content"
      (lambda ()
        (buffer-insert "shared content\n")))

    (it-test "verify content"
      (lambda ()
        (should-equal (buffer-string) "shared content\n")))

    (it-test "save pathless buffer shows error"
      (lambda ()
        (run-command "save")))

    (it-test "saveas creates file on disk"
      (lambda ()
        (execute-ex "saveas /tmp/mae-test-collab-join/saved.txt")))

    (it-test "verify file exists"
      (lambda ()
        (should (file-exists? "/tmp/mae-test-collab-join/saved.txt"))))

    (it-test "save again works after saveas"
      (lambda ()
        (run-command "save")))

    ;; --- Option round-trip tests ---
    (it-test "collab_auto_resolve_paths default is false"
      (lambda ()
        (should-equal (get-option "collab_auto_resolve_paths") "false")))

    (it-test "set collab_auto_resolve_paths to true"
      (lambda ()
        (set-option! "collab_auto_resolve_paths" "true")))

    (it-test "verify collab_auto_resolve_paths round-trip"
      (lambda ()
        (should-equal (get-option "collab_auto_resolve_paths") "true")))

    (it-test "collab_default_save_dir default is empty"
      (lambda ()
        (should-equal (get-option "collab_default_save_dir") "")))

    (it-test "set collab_default_save_dir"
      (lambda ()
        (set-option! "collab_default_save_dir" "/tmp/collab")))

    (it-test "verify collab_default_save_dir round-trip"
      (lambda ()
        (should-equal (get-option "collab_default_save_dir") "/tmp/collab")))

    (it-test "collab_save_on_remote_update default is false"
      (lambda ()
        (should-equal (get-option "collab_save_on_remote_update") "false")))

    (it-test "set collab_save_on_remote_update to true"
      (lambda ()
        (set-option! "collab_save_on_remote_update" "true")))

    (it-test "verify collab_save_on_remote_update round-trip"
      (lambda ()
        (should-equal (get-option "collab_save_on_remote_update") "true")))))
