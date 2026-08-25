;; Phase 6 / Story A — the day-one experience: a fresh install with NO `.org`
;; directory anywhere.
;;
;; This is the story the cutover has to satisfy first, and it was **untestable
;; until #781**: the harness never opened a KB store, so every assertion about
;; "the node persisted" passed whether persistence worked or was entirely absent.
;;
;; The scenario is deliberately the *whole* first session a user has — capture a
;; thought, find it again, open today's daily, link them, come back tomorrow —
;; rather than one primitive at a time. A day-one experience that works only when
;; each step is tested in isolation is not a working day-one experience.
;;
;; Every assertion reads the effect BACK. A call returning without error proves
;; nothing; that is precisely what made the first dogfood of this vacuous.

(describe-group "day one: fresh install, no org directory"
  (lambda ()
    ;; --- the first thing anyone does ---
    (it-test "capture a thought"
      (lambda ()
        (kb-create "note:first-thought" "First Thought" "something worth keeping" "note")))
    (it-test "it is there afterwards"
      (lambda ()
        (should (kb-get "note:first-thought"))))
    (it-test "with the title as written"
      (lambda ()
        (should-equal (list-ref (kb-get "note:first-thought") 1) "First Thought")))

    ;; --- find it again, which is the whole point of a KB ---
    (it-test "search finds it by title"
      (lambda ()
        (kb-search "First Thought")))
    (it-test "search finds it by body text too"
      (lambda ()
        (kb-search "worth keeping")))

    ;; --- today's daily, with no dailies directory configured ---
    (it-test "open today's daily"
      (lambda ()
        (execute-ex "daily-goto-today")))

    ;; --- a second note, linked to the first ---
    (it-test "capture a second, linking the first"
      (lambda ()
        (kb-create "note:second" "Second" "follows on from [[note:first-thought|the first]]" "note")))
    (it-test "the link target resolves to a real node"
      (lambda ()
        (should (kb-get "note:first-thought"))))

    ;; --- editing, which must survive ---
    (it-test "edit the first note"
      (lambda ()
        (kb-update "note:first-thought" #f "something worth keeping, revised")))
    (it-test "the edit is visible on re-read"
      (lambda ()
        (should-contain (list-ref (kb-get "note:first-thought") 3) "revised")))

    ;; --- and a selective oracle: absence must still read as absence ---
    (it-test "a node nobody created reads back as #f"
      (lambda ()
        (should-equal (kb-get "note:never-written") #f)))

    ;; Restore the window layout this scenario changed.
    ;;
    ;; `daily-goto-today` opens the daily in a COMPANION window (store-backed, so
    ;; via `open_help_at`), and the runner's file-boundary snapshot covers mode,
    ;; keymap flavor and options -- NOT window count. Without this, the four
    ;; window-management assertions in a later file measure a baseline this file
    ;; moved, and fail in the suite while passing in isolation. Found exactly that
    ;; way.
    ;;
    ;; The framework says the snapshot is "a safety net, not a substitute for
    ;; proper cleanup". This is the cleanup.
    (it-test "restore the window layout for later files"
      (lambda ()
        (execute-ex "close-window")))))
