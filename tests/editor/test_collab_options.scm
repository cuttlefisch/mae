;; Test: Collaboration options — Scheme-accessible round-trip
;; Verifies new collab options can be read/written via get-option / set-option!

(describe-group "Collab options"
  (lambda ()
    ;; Read defaults
    (it-test "collab_server_address default"
      (lambda ()
        (should-equal (get-option "collab_server_address") "127.0.0.1:9473")))

    (it-test "collab_auto_connect default"
      (lambda ()
        (should-equal (get-option "collab_auto_connect") "false")))

    (it-test "collab_max_pending_updates default"
      (lambda ()
        (should-equal (get-option "collab_max_pending_updates") "1000")))

    (it-test "collab_reconnect_backoff_factor default"
      (lambda ()
        (should-equal (get-option "collab_reconnect_backoff_factor") "2")))

    (it-test "collab_max_reconnect_attempts default"
      (lambda ()
        (should-equal (get-option "collab_max_reconnect_attempts") "0")))

    (it-test "collab_batch_update_ms default"
      (lambda ()
        (should-equal (get-option "collab_batch_update_ms") "0")))

    ;; Set and read back
    (it-test "set collab_max_pending_updates"
      (lambda ()
        (set-option! "collab_max_pending_updates" "500")))

    (it-test "verify collab_max_pending_updates changed"
      (lambda ()
        (should-equal (get-option "collab_max_pending_updates") "500")))

    (it-test "set collab_batch_update_ms"
      (lambda ()
        (set-option! "collab_batch_update_ms" "100")))

    (it-test "verify collab_batch_update_ms changed"
      (lambda ()
        (should-equal (get-option "collab_batch_update_ms") "100")))

    (it-test "set collab_reconnect_backoff_factor"
      (lambda ()
        (set-option! "collab_reconnect_backoff_factor" "3")))

    (it-test "verify collab_reconnect_backoff_factor changed"
      (lambda ()
        (should-equal (get-option "collab_reconnect_backoff_factor") "3")))))
