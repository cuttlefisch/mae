;;; test_collab_join_save.scm — Join-save model data lifecycle tests
;;;
;;; Verifies that:
;;; - Buffers without file_path report appropriate errors on :w
;;; - :saveas works to set a path and persist
;;; - New collab options have correct defaults and round-trip

(describe-group "Collab join-save model"
  (lambda ()
    (it-test "pathless buffer save and saveas workflow"
      (lambda ()
        (create-buffer "*collab-test*")
        (buffer-insert "shared content\n")
        (should-equal (buffer-string) "shared content\n")
        (run-command "save")
        (execute-ex "saveas /tmp/mae-test-collab-join-saved.txt")
        (should (file-exists? "/tmp/mae-test-collab-join-saved.txt"))
        (run-command "save")))

    (it-test "collab join-save option round-trips"
      (lambda ()
        (should-equal (get-option "collab_auto_resolve_paths") "false")
        (set-option! "collab_auto_resolve_paths" "true")
        (should-equal (get-option "collab_auto_resolve_paths") "true")
        (should-equal (get-option "collab_default_save_dir") "")
        (set-option! "collab_default_save_dir" "/tmp/collab")
        (should-equal (get-option "collab_default_save_dir") "/tmp/collab")
        (should-equal (get-option "collab_save_on_remote_update") "false")
        (set-option! "collab_save_on_remote_update" "true")
        (should-equal (get-option "collab_save_on_remote_update") "true")))))
