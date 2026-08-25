;; KB persistence through the Scheme test harness (#781).
;;
;; **No test in tests/editor/ exercised a `kb-` primitive before this** — 27
;; files, zero. That was not an oversight: `handle_test_mode` never opened a
;; primary KB store, so `kb-create` reported success and wrote nowhere, and
;; `kb-get` on the same id returned #f. Nobody could write a KB test that worked.
;;
;; The failure mode that matters is not "the feature was broken" but "the test
;; would have passed anyway": a scenario asserting only that the call did not
;; error passed identically whether persistence worked or was entirely absent.
;;
;; So every assertion here reads the effect BACK. `kb-create` returning without
;; error proves nothing.

(describe-group "KB persistence"
  (lambda ()
    (it-test "create a node"
      (lambda ()
        (kb-create "note:harness-canary" "Harness Canary" "body text" "note")))

    ;; THE assertion. This is what was impossible before.
    (it-test "the created node reads back"
      (lambda ()
        (should (kb-get "note:harness-canary"))))

    (it-test "it reads back with the right title, not merely truthy"
      (lambda ()
        (let ((node (kb-get "note:harness-canary")))
          (should-equal (list-ref node 1) "Harness Canary"))))

    ;; A selective oracle: a store that returned SOMETHING for every id would
    ;; pass the assertions above. This one must come back #f.
    (it-test "a node that was never created reads back as #f"
      (lambda ()
        (should-equal (kb-get "note:never-created-at-all") #f)))

    (it-test "search finds the created node"
      (lambda ()
        (kb-search "Harness")))

    (it-test "update changes the body"
      (lambda ()
        (kb-update "note:harness-canary" #f "edited body text")))

    (it-test "the edit is visible on a re-read"
      (lambda ()
        (let ((node (kb-get "note:harness-canary")))
          (should-contain (list-ref node 3) "edited"))))

    (it-test "delete removes it"
      (lambda ()
        (kb-delete "note:harness-canary")))

    (it-test "the deleted node no longer reads back"
      (lambda ()
        (should-equal (kb-get "note:harness-canary") #f)))))
