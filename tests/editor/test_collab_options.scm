;; Test: Collaboration options — Scheme-accessible round-trip
;; Verifies new collab options can be read/written via get-option / set-option!

(describe-group "Collab options"
  (lambda ()
    (it-test "collab option defaults"
      (lambda ()
        (should-equal (get-option "collab_server_address") "127.0.0.1:9473")
        (should-equal (get-option "collab_auto_connect") "false")
        (should-equal (get-option "collab_max_pending_updates") "1000")
        (should-equal (get-option "collab_reconnect_backoff_factor") "2")
        (should-equal (get-option "collab_max_reconnect_attempts") "0")
        (should-equal (get-option "collab_batch_update_ms") "0")))

    (it-test "collab option round-trips"
      (lambda ()
        (set-option! "collab_max_pending_updates" "500")
        (should-equal (get-option "collab_max_pending_updates") "500")
        (set-option! "collab_batch_update_ms" "100")
        (should-equal (get-option "collab_batch_update_ms") "100")
        (set-option! "collab_reconnect_backoff_factor" "3")
        (should-equal (get-option "collab_reconnect_backoff_factor") "3")))))
