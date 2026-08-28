;;; The KB cutover lifecycle is scriptable from Scheme (ADR-110, principle #3).
;;;
;;; Before these primitives existed the cutover was reachable by a human (the
;;; `:kb-*` ex-commands) and by an agent (the generated MCP mirrors) but NOT
;;; from Scheme — while `kb-register`, its opposite number, had a primitive all
;;; along. That asymmetry is what this closes.
;;;
;;; This is a WIRING test: it proves each primitive is registered, accepts its
;;; argument, and reaches the editor without erroring. The semantics — the
;;; three-state model, the retirement gate, native-KB identity — are pinned in
;;; `kb_ops_{native_kb,retire,ingest_policy}_tests`, where the fixture can
;;; observe the registry directly. Asserting them from here would be a claim
;;; this harness cannot honestly check.

(describe-group "KB cutover primitives are scriptable"
  (lambda ()
    (it-test "kb-new creates a native KB"
      (lambda () (kb-new "ScriptedKb")))
    (it-test "kb-detach is callable"
      (lambda () (kb-detach "ScriptedKb")))
    (it-test "kb-attach is callable"
      (lambda () (kb-attach "ScriptedKb")))
    (it-test "kb-retire-archive dry run is callable"
      (lambda () (kb-retire-archive "ScriptedKb")))
    ;; The store IS reachable afterwards — a KB created purely from Scheme is a
    ;; real KB, not a registry row with nothing behind it.
    (it-test "a node can be created in the scripted KB"
      (lambda () (kb-create "note:scripted" "Scripted Note" "made from Scheme" "note")))
    (it-test "and read back"
      (lambda () (should (kb-get "note:scripted"))))))
