//! Behavioural tests for the CLAUDE.md principle-#3 parity primitives:
//! KB CRUD, `set-option-save!`, and the LSP/DAP surface.
//!
//! Written attacker-first (principle #14). For each primitive the *primary*
//! assertion is the failure path — the missing node, the nonexistent option,
//! the absent language server, the absent debug session, the unknown request
//! id — because a primitive that silently succeeds on those is worse than one
//! that does not exist: a Scheme program would branch on a lie. The
//! happy-path cases exist only to prove the failures are not vacuous.

use super::*;
use mae_core::Editor;

fn new_runtime() -> SchemeRuntime {
    SchemeRuntime::new().unwrap()
}

/// A per-test in-memory CozoDB store wired as the primary KB, mirroring
/// `runtime_tests.rs::editor_with_cozo_store`. In-memory, so tests never share
/// a path and can run in parallel.
fn editor_with_cozo_store() -> Editor {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor
}

// ---------------------------------------------------------------------------
// KB CRUD — the failure paths first
// ---------------------------------------------------------------------------

/// `kb-get` on an id that does not exist answers `#f`. That is deliberately
/// NOT an error: "absent" is a normal branch for a lookup. What must never
/// happen is a *store failure* also reading as `#f` — covered separately by
/// the no-store case below, which likewise must not fabricate a node.
#[test]
fn kb_get_distinguishes_absent_from_present() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("note:present", "Present", "body", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);

    assert_eq!(
        rt.eval(r#"(kb-get "note:definitely-not-here")"#).unwrap(),
        "#f",
        "a missing node must read as #f, not as an empty node"
    );
    let found = rt.eval(r#"(kb-get "note:present")"#).unwrap();
    assert!(found.contains("note:present"), "{found}");
    assert!(found.contains("Present"), "{found}");
    assert!(found.contains("body"), "{found}");
}

/// Several distinct ids, not one hand-picked one: an id with a namespace, one
/// with punctuation, one with a unicode title, and one whose body is empty.
/// Each must round-trip through `kb-create` → `kb-get` unchanged.
#[test]
fn kb_create_round_trips_varied_ids_and_bodies() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);
    let mut editor = editor;

    let cases = [
        ("note:plain", "Plain", "a body"),
        ("project:with-dashes-2", "Dashed", ""),
        (
            "concept:unicode",
            "Ünïcødé — ✓",
            "naïve façade\nsecond line",
        ),
        (
            "task:punct",
            "Quotes \"and\" 'apostrophes'",
            "body: with; punct.",
        ),
    ];

    for (id, title, body) in cases {
        let code = format!(
            r#"(kb-create "{id}" "{}" "{}")"#,
            title.replace('"', "\\\""),
            body.replace('"', "\\\"").replace('\n', "\\n")
        );
        assert_eq!(rt.eval(&code).unwrap(), "#t", "kb-create {id}");
    }
    rt.apply_to_editor(&mut editor);
    rt.inject_editor_state(&editor);

    for (id, title, body) in cases {
        let got = rt.eval(&format!(r#"(kb-get "{id}")"#)).unwrap();
        assert!(got.contains(id), "{id} missing from {got}");
        // The selective oracle: the *title we wrote* comes back, not merely
        // "some node exists".
        assert!(
            got.contains(title.split(' ').next().unwrap()),
            "title for {id} not round-tripped: {got}"
        );
        if !body.is_empty() {
            assert!(
                got.contains(body.lines().next().unwrap()),
                "body for {id} not round-tripped: {got}"
            );
        }
    }
}

/// The attacker's case for create: an id that is already taken must be
/// refused *before* anything is queued, or a Scheme program would believe it
/// created a node while silently overwriting or colliding with another.
#[test]
fn kb_create_refuses_a_duplicate_id_and_queues_nothing() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("note:taken", "Original", "keep me", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);

    let err = rt
        .eval(r#"(kb-create "note:taken" "Impostor" "clobber")"#)
        .unwrap_err();
    assert!(
        err.message.contains("already exists") && err.message.contains("note:taken"),
        "the error must name the collision: {}",
        err.message
    );

    // Effect-level oracle: applying whatever WAS queued must not have touched
    // the original node.
    rt.apply_to_editor(&mut editor);
    let node = editor.kb.primary.get("note:taken").expect("node survives");
    assert_eq!(node.title, "Original", "the existing node was clobbered");
    assert_eq!(node.body, "keep me");
}

/// An empty id is refused. Without this a `(kb-create "" ...)` would queue a
/// node with no addressable id — creatable but never gettable.
#[test]
fn kb_create_refuses_an_empty_id() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);
    for blank in [r#""""#, r#""   ""#, "\"\\t\""] {
        let err = rt
            .eval(&format!(r#"(kb-create {blank} "T" "B")"#))
            .unwrap_err();
        assert!(
            err.message.contains("must not be empty"),
            "{blank} should be refused: {}",
            err.message
        );
    }
}

/// `kb-update` and `kb-delete` on a nonexistent node are errors, not silent
/// no-ops — the case the parity task names explicitly. A no-op here would let
/// a script "successfully" update a node it misspelled.
#[test]
fn kb_update_and_delete_error_on_a_missing_node() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);

    for call in [
        r#"(kb-update "note:ghost" "New title")"#,
        r#"(kb-delete "note:ghost")"#,
    ] {
        let err = rt.eval(call).unwrap_err();
        assert!(
            err.message.contains("No KB node") && err.message.contains("note:ghost"),
            "{call} must name the missing node, got: {}",
            err.message
        );
    }
}

/// An update that changes nothing is refused rather than queued: a
/// `(kb-update "id")` with no fields is almost certainly a caller mistake, and
/// queueing it would make the primitive report success for a no-op.
#[test]
fn kb_update_refuses_an_empty_update() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("note:u", "T", "B", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);

    let err = rt.eval(r#"(kb-update "note:u")"#).unwrap_err();
    assert!(
        err.message.contains("nothing to update"),
        "got: {}",
        err.message
    );
    // Passing all three as #f is the same thing said the long way.
    let err = rt.eval(r#"(kb-update "note:u" #f #f #f)"#).unwrap_err();
    assert!(err.message.contains("nothing to update"), "{}", err.message);
}

/// Each field updates independently, and an omitted/`#f` field is genuinely
/// left alone — the property that makes the optional arguments safe to use.
/// Exercised over all three fields rather than one, and asserted on the
/// *other* fields' survival, not only on the changed one.
#[test]
fn kb_update_changes_only_the_fields_given() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node(
            "note:sel",
            "Original Title",
            "original body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    rt.inject_editor_state(&editor);

    // Title only.
    rt.eval(r#"(kb-update "note:sel" "New Title")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let n = editor.kb.primary.get("note:sel").unwrap();
    assert_eq!(n.title, "New Title");
    assert_eq!(n.body, "original body", "body must be untouched");

    // Body only (title passed as #f).
    rt.inject_editor_state(&editor);
    rt.eval(r#"(kb-update "note:sel" #f "new body")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let n = editor.kb.primary.get("note:sel").unwrap();
    assert_eq!(n.title, "New Title", "title must be untouched");
    assert_eq!(n.body, "new body");

    // Tags only — and tags REPLACE rather than merge.
    rt.inject_editor_state(&editor);
    rt.eval(r#"(kb-update "note:sel" #f #f '("alpha" "beta"))"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let n = editor.kb.primary.get("note:sel").unwrap();
    assert_eq!(n.tags, vec!["alpha".to_string(), "beta".to_string()]);
    assert_eq!(n.title, "New Title");
    assert_eq!(n.body, "new body");
}

/// A non-string in the tag list is a type error, not a silently-stringified
/// tag. Chosen because `'("a" #f)` is exactly what a caller writing
/// `(kb-update id #f #f (list a b))` produces when one variable is unset.
#[test]
fn kb_update_rejects_a_malformed_tag_list() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("note:tags", "T", "B", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);

    assert!(rt
        .eval(r#"(kb-update "note:tags" #f #f '("ok" #f))"#)
        .is_err());
    assert!(rt.eval(r#"(kb-update "note:tags" #f #f 42)"#).is_err());
}

/// Delete actually removes the node — the effect-level oracle, not the return
/// value. Then a second delete of the same id must fail, proving the first one
/// really took effect rather than the error simply being absent.
#[test]
fn kb_delete_removes_the_node_and_a_second_delete_fails() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("note:doomed", "T", "B", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);

    assert_eq!(rt.eval(r#"(kb-delete "note:doomed")"#).unwrap(), "#t");
    rt.apply_to_editor(&mut editor);
    assert!(
        editor.kb.primary.get("note:doomed").is_none(),
        "the node must actually be gone"
    );

    rt.inject_editor_state(&editor);
    let err = rt.eval(r#"(kb-delete "note:doomed")"#).unwrap_err();
    assert!(err.message.contains("No KB node"), "{}", err.message);
}

/// `kb-search` finds what was written and does NOT find what was not — the
/// second half being the part a search that returns everything would fail.
#[test]
fn kb_search_returns_matches_and_excludes_non_matches() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node(
            "note:zebra",
            "Zebra crossing",
            "stripes and hooves",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    editor
        .kb_create_node(
            "note:kettle",
            "Kettle boiling",
            "water and steam",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    rt.inject_editor_state(&editor);

    let hits = rt.eval(r#"(kb-search "zebra")"#).unwrap();
    assert!(hits.contains("note:zebra"), "expected the match: {hits}");
    assert!(
        !hits.contains("note:kettle"),
        "an unrelated node must not match: {hits}"
    );

    // A query matching nothing is an empty list, not an error and not
    // everything.
    let none = rt.eval(r#"(kb-search "xyzzy-no-such-term")"#).unwrap();
    assert!(!none.contains("note:"), "expected no hits, got: {none}");
}

/// The queries a MAE user actually types are full of `:` and `-` — every node
/// id in the shipped KB looks like `note:probe-buffer` or `note:probe-kb-share`. Those
/// are CozoDB full-text-search *operators*, so passing them through raw turns
/// the most natural query into a parser error. This pins that they behave as
/// term searches instead, and that a punctuation-only query returns nothing
/// rather than everything (the failure mode of naively stripping to "").
#[test]
fn kb_search_treats_punctuation_as_separators_not_operators() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node(
            "note:probe-buffer",
            "Buffer",
            "the rope-backed text container",
            mae_kb::NodeKind::Concept,
        )
        .unwrap();
    editor
        .kb_create_node(
            "note:probe-kb-share",
            "probe kb-share command",
            "shares a knowledge base",
            mae_kb::NodeKind::Command,
        )
        .unwrap();
    rt.inject_editor_state(&editor);

    for query in [
        "note:probe-buffer",
        "kb-share",
        "rope-backed",
        "buffer*",
        "\\\"buffer\\\"",
        "buffer -- rope",
    ] {
        rt.eval(&format!(r#"(kb-search "{query}")"#))
            .unwrap_or_else(|e| panic!("{query:?} must not be a parse error: {}", e.message));
    }

    // Not vacuous: the hyphenated query still finds the right node.
    let hits = rt.eval(r#"(kb-search "rope-backed")"#).unwrap();
    assert!(
        hits.contains("note:probe-buffer"),
        "hyphenated query should still match: {hits}"
    );

    // A query with no usable term matches NOTHING, not everything — the
    // failure mode of sanitizing to the empty string, which `fts_search`
    // treats as "list all".
    for junk in ["---", "!!!", "::"] {
        let out = rt.eval(&format!(r#"(kb-search "{junk}")"#)).unwrap();
        assert!(
            !out.contains("note:probe-buffer") && !out.contains("note:probe-kb-share"),
            "{junk:?} must not match everything: {out}"
        );
    }

    // …while an explicitly empty query does list everything, matching
    // `KbStore::fts_search`'s own documented contract.
    let all = rt.eval(r#"(kb-search "")"#).unwrap();
    assert!(
        all.contains("note:probe-buffer"),
        "empty query lists all: {all}"
    );
}

/// LIMIT caps results, and a nonsense LIMIT/SCOPE is refused rather than
/// silently coerced — a `(kb-search q "everything")` typo must not quietly
/// fall back to a narrower or wider scope than the caller asked for.
#[test]
fn kb_search_validates_its_scope_and_limit() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);

    let err = rt.eval(r#"(kb-search "q" "everything")"#).unwrap_err();
    assert!(err.message.contains("unknown SCOPE"), "{}", err.message);

    for bad in ["0", "-1", "-100"] {
        let err = rt
            .eval(&format!(r#"(kb-search "q" "primary" {bad})"#))
            .unwrap_err();
        assert!(err.message.contains("LIMIT"), "{}", err.message);
    }

    // Both valid scopes are accepted.
    for scope in ["primary", "local", "all"] {
        rt.eval(&format!(r#"(kb-search "q" "{scope}")"#))
            .unwrap_or_else(|e| panic!("{scope} should be accepted: {}", e.message));
    }
}

/// With no KB store at all, the reads must degrade to "nothing found" rather
/// than panicking or erroring — an editor started with `--no-kb` still runs
/// user config.
#[test]
fn kb_reads_without_a_store_are_empty_not_fatal() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    assert_eq!(rt.eval(r#"(kb-get "anything")"#).unwrap(), "#f");
    let hits = rt.eval(r#"(kb-search "anything")"#).unwrap();
    assert!(!hits.contains("note:"), "{hits}");
    // …but a write against a node that cannot be found still errors, rather
    // than pretending to succeed.
    assert!(rt.eval(r#"(kb-delete "anything")"#).is_err());
}

// ---------------------------------------------------------------------------
// set-option-save!
// ---------------------------------------------------------------------------

/// An unknown option name is refused at call time. Without this the value
/// would be queued, fail on the next tick, and the Scheme caller would never
/// learn that its config line did nothing.
#[test]
fn set_option_save_rejects_an_unknown_option() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    rt.set_option_values(vec![
        ("theme".to_string(), "default".to_string()),
        ("tab_width".to_string(), "4".to_string()),
    ]);

    for bad in ["not_an_option", "themee", "TAB_WIDTH_X"] {
        let err = rt
            .eval(&format!(r#"(set-option-save! "{bad}" "x")"#))
            .unwrap_err();
        assert!(
            err.message.contains("Unknown option") && err.message.contains(bad),
            "{bad}: {}",
            err.message
        );
    }
}

/// A registered option is accepted under either spelling — the registry itself
/// treats `-` and `_` as the same separator, and a validation layer that
/// disagreed with it would reject calls the editor would have honoured.
#[test]
fn set_option_save_accepts_both_separator_spellings() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    rt.set_option_values(vec![("tab_width".to_string(), "4".to_string())]);

    assert_eq!(
        rt.eval(r#"(set-option-save! "tab_width" "8")"#).unwrap(),
        "#t"
    );
    assert_eq!(
        rt.eval(r#"(set-option-save! "tab-width" "2")"#).unwrap(),
        "#t"
    );
}

// ---------------------------------------------------------------------------
// LSP — the async boundary
// ---------------------------------------------------------------------------

/// The core honesty property: a queued LSP request reads as `pending`, and an
/// id that was never issued reads as an *error*. Collapsing those two would
/// make "the server has not answered" indistinguishable from "you asked about
/// nothing", and a polling loop could never terminate.
#[test]
fn lsp_result_distinguishes_pending_from_an_unknown_id() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.inject_editor_state(&editor);

    // Nothing issued yet: every id is unknown.
    for id in ["0", "1", "999999", "-5"] {
        let err = rt.eval(&format!("(lsp-result {id})")).unwrap_err();
        assert!(
            err.message.contains("unknown request id"),
            "id {id}: {}",
            err.message
        );
    }

    // A request against a buffer with no file path cannot reach a server, so
    // it must come back as a completed ERROR — not as an id that polls
    // forever.
    let id = rt.eval("(lsp-hover)").unwrap();
    rt.apply_to_editor(&mut editor);
    rt.inject_editor_state(&editor);
    let err = rt.eval(&format!("(lsp-result {id})")).unwrap_err();
    assert!(
        !err.message.contains("unknown request id"),
        "the id must be known once issued: {}",
        err.message
    );
    assert!(
        err.message.contains("LSP unavailable")
            || err.message.contains("No language server")
            || err.message.contains("no file path"),
        "the error should explain why no server was reached: {}",
        err.message
    );
}

/// Request ids are distinct and monotonic across primitives and across evals.
/// A reused id would silently hand one caller another caller's answer.
#[test]
fn lsp_request_ids_are_distinct_and_never_zero() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    let mut ids = Vec::new();
    for call in [
        "(lsp-definition)",
        "(lsp-references)",
        "(lsp-hover)",
        r#"(lsp-workspace-symbol "X" "rust")"#,
        "(lsp-document-symbols)",
        "(lsp-definition)",
    ] {
        let id: i64 = rt.eval(call).unwrap().parse().unwrap();
        assert!(id > 0, "{call} returned id {id}; 0 must never be valid");
        ids.push(id);
    }
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "request ids collided: {ids:?}");
    assert!(
        ids.windows(2).all(|w| w[1] > w[0]),
        "ids must be monotonic: {ids:?}"
    );
}

/// Position arguments are validated as 1-indexed. A 0 slipping through would
/// underflow to the last line/column on the server side, which is a silently
/// wrong answer rather than a visible failure.
#[test]
fn lsp_position_arguments_reject_zero_and_negative_indices() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    for call in [
        r#"(lsp-hover #f 0 1)"#,
        r#"(lsp-hover #f 1 0)"#,
        r#"(lsp-hover #f -3 1)"#,
        r#"(lsp-definition #f 1 -1)"#,
    ] {
        let err = rt.eval(call).unwrap_err();
        assert!(
            err.message.contains("1-indexed"),
            "{call} should be refused: {}",
            err.message
        );
    }
    // …and the boundary value 1 IS accepted.
    rt.eval(r#"(lsp-hover #f 1 1)"#)
        .expect("line/col 1 is the first valid position");
}

/// `lsp-workspace-symbol` requires both arguments — it cannot infer a language
/// server from a buffer, so a one-argument call must fail loudly rather than
/// guessing.
#[test]
fn lsp_workspace_symbol_requires_both_arguments() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    assert!(rt.eval(r#"(lsp-workspace-symbol "X")"#).is_err());
    assert!(rt.eval("(lsp-workspace-symbol)").is_err());
}

/// With no language server running there are no diagnostics, and the payload
/// must still be well-formed structured data with zeroed counts — a caller
/// doing `(assq "counts" (lsp-diagnostics))` must not have to special-case it.
#[test]
fn lsp_diagnostics_without_a_server_is_a_well_formed_empty_payload() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    for scope in ["", r#" "buffer""#, r#" "all""#] {
        let out = rt.eval(&format!("(lsp-diagnostics{scope})")).unwrap();
        assert!(out.contains("counts"), "scope {scope:?}: {out}");
        assert!(out.contains("files"), "scope {scope:?}: {out}");
        assert!(out.contains("total"), "scope {scope:?}: {out}");
    }

    let err = rt.eval(r#"(lsp-diagnostics "everything")"#).unwrap_err();
    assert!(err.message.contains("unknown SCOPE"), "{}", err.message);
}

// ---------------------------------------------------------------------------
// DAP
// ---------------------------------------------------------------------------

/// Every control primitive that requires a live session refuses when there
/// isn't one, and says so in the same words the MCP tool uses. The `dap-start`
/// case is the counterexample that proves these are real preconditions rather
/// than a blanket refusal: starting a session is precisely the operation that
/// must work without one.
#[test]
fn dap_control_primitives_refuse_without_a_session() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    for call in [
        "(dap-continue)",
        "(dap-step-over)",
        "(dap-step-into)",
        "(dap-step-out)",
    ] {
        let err = rt.eval(call).unwrap_err();
        assert!(
            err.message.contains("No active debug session"),
            "{call}: {}",
            err.message
        );
    }

    // dap-start does NOT require a session — it creates one.
    rt.eval(r#"(dap-start "lldb" "/bin/true")"#)
        .expect("dap-start must be callable without an existing session");
    // Nor does setting a breakpoint, which is meaningful before launch.
    rt.eval(r#"(dap-set-breakpoint "src/main.rs" 10)"#)
        .expect("breakpoints may be set before a session exists");
}

/// The read primitives with no session: `debug-state` answers `#f` (a normal
/// "nothing to report" branch) while `dap-inspect-variable` errors (asking for
/// a specific variable when nothing is running is a caller mistake, not an
/// empty result). The two must not be conflated.
#[test]
fn dap_reads_without_a_session_answer_distinctly() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    assert_eq!(rt.eval("(debug-state)").unwrap(), "#f");

    let err = rt.eval(r#"(dap-inspect-variable "count")"#).unwrap_err();
    assert!(
        err.message.contains("No active debug session"),
        "{}",
        err.message
    );
}

/// Breakpoint lines are 1-indexed and validated, for the same reason LSP
/// positions are: a 0 would land somewhere the caller did not ask for.
#[test]
fn dap_set_breakpoint_rejects_non_positive_lines() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    for line in ["0", "-1", "-999"] {
        let err = rt
            .eval(&format!(r#"(dap-set-breakpoint "src/main.rs" {line})"#))
            .unwrap_err();
        assert!(
            err.message.contains("1-indexed"),
            "line {line}: {}",
            err.message
        );
    }
    rt.eval(r#"(dap-set-breakpoint "src/main.rs" 1)"#)
        .expect("line 1 is valid");
}

/// A `dap-start` with an unknown adapter must surface as an error the Scheme
/// caller can see. It cannot surface at call time (the adapter table lives on
/// `Editor`), so it lands as a status message on apply — this test pins that
/// the dispatch genuinely fails rather than silently starting nothing.
#[test]
fn dap_start_with_an_unknown_adapter_fails_on_dispatch() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.inject_editor_state(&editor);

    rt.eval(r#"(dap-start "not-a-real-adapter" "/bin/true")"#)
        .expect("the call itself queues");
    rt.apply_to_editor(&mut editor);

    assert!(
        editor.dap.state.is_none(),
        "an unknown adapter must not have produced a session"
    );
}

/// Argument-shape errors are caught at call time, before anything is queued —
/// a list containing a non-string is a caller bug, and queueing it would defer
/// the error to a status line the script cannot read.
#[test]
fn dap_start_rejects_a_malformed_argument_list() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);

    assert!(rt
        .eval(r#"(dap-start "lldb" "/bin/true" '("ok" 42))"#)
        .is_err());
    assert!(rt.eval(r#"(dap-start "lldb" "/bin/true" 42)"#).is_err());
    // A well-formed list, and the omitted-args case, both work.
    rt.eval(r#"(dap-start "lldb" "/bin/true" '("--flag" "value"))"#)
        .expect("a list of strings is valid");
    rt.eval(r#"(dap-start "lldb" "/bin/true")"#)
        .expect("ARGS is optional");
}
