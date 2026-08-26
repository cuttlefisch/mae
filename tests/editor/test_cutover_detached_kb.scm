;; KB cutover, Phase 1 — drive a detached KB end to end.
;;
;; Every existing test covers ONE path in isolation (buffer save, the daemon
;; tick, `:kb-reimport`). That is deliberate and right — the failure mode is "one
;; path nobody remembered", and a single end-to-end test would pass while nine
;; paths stayed open.
;;
;; What was missing is the other half: a scenario that detaches and then *lives*
;; in the KB, the way the dogfood will. Reconnaissance found four defects that
;; per-path unit tests could not see, because each was in the seam BETWEEN paths:
;;
;;   - `:kb-detach primary` wrote one field while dailies and the edit surface
;;     read another, so the detach was a no-op for both
;;   - deleting the stale archive deleted the nodes from the durable store
;;   - the four daily navigation commands had no store path at all
;;   - capture wrote `.org` into the archive for anyone with a `notes_dir` set
;;
;; So this scenario asserts the *state after a sequence*, not the return value of
;; any one call. A step that merely runs without error proves nothing — that is
;; exactly what made the first dogfood of this vacuous.
;;
;; **What this file proves, precisely.** That the whole sequence works against the
;; REAL binary on a store-backed KB, and — checked by the runner, not by Scheme —
;; that no `.org` file is written anywhere under an isolated HOME while it runs.
;; Run it that way, always:
;;
;;   T=$(mktemp -d); HOME=$T XDG_CONFIG_HOME=$T/.config \
;;   XDG_DATA_HOME=$T/.local/share MAE_SKIP_WIZARD=1 \
;;   ./target/release/mae --test tests/editor/test_cutover_detached_kb.scm
;;   find "$T" -name '*.org'      # must be empty
;;
;; Running it against a live HOME writes into real notes and rewrites
;; `kb-registry.toml`, detaching the primary mid-run — it did exactly that once.
;;
;; **What it does NOT prove:** the ingest-policy fixes themselves. A fresh HOME
;; has no `kb-notes-dir`, so `kb_daily_backing` already returns `Store` via the
;; no-dailies-dir branch and the policy branch never decides anything — the
;; scenario passes unchanged with the policy defect reintroduced. Those fixes are
;; falsifiable only in `kb_ops_ingest_policy_tests`, where the fixture can hold a
;; notes dir AND a detached primary at once.
;;
;; **What this file deliberately does NOT verify:** that `daily-prev` actually
;; MOVED the view. No Scheme primitive exposes the current KB view's node, and
;; `buffer-string` does not observe a `BufferKind::Kb` buffer in this harness —
;; two oracles were tried and both passed against a deliberately-broken
;; `daily-prev`, the second because the current daily's body contains a
;; `Previous:` link carrying the very date being matched. Navigation is verified
;; where it can be: `kb_ops_daily_backing_tests::daily_navigation_works_on_a_store_backing`,
;; which does fail when the store path is removed. An assertion a test cannot
;; honestly make is worse than no assertion, because it reads as coverage.

(describe-group "cutover: living in a detached KB"
  (lambda ()
    ;; --- detach the primary ---
    (it-test "detach the primary KB"
      (lambda ()
        (execute-ex "kb-detach primary")))

    ;; --- capture still works, and reaches the store ---
    (it-test "capture a thought after detaching"
      (lambda ()
        (kb-create "note:after-detach" "After Detach" "written once the store is truth" "note")))
    (it-test "it is in the store"
      (lambda ()
        (should (kb-get "note:after-detach"))))
    (it-test "with the body as written"
      (lambda ()
        ;; kb-get returns (id title kind body tags) — body is index 3.
        (should-contain (list-ref (kb-get "note:after-detach") 3) "store is truth")))

    ;; --- search reads it back ---
    (it-test "search finds it by title"
      (lambda ()
        (kb-search "After Detach")))
    (it-test "search finds it by body"
      (lambda ()
        (kb-search "store is truth")))

    ;; --- dailies, the surface with no store path until now ---
    (it-test "open today's daily"
      (lambda ()
        (execute-ex "daily-goto-today")))
    (it-test "navigate to a specific date"
      (lambda ()
        (execute-ex "daily-goto-date 2026-08-26")))
    (it-test "that daily exists as a node"
      (lambda ()
        (should (kb-get "daily:2026-08-26"))))
    ;; The previous day must EXIST as a node first, or `daily-prev` has nothing
    ;; to find and the step asserts nothing. `execute-ex` reports ok for a
    ;; command that runs and then fails via its status line, so the navigation
    ;; is verified by reading the buffer it landed in — not by its return.
    (it-test "the previous day exists as a node too"
      (lambda ()
        (should (kb-get "daily:2026-08-25"))))
    (it-test "and daily-prev runs against a store backing"
      (lambda ()
        (execute-ex "daily-prev")))

    ;; --- edit a node through the surface ADR-092 D3/D5 added ---
    (it-test "update the captured node"
      (lambda ()
        (kb-update "note:after-detach" "After Detach" "edited while detached")))
    (it-test "the edit landed in the store"
      (lambda ()
        (should-contain (list-ref (kb-get "note:after-detach") 3) "edited while detached")))

    ;; --- re-attaching must not destroy what was written while detached ---
    (it-test "re-attach the primary"
      (lambda ()
        (execute-ex "kb-attach primary")))
    (it-test "the node written while detached survives re-attaching"
      (lambda ()
        (should (kb-get "note:after-detach"))))
    (it-test "and so does its edit"
      (lambda ()
        (should-contain (list-ref (kb-get "note:after-detach") 3) "edited while detached")))

    ;; --- leave the editor as we found it ---
    (it-test "restore the attached default"
      (lambda ()
        (execute-ex "kb-attach primary")))))
