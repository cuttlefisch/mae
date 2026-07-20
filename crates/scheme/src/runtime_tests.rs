// @ai-caution: [architecture-debt] `runtime.rs`'s extracted `#[cfg(test)]
// mod tests` — was ~1,526 lines when split out, now ~2,020 (~32% growth
// since), well over the 500-line test-file ceiling. A further split into
// focused sibling test files (mirroring `runtime/*.rs`'s category split of
// the non-test code) is a reasonable future candidate, not attempted here.
// Tracked in .claude/commands/mae-audit.md's "Known exceptions" and
// ROADMAP.md's "Architecture Debt" section; re-measure each audit pass
// rather than trusting this comment's line count to stay current.

use super::*;
use mae_core::{parse_key_seq, CommandSource, Editor};

fn new_runtime() -> SchemeRuntime {
    SchemeRuntime::new().unwrap()
}

#[test]
fn new_runtime_creates_successfully() {
    let rt = SchemeRuntime::new();
    assert!(rt.is_ok());
}

#[test]
fn load_source_evaluates_in_memory_content() {
    // load_source is how embedded modules are loaded (no filesystem).
    let mut rt = new_runtime();
    rt.load_source(
        "(define embedded-test-var 42)",
        "embedded:test/autoloads.scm",
    )
    .expect("valid in-memory source should evaluate");
    let out = rt.eval("embedded-test-var").unwrap();
    assert!(
        out.contains("42"),
        "define from load_source should take effect: {out}"
    );
    // Malformed source surfaces an error rather than silently succeeding.
    assert!(
        rt.load_source("(((", "embedded:test/bad.scm").is_err(),
        "malformed in-memory source should error"
    );
}

#[test]
fn eval_arithmetic() {
    let mut rt = new_runtime();
    let result = rt.eval("(+ 1 2 3)").unwrap();
    assert_eq!(result, "6");
}

#[test]
fn eval_string_ops() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(string-append "hello" " " "world")"#).unwrap();
    assert_eq!(result, "hello world");
}

#[test]
fn eval_boolean() {
    let mut rt = new_runtime();
    assert_eq!(rt.eval("(= 1 1)").unwrap(), "#t");
    assert_eq!(rt.eval("(= 1 2)").unwrap(), "#f");
}

#[test]
fn define_key_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
    rt.apply_to_editor(&mut editor);

    let keymap = editor.keymaps.get("normal").unwrap();
    let seq = parse_key_seq("Q");
    assert_eq!(keymap.lookup(&seq), mae_core::LookupResult::Exact("quit"));
}

#[test]
fn define_command_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(define-command "greet" "Say hello" "greet-fn")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);

    let cmd = editor.commands.get("greet").unwrap();
    assert_eq!(cmd.doc, "Say hello");
    assert_eq!(cmd.source, CommandSource::Scheme("greet-fn".into()));
}

#[test]
fn set_status_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-status "Hello from Scheme!")"#).unwrap();
    rt.apply_to_editor(&mut editor);

    assert_eq!(editor.status_msg, "Hello from Scheme!");
}

#[test]
fn inject_and_read_editor_state() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    // Insert some text so we have state to read
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'h');
    }
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'i');
    }

    rt.inject_editor_state(&editor);
    let result = rt.eval("*cursor-col*").unwrap();
    assert_eq!(result, "2");

    let result = rt.eval("*buffer-line-count*").unwrap();
    assert_eq!(result, "1");
}

#[test]
fn load_file_works() {
    let dir = std::env::temp_dir().join("mae_test_scheme_load");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("test.scm");
    std::fs::write(&path, r#"(define-key "normal" "Q" "my-custom-save")"#).unwrap();

    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.load_file(&path).unwrap();
    rt.apply_to_editor(&mut editor);

    let keymap = editor.keymaps.get("normal").unwrap();
    assert_eq!(
        keymap.lookup(&parse_key_seq("Q")),
        mae_core::LookupResult::Exact("my-custom-save")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn define_key_spaced_sequence_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(define-key "normal" "SPC t t" "my-custom-cmd")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);

    let keymap = editor.keymaps.get("normal").unwrap();
    let seq = mae_core::parse_key_seq_spaced("SPC t t");
    assert_eq!(
        keymap.lookup(&seq),
        mae_core::LookupResult::Exact("my-custom-cmd")
    );
}

/// Regression test for the audit finding that an unrecognized
/// `register-ai-tool!` permission string silently downgraded to `Write`
/// (crates/ai's `scheme_tools_to_definitions`) instead of failing safe. The
/// real fix is at the registration boundary itself: an invalid permission
/// must reject the whole `register-ai-tool!` call, at the point where the
/// Scheme author will actually see the error, not silently grant more
/// access than intended much later during tool-definition conversion.
#[test]
fn register_ai_tool_rejects_unknown_permission() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(register-ai-tool! "my-tool" "desc" "my-handler" "bogus")"#);
    assert!(
        result.is_err(),
        "an unrecognized permission string must be rejected, not silently downgraded"
    );
}

#[test]
fn register_ai_tool_accepts_each_known_permission() {
    let mut rt = new_runtime();
    for (i, perm) in ["read", "readonly", "write", "shell", "privileged"]
        .iter()
        .enumerate()
    {
        let src = format!(r#"(register-ai-tool! "tool-{i}" "desc" "handler-{i}" "{perm}")"#,);
        rt.eval(&src)
            .unwrap_or_else(|e| panic!("permission '{perm}' should be accepted, got: {e}"));
    }
}

#[test]
fn eval_error_returns_scheme_error() {
    let mut rt = new_runtime();
    let result = rt.eval("(undefined-function)");
    assert!(result.is_err());
}

#[test]
fn eval_error_recorded_in_history() {
    let mut rt = new_runtime();
    let _ = rt.eval("(undefined-function)");
    let errors = rt.last_errors();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].expression.contains("undefined-function"));
    assert!(!errors[0].error_message.is_empty());
    assert_eq!(errors[0].seq, 1);
}

#[test]
fn set_theme_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-theme "gruvbox-dark")"#).unwrap();
    rt.apply_to_editor(&mut editor);

    assert_eq!(editor.theme.name, "gruvbox-dark");
}

#[test]
fn list_user_commands_after_define() {
    let mut rt = new_runtime();
    rt.eval(r#"(define-command "greet" "Say hello" "greet-fn")"#)
        .unwrap();
    let cmds = rt.list_user_commands();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].0, "greet");
}

#[test]
fn list_keybindings_after_define() {
    let mut rt = new_runtime();
    rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
    let bindings = rt.list_keybindings();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0], ("normal".into(), "Q".into(), "quit".into()));
}

#[test]
fn define_keymap_creates_with_parent() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(define-keymap "python" "normal")"#).unwrap();
    rt.eval(r#"(define-key "python" "C-c" "run-python-buffer")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);

    let km = editor.keymaps.get("python").unwrap();
    assert_eq!(km.parent.as_deref(), Some("normal"));
    let seq = parse_key_seq("C-c");
    assert_eq!(
        km.lookup(&seq),
        mae_core::LookupResult::Exact("run-python-buffer")
    );
}

#[test]
fn eval_for_debug_works() {
    let mut rt = new_runtime();
    let result = rt.eval_for_debug("(+ 10 20)").unwrap();
    assert_eq!(result, "30");
}

// --- New API surface tests ---

#[test]
fn buffer_text_global_available() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'A');
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'B');
    }
    rt.inject_editor_state(&editor);
    let result = rt.eval("*buffer-text*").unwrap();
    assert_eq!(result, "AB");
}

#[test]
fn mode_global_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    assert_eq!(rt.eval("*mode*").unwrap(), "normal");
}

#[test]
fn kb_lifecycle_primitives_queue_collab_intents() {
    // P2: first-class `(kb-…)` primitives route through the SAME CollabIntent
    // the commands + MCP tools use (no execute-ex strings).
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval("(kb-add-member \"team\" \"SHA256:bob\" \"viewer\")")
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(matches!(
        &editor.collab.pending_intent,
        Some(mae_core::CollabIntent::KbAddMember { kb_id, member, role })
            if kb_id == "team" && member == "SHA256:bob" && role == "viewer"
    ));

    editor.collab.pending_intent = None;
    rt.eval("(kb-set-policy \"team\" \"permissive\")").unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(matches!(
        &editor.collab.pending_intent,
        Some(mae_core::CollabIntent::KbSetPolicy { kb_id, policy })
            if kb_id == "team" && policy == "permissive"
    ));

    editor.collab.pending_intent = None;
    rt.eval("(kb-leave \"team\")").unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(matches!(
        &editor.collab.pending_intent,
        Some(mae_core::CollabIntent::LeaveKb { kb_id }) if kb_id == "team"
    ));
}

#[test]
fn kb_set_ai_residency_primitive_mutates_registry_via_ex_command() {
    // ADR-048: unlike the KB-sharing primitives above, `(kb-set-ai-residency ...)` is
    // NOT a collab/daemon action — it queues a plain ex-command string
    // (`pending_ex_commands`) that `apply_to_editor` runs through the same
    // `execute_command` → `dispatch_kb` path as typing `:kb-set-ai-residency ...`
    // would, synchronously mutating the local KB registry — no `CollabIntent` involved.
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval("(kb-set-ai-residency \"primary\" \"local_models_only\")")
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        editor.kb.registry.primary_ai_residency,
        mae_kb::federation::AiResidency::LocalModelsOnly
    );
    assert!(
        editor.collab.pending_intent.is_none(),
        "kb-set-ai-residency must not go through the collab-intent path"
    );

    rt.eval("(kb-set-ai-residency \"primary\" \"open\")")
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        editor.kb.registry.primary_ai_residency,
        mae_kb::federation::AiResidency::Open
    );
}

#[test]
fn kb_set_role_primitive_stamps_property_via_ex_command() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor
        .kb_create_node(
            "note:role-scheme-test",
            "Test",
            "body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();

    rt.eval("(kb-set-role \"note:role-scheme-test\" \"hub\")")
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        editor
            .kb
            .primary
            .get("note:role-scheme-test")
            .unwrap()
            .properties
            .get("role"),
        Some(&"hub".to_string())
    );
    assert!(
        editor.collab.pending_intent.is_none(),
        "kb-set-role must not go through the collab-intent path"
    );
}

/// Open an in-memory `CozoKbStore`, seed its schema, and wire it as
/// `editor.kb.store` — the shared setup for the `kb-graph`/`kb-neighborhood`/
/// `kb-related`/`kb-shortest-path` primitive tests below (mirrors the
/// existing pattern at `crates/core/src/editor/command.rs:1451-1453`).
fn editor_with_cozo_store() -> Editor {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor
}

#[test]
fn kb_graph_primitive_walks_the_primary_store_and_shares_bfs_with_mcp() {
    // Closes the parity gap: `kb_graph` (MCP tool) had no Scheme
    // counterpart. The walk itself is shared with the MCP executor via
    // `mae_kb::graph_query::bfs_neighborhood` — this test exercises the
    // Scheme-side wiring of that shared function through a real `KbStore`.
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("graph:a", "Node A", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_create_node("graph:b", "Node B", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_create_node("graph:c", "Node C", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_update_node("graph:a", None, Some("Links to [[graph:b]]."), None)
        .unwrap();
    editor
        .kb_update_node("graph:b", None, Some("Links to [[graph:c]]."), None)
        .unwrap();

    rt.inject_editor_state(&editor);

    // depth 1: reaches graph:b, NOT graph:c.
    let out = rt.eval("(kb-graph \"graph:a\" 1)").unwrap();
    assert!(out.contains("graph:a"), "root echoed back: {out}");
    assert!(out.contains("graph:b"), "1-hop neighbor present: {out}");
    assert!(
        !out.contains("graph:c"),
        "2-hop node must NOT appear at depth 1: {out}"
    );

    // depth 2: reaches graph:c too.
    let out2 = rt.eval("(kb-graph \"graph:a\" 2)").unwrap();
    assert!(
        out2.contains("graph:c"),
        "2-hop node must appear at depth 2: {out2}"
    );

    // default depth (no second arg) matches the MCP tool's default of 1.
    let out3 = rt.eval("(kb-graph \"graph:a\")").unwrap();
    assert!(!out3.contains("graph:c"), "default depth must be 1: {out3}");
}

#[test]
fn kb_graph_primitive_errors_on_unknown_root() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);
    let err = rt.eval("(kb-graph \"no:such:node\" 1)").unwrap_err();
    assert!(
        err.message.contains("No KB node"),
        "error should name the missing node: {}",
        err.message
    );
}

#[test]
fn kb_graph_primitive_without_a_store_returns_empty_list_not_an_error() {
    // No CozoDB store configured (the common in-process-only case) — degrades
    // gracefully to '() like every other kb-* read primitive in this file,
    // rather than erroring.
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let out = rt.eval("(kb-graph \"anything\" 1)").unwrap();
    assert_eq!(out, "()");
}

#[test]
fn kb_neighborhood_primitive_returns_typed_edges() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("nbhd:a", "A", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_create_node("nbhd:b", "B", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_update_node("nbhd:a", None, Some("Links to [[nbhd:b]]."), None)
        .unwrap();

    rt.inject_editor_state(&editor);
    let out = rt.eval("(kb-neighborhood \"nbhd:a\" 2)").unwrap();
    assert!(out.contains("nbhd:a"), "root echoed back: {out}");
    assert!(out.contains("nbhd:b"), "neighbor present: {out}");
}

#[test]
fn kb_related_primitive_returns_empty_list_for_unknown_node() {
    let mut rt = new_runtime();
    let editor = editor_with_cozo_store();
    rt.inject_editor_state(&editor);
    let out = rt.eval("(kb-related \"no:such:node\" 5)").unwrap();
    assert_eq!(out, "()");
}

#[test]
fn kb_related_primitive_does_not_error_on_a_real_node() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("rel:a", "A", "", mae_kb::NodeKind::Note)
        .unwrap();
    rt.inject_editor_state(&editor);
    // Not asserting on ranked content (CozoKbStore::related's heuristic is
    // exercised directly in shared/kb's own tests) — just that the Scheme
    // wiring round-trips through a real store without erroring.
    assert!(rt.eval("(kb-related \"rel:a\" 5)").is_ok());
}

#[test]
fn kb_shortest_path_primitive_finds_a_direct_link() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("path:a", "A", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_create_node("path:b", "B", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_update_node("path:a", None, Some("Links to [[path:b]]."), None)
        .unwrap();

    rt.inject_editor_state(&editor);
    let out = rt.eval("(kb-shortest-path \"path:a\" \"path:b\")").unwrap();
    assert!(out.contains("path:a"), "{out}");
    assert!(out.contains("path:b"), "{out}");
}

#[test]
fn kb_shortest_path_primitive_empty_for_disconnected_nodes() {
    let mut rt = new_runtime();
    let mut editor = editor_with_cozo_store();
    editor
        .kb_create_node("iso:a", "A", "", mae_kb::NodeKind::Note)
        .unwrap();
    editor
        .kb_create_node("iso:b", "B", "", mae_kb::NodeKind::Note)
        .unwrap();
    // No link between them.
    rt.inject_editor_state(&editor);
    let out = rt.eval("(kb-shortest-path \"iso:a\" \"iso:b\")").unwrap();
    assert_eq!(out, "()");
}

#[test]
fn kb_sharing_status_primitive_returns_snapshot_json() {
    // P0: users can script KB-sharing introspection — `(kb-sharing-status)`
    // returns the same JSON snapshot the buffer + MCP tool expose.
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.collab.local_fingerprint = "mefp".to_string();
    let coll = mae_sync::kb::KbCollectionDoc::new_owned("Team", "mefp", "me");
    editor
        .collab
        .kb_collection_state
        .insert("team".to_string(), coll.encode_state());
    rt.inject_editor_state(&editor);
    let json = rt.eval("(kb-sharing-status)").unwrap();
    // The Scheme string is the JSON snapshot; it names our KB + owner role.
    assert!(json.contains("\"team\""), "snapshot names the KB: {json}");
    assert!(
        json.contains("\"owner\""),
        "snapshot shows the owner role: {json}"
    );
}

#[test]
fn daemon_capability_primitives_expose_the_model() {
    // ADR-035 parity: Scheme sees the same capability model as AI + commands.
    let mut rt = new_runtime();
    let editor = Editor::new(); // no daemon wired → floor only
    rt.inject_editor_state(&editor);

    // (daemon-available?) → #f with no daemon.
    assert_eq!(rt.eval("(daemon-available?)").unwrap(), "#f");

    // (daemon-status) → JSON naming the mode + features.
    let status = rt.eval("(daemon-status)").unwrap();
    assert!(status.contains("\"mode\""), "status has mode: {status}");
    assert!(
        status.contains("p2p-sharing"),
        "status enumerates features: {status}"
    );

    // (feature-available? "p2p-sharing") → unavailable + a fix, with no daemon.
    let p2p = rt.eval("(feature-available? \"p2p-sharing\")").unwrap();
    assert!(p2p.contains("\"requirement\":\"requires\""), "p2p: {p2p}");
    assert!(
        p2p.contains("\"available\":false"),
        "p2p unavailable w/o daemon: {p2p}"
    );
    assert!(p2p.contains("\"fix\""), "p2p carries a fix: {p2p}");

    // local-kb is the floor — always available.
    let local = rt.eval("(feature-available? \"local-kb\")").unwrap();
    assert!(
        local.contains("\"available\":true"),
        "local-kb always available: {local}"
    );

    // Unknown id → an error object naming known ids.
    let bogus = rt.eval("(feature-available? \"nope\")").unwrap();
    assert!(bogus.contains("\"error\""), "unknown id errors: {bogus}");
}

#[test]
fn buffer_line_function_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        for ch in "hello\nworld".chars() {
            editor.buffers[0].insert_char(win, ch);
        }
    }
    rt.inject_editor_state(&editor);
    let line0 = rt.eval("(buffer-line 0)").unwrap();
    assert!(line0.contains("hello"));
    let line1 = rt.eval("(buffer-line 1)").unwrap();
    assert!(line1.contains("world"));
    // Out-of-range returns empty string
    let line99 = rt.eval("(buffer-line 99)").unwrap();
    assert_eq!(line99, "");
}

#[test]
fn buffer_insert_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(buffer-insert "hello")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers[0].text(), "hello");
}

#[test]
fn cursor_goto_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        for ch in "abc\ndef\nghi".chars() {
            editor.buffers[0].insert_char(win, ch);
        }
    }
    rt.eval("(cursor-goto 1 2)").unwrap();
    rt.apply_to_editor(&mut editor);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 1);
    assert_eq!(win.cursor_col, 2);
}

#[test]
fn run_command_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    // search-forward-start switches to Search mode.
    rt.eval(r#"(run-command "search-forward-start")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.mode, mae_core::Mode::Search);
}

#[test]
fn eval_for_repl_formats_output() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    let output = rt.eval_for_repl("(+ 1 2)", &mut editor);
    assert!(output.contains("> (+ 1 2)"));
    assert!(output.contains("; => 3"));
}

#[test]
fn eval_for_repl_formats_error() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    let output = rt.eval_for_repl("(undefined-fn)", &mut editor);
    assert!(output.contains("> (undefined-fn)"));
    assert!(output.contains("; error:"));
}

#[test]
fn multiple_define_keys_in_sequence() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(
        r#"
            (define-key "normal" "j" "move-down")
            (define-key "normal" "k" "move-up")
            (define-key "normal" "dd" "delete-line")
        "#,
    )
    .unwrap();
    rt.apply_to_editor(&mut editor);

    let km = editor.keymaps.get("normal").unwrap();
    assert_eq!(
        km.lookup(&parse_key_seq("j")),
        mae_core::LookupResult::Exact("move-down")
    );
    assert_eq!(
        km.lookup(&parse_key_seq("k")),
        mae_core::LookupResult::Exact("move-up")
    );
    assert_eq!(
        km.lookup(&parse_key_seq("dd")),
        mae_core::LookupResult::Exact("delete-line")
    );
}

// --- Hook system tests ---

#[test]
fn add_hook_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(add-hook! "before-save" "my-save-fn")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);

    assert_eq!(editor.hooks.get("before-save"), &["my-save-fn"]);
}

#[test]
fn remove_hook_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(add-hook! "after-save" "fn-a")"#).unwrap();
    rt.eval(r#"(add-hook! "after-save" "fn-b")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.hooks.get("after-save").len(), 2);

    rt.eval(r#"(remove-hook! "after-save" "fn-a")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.hooks.get("after-save"), &["fn-b"]);
}

#[test]
fn add_hook_any_name_succeeds() {
    // Hook namespace is open — modules can define custom hooks.
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(add-hook! "custom-module-hook" "fn")"#).unwrap();
    rt.apply_to_editor(&mut editor);

    assert_eq!(editor.hooks.get("custom-module-hook"), &["fn"]);
}

// --- set-option! tests ---

#[test]
fn set_option_line_numbers() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    assert!(editor.show_line_numbers); // default true

    rt.eval(r#"(set-option! "line-numbers" "false")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(!editor.show_line_numbers);
}

#[test]
fn set_option_word_wrap() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    assert!(!editor.word_wrap); // default false

    rt.eval(r#"(set-option! "word-wrap" "true")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor.word_wrap);
}

#[test]
fn set_option_relative_line_numbers() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-option! "relative-line-numbers" "on")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor.relative_line_numbers);
}

#[test]
fn set_option_theme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-option! "theme" "gruvbox-dark")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.theme.name, "gruvbox-dark");
}

#[test]
fn set_option_show_break() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-option! "show-break" ">> ")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.show_break, ">> ");
}

#[test]
fn set_option_unknown_warns() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(set-option! "nonexistent" "value")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor.status_msg.contains("Unknown option"));
}

// --- Shell state tests ---

#[test]
fn test_shell_cwd_returns_cached_value() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor
        .shell
        .viewport_cwds
        .insert(1, "/home/user".to_string());
    rt.inject_editor_state(&editor);
    let result = rt.eval("(shell-cwd 1)").unwrap();
    assert_eq!(result, "/home/user");
}

#[test]
fn test_shell_read_output_returns_viewport() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor
        .shell
        .viewports
        .insert(2, vec!["$ ls".to_string(), "file.txt".to_string()]);
    rt.inject_editor_state(&editor);
    let result = rt.eval("(shell-read-output 2 10)").unwrap();
    assert!(result.contains("$ ls"));
    assert!(result.contains("file.txt"));
}

#[test]
fn test_shell_list_with_buffers() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor
        .buffers
        .push(mae_core::Buffer::new_shell("*terminal*"));
    rt.inject_editor_state(&editor);
    let result = rt.eval("*shell-buffers*").unwrap();
    // Should contain the index of the shell buffer (1).
    assert!(result.contains("1"));
}

#[test]
fn test_recent_files_and_projects() {
    let mut editor = Editor::new();
    let mut runtime = new_runtime();

    // Initially empty
    assert_eq!(editor.recent_files.len(), 0);
    assert_eq!(editor.recent_projects.len(), 0);

    // Evaluate scheme calls (use non-temp paths since temp dirs are rejected)
    runtime
        .eval("(recent-files-add! \"/home/testuser/test.txt\")")
        .unwrap();
    runtime
        .eval("(recent-projects-add! \"/home/testuser/project\")")
        .unwrap();

    // Apply to editor
    runtime.apply_to_editor(&mut editor);

    // Verify editor state updated
    assert_eq!(editor.recent_files.len(), 1);
    assert_eq!(
        editor.recent_files.list()[0],
        std::path::PathBuf::from("/home/testuser/test.txt")
    );
    assert_eq!(editor.recent_projects.len(), 1);
    assert_eq!(
        editor.recent_projects.list()[0],
        std::path::PathBuf::from("/home/testuser/project")
    );
}

// --- Round 2: buffer editing API tests ---

#[test]
fn buffer_text_range_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "Hello, World!");
    rt.inject_editor_state(&editor);
    let result = rt.eval("(buffer-text-range 0 5)").unwrap();
    assert_eq!(result, "Hello");
}

#[test]
fn buffer_text_range_out_of_bounds() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "Hi");
    rt.inject_editor_state(&editor);
    let result = rt.eval("(buffer-text-range 0 100)").unwrap();
    assert_eq!(result, "Hi");
}

#[test]
fn buffer_delete_range_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "Hello, World!");
    rt.eval("(buffer-delete-range 5 13)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers[0].text(), "Hello");
}

#[test]
fn buffer_replace_range_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "Hello, World!");
    rt.eval(r#"(buffer-replace-range 7 12 "Scheme")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers[0].text(), "Hello, Scheme!");
}

#[test]
fn buffer_undo_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'A');
    }
    assert_eq!(editor.buffers[0].text(), "A");
    rt.eval("(buffer-undo)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers[0].text(), "");
}

#[test]
fn buffer_redo_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'X');
    }
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].undo(win);
    }
    assert_eq!(editor.buffers[0].text(), "");
    rt.eval("(buffer-redo)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers[0].text(), "X");
}

// --- Round 2: buffer list API tests ---

#[test]
fn buffer_list_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("(length *buffer-list*)").unwrap();
    assert!(result.parse::<i32>().unwrap() >= 1);
}

#[test]
fn get_buffer_by_name_found() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(get-buffer-by-name "[scratch]")"#).unwrap();
    assert_eq!(result, "0");
}

#[test]
fn get_buffer_by_name_not_found() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(get-buffer-by-name "nonexistent")"#).unwrap();
    assert_eq!(result, "#f");
}

#[test]
fn switch_to_buffer_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    // Add a second buffer manually
    editor.buffers.push(mae_core::Buffer::new());
    editor.buffers[1].name = "second".to_string();
    // Switch to it via Scheme, then back to 0
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    assert_eq!(editor.active_buffer_idx(), 1);
    rt.eval("(switch-to-buffer 0)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.active_buffer_idx(), 0);
}

// --- Round 2: window API tests ---

#[test]
fn window_count_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("*window-count*").unwrap();
    assert_eq!(result, "1");
}

#[test]
fn window_list_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("(length *window-list*)").unwrap();
    assert_eq!(result, "1");
}

// --- Round 2: option + command introspection tests ---

#[test]
fn command_exists_true() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(command-exists? "save")"#).unwrap();
    assert_eq!(result, "#t");
}

#[test]
fn command_exists_false() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(command-exists? "nonexistent-cmd")"#).unwrap();
    assert_eq!(result, "#f");
}

#[test]
fn command_list_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("(length *command-list*)").unwrap();
    let count: i32 = result.parse().unwrap();
    assert!(
        count > 10,
        "should have many builtin commands, got {}",
        count
    );
}

#[test]
fn option_list_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("(length *option-list*)").unwrap();
    let count: i32 = result.parse().unwrap();
    assert!(count >= 10, "should have many options, got {}", count);
}

#[test]
fn get_option_returns_value() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(get-option "scroll_speed")"#).unwrap();
    assert_eq!(result, "3");
}

#[test]
fn get_option_unknown_returns_false() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(get-option "nonexistent_option")"#).unwrap();
    assert_eq!(result, "#f");
}

// --- Round 2: keymap introspection tests ---

#[test]
fn keymap_list_available() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval("(length *keymap-list*)").unwrap();
    let count: i32 = result.parse().unwrap();
    assert!(
        count >= 2,
        "should have normal + insert keymaps, got {}",
        count
    );
}

#[test]
fn keymap_bindings_returns_list() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt.eval(r#"(length (keymap-bindings "normal"))"#).unwrap();
    let count: i32 = result.parse().unwrap();
    assert!(count > 0, "normal keymap should have bindings");
}

#[test]
fn keymap_bindings_unknown_returns_empty() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    let result = rt
        .eval(r#"(length (keymap-bindings "nonexistent"))"#)
        .unwrap();
    assert_eq!(result, "0");
}

#[test]
fn undefine_key_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(define-key "normal" "Q" "quit")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        editor
            .keymaps
            .get("normal")
            .unwrap()
            .lookup(&parse_key_seq("Q")),
        mae_core::LookupResult::Exact("quit")
    );
    rt.eval(r#"(undefine-key! "normal" "Q")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(
        editor
            .keymaps
            .get("normal")
            .unwrap()
            .lookup(&parse_key_seq("Q")),
        mae_core::LookupResult::None
    );
}

#[test]
fn set_group_name_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    // Add some bindings under SPC z prefix
    rt.eval(r#"(define-key "normal" "SPC z a" "quit")"#)
        .unwrap();
    rt.eval(r#"(define-key "normal" "SPC z b" "save")"#)
        .unwrap();
    rt.eval(r#"(set-group-name "normal" "SPC z" "+test-group")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let normal = editor.keymaps.get("normal").unwrap();
    let spc = mae_core::parse_key_seq_spaced("SPC");
    let entries = normal.which_key_entries(&spc, &editor.commands);
    let z_entry = entries
        .iter()
        .find(|e| matches!(e.key.key, mae_core::Key::Char('z')));
    assert!(z_entry.is_some(), "SPC should have a 'z' group");
    assert_eq!(z_entry.unwrap().label, "+test-group");
}

#[test]
fn runtime_define_key_updates_keymap() {
    let mut rt = new_runtime();
    let mut ed = Editor::new();
    rt.eval(r#"(define-key "normal" "SPC z z" "quit")"#)
        .unwrap();
    rt.apply_to_editor(&mut ed);
    let normal = ed.keymaps.get("normal").unwrap();
    assert_eq!(
        normal.lookup(&mae_core::parse_key_seq_spaced("SPC z z")),
        mae_core::LookupResult::Exact("quit")
    );
}

// --- Round 2: file I/O tests ---

#[test]
fn file_exists_check() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(file-exists? "/tmp")"#).unwrap();
    assert_eq!(result, "#t");
}

#[test]
fn file_exists_false() {
    let mut rt = new_runtime();
    let result = rt
        .eval(r#"(file-exists? "/tmp/nonexistent_file_12345")"#)
        .unwrap();
    assert_eq!(result, "#f");
}

#[test]
fn read_file_works() {
    let mut rt = new_runtime();
    let test_path = "/tmp/mae_test_read_file.txt";
    std::fs::write(test_path, "test content").unwrap();
    let result = rt.eval(&format!(r#"(read-file "{}")"#, test_path)).unwrap();
    assert_eq!(result, "test content");
    let _ = std::fs::remove_file(test_path);
}

#[test]
fn read_file_missing_returns_error() {
    let mut rt = new_runtime();
    let result = rt
        .eval(r#"(read-file "/tmp/nonexistent_file_99999")"#)
        .unwrap();
    assert!(result.starts_with("ERROR:"));
}

#[test]
fn list_directory_works() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(length (list-directory "/tmp"))"#).unwrap();
    let count: i32 = result.parse().unwrap();
    assert!(count >= 0);
}

// --- Round 2: hook tests ---

#[test]
fn new_hooks_valid() {
    use mae_core::hooks::HookRegistry;
    assert!(HookRegistry::is_valid("option-change"));
    assert!(HookRegistry::is_valid("before-revert"));
    assert!(HookRegistry::is_valid("after-revert"));
    assert!(HookRegistry::is_valid("window-split"));
    assert!(HookRegistry::is_valid("window-close"));
    assert!(HookRegistry::is_valid("option-change:scroll_speed"));
}

#[test]
fn buffer_char_count_injected() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "ABCDE");
    rt.inject_editor_state(&editor);
    let result = rt.eval("*buffer-char-count*").unwrap();
    assert_eq!(result, "5");
}

// --- Package infrastructure tests ---

#[test]
fn require_feature_not_found() {
    let mut rt = new_runtime();
    let result = rt.require_feature("nonexistent_feature_xyz");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found in load-path"));
}

#[test]
fn provide_marks_feature() {
    let mut rt = new_runtime();
    // provide-feature is the Rust-registered canonical name.
    rt.eval(r#"(provide-feature "my-feature")"#).unwrap();
    {
        let state = rt.shared.lock();
        assert!(
            state.loaded_features.contains("my-feature"),
            "SharedState should contain 'my-feature', got: {:?}",
            state.loaded_features
        );
    }
    let result = rt.eval(r#"(featurep "my-feature")"#).unwrap();
    assert_eq!(result, "#t");
}

#[test]
fn load_path_default() {
    let rt = new_runtime();
    assert_eq!(rt.load_path.len(), 2);
    let paths: Vec<String> = rt
        .load_path
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    assert!(
        paths[0].ends_with("mae/packages"),
        "first entry should be packages dir: {}",
        paths[0]
    );
    assert!(
        paths[1].ends_with("mae/lisp"),
        "second entry should be lisp dir: {}",
        paths[1]
    );
}

#[test]
fn add_to_load_path() {
    let mut rt = new_runtime();
    rt.eval(r#"(add-to-load-path! "/tmp/mae-test-packages")"#)
        .unwrap();
    // Sync from SharedState.
    rt.process_requires();
    assert_eq!(rt.load_path.len(), 3);
    assert_eq!(
        rt.load_path[0].display().to_string(),
        "/tmp/mae-test-packages"
    );
}

#[test]
fn featurep_false_initially() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(featurep "unknown-feature")"#).unwrap();
    assert_eq!(result, "#f");
}

#[test]
fn require_already_loaded_is_noop() {
    let mut rt = new_runtime();
    // Manually mark as loaded.
    rt.loaded_features.insert("already-loaded".to_string());
    let result = rt.require_feature("already-loaded");
    assert!(result.is_ok());
}

#[test]
fn require_feature_loads_and_provides() {
    let dir = std::env::temp_dir().join("mae_test_require");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("test-pkg.scm"), r#"(provide-feature "test-pkg")"#).unwrap();

    let mut rt = new_runtime();
    rt.load_path.insert(0, dir.clone());
    let result = rt.require_feature("test-pkg");
    assert!(result.is_ok(), "require_feature failed: {:?}", result);
    assert!(rt.loaded_features.contains("test-pkg"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autoload_registers_command() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(autoload "my-cmd" "my-pkg" "My autoloaded command")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let cmd = editor.commands.get("my-cmd").unwrap();
    assert_eq!(cmd.doc, "My autoloaded command");
    assert_eq!(
        cmd.source,
        CommandSource::Autoload {
            feature: "my-pkg".into()
        }
    );
}

#[test]
fn module_loaded_query() {
    let mut rt = SchemeRuntime::new().unwrap();
    // No modules registered → module-loaded? returns false
    let result = rt.eval(r#"(module-loaded? "dashboard")"#).unwrap();
    assert!(result.contains("f"), "expected false, got: {}", result);

    // Register a module → returns true
    rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
        .unwrap();
    let result = rt.eval(r#"(module-loaded? "dashboard")"#).unwrap();
    assert!(result.contains("t"), "expected true, got: {}", result);
}

#[test]
fn module_version_query() {
    let mut rt = SchemeRuntime::new().unwrap();
    let result = rt.eval(r#"(module-version "dashboard")"#).unwrap();
    assert!(result.contains("f"), "expected false, got: {}", result);

    rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
        .unwrap();
    let result = rt.eval(r#"(module-version "dashboard")"#).unwrap();
    assert!(
        result.contains("0.1.0"),
        "expected version, got: {}",
        result
    );
}

#[test]
fn module_list_query() {
    let mut rt = SchemeRuntime::new().unwrap();
    let result = rt.eval("(module-list)").unwrap();
    // Empty list
    assert!(
        result.contains("()"),
        "expected empty list, got: {}",
        result
    );

    rt.eval(r#"(register-module! "dashboard" "0.1.0")"#)
        .unwrap();
    let result = rt.eval("(module-list)").unwrap();
    assert!(
        result.contains("dashboard"),
        "expected dashboard, got: {}",
        result
    );
}

#[test]
fn define_option_applies() {
    let mut rt = SchemeRuntime::new().unwrap();
    rt.eval(r#"(define-option! "my_option" "string" "hello" "A test option")"#)
        .unwrap();
    let mut editor = Editor::new();
    rt.apply_to_editor(&mut editor);
    let def = editor.option_registry.find("my_option");
    assert!(def.is_some(), "dynamic option should be registered");
    assert_eq!(def.unwrap().default_value.as_ref(), "hello");
}

#[test]
fn undefine_command_applies() {
    let mut rt = SchemeRuntime::new().unwrap();
    let mut editor = Editor::new();
    // Editor starts with built-in commands
    assert!(editor.commands.get("move-left").is_some());
    rt.eval(r#"(undefine-command! "move-left")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor.commands.get("move-left").is_none());
}

#[test]
fn unload_feature_removes() {
    let mut rt = SchemeRuntime::new().unwrap();
    rt.eval(r#"(provide-feature "test-mod")"#).unwrap();
    // Check via unload return value — true means it was present
    let result = rt.eval(r#"(unload-feature "test-mod")"#).unwrap();
    assert!(
        result.contains("t"),
        "expected true (was loaded), got: {}",
        result
    );
    // Second unload should return false
    let result = rt.eval(r#"(unload-feature "test-mod")"#).unwrap();
    assert!(
        result.contains("f"),
        "expected false (already removed), got: {}",
        result
    );
}

#[test]
fn deprecation_warns_once() {
    let mut rt = SchemeRuntime::new().unwrap();
    rt.eval(r#"(deprecate-function! "old-fn" "new-fn" "0.9.0")"#)
        .unwrap();

    // First check-deprecated returns true
    let result = rt.eval(r#"(check-deprecated "old-fn")"#).unwrap();
    assert!(result.contains("t"), "expected true, got: {}", result);

    // Non-deprecated returns false
    let result = rt.eval(r#"(check-deprecated "new-fn")"#).unwrap();
    assert!(result.contains("f"), "expected false, got: {}", result);

    // Verify a warning message was queued
    let state = rt.shared.lock();
    assert!(
        state
            .pending_messages
            .iter()
            .any(|m| m.contains("deprecated")),
        "expected deprecation warning in messages"
    );
}

// ── mae! / package! declarative config tests ────────────────

#[test]
fn mae_bang_parses_modules() {
    let mut rt = new_runtime();
    rt.eval(r#"(mae! :editor "surround" "search")"#).unwrap();
    let decl = rt.declared_modules();
    assert!(decl.contains_key("surround"), "expected surround");
    assert!(decl.contains_key("search"), "expected search");
    assert_eq!(decl.len(), 2);
}

#[test]
fn mae_bang_parses_flags() {
    let mut rt = new_runtime();
    rt.eval(r#"(mae! :editor (list "multicursor" "+align" "+fancy"))"#)
        .unwrap();
    let decl = rt.declared_modules();
    let flags = decl.get("multicursor").unwrap();
    assert!(flags.contains(&"+align".to_string()));
    assert!(flags.contains(&"+fancy".to_string()));
}

#[test]
fn mae_bang_categories_are_labels() {
    let mut rt = new_runtime();
    rt.eval(r#"(mae! :editor "surround" :ui "dashboard" :lang "tables")"#)
        .unwrap();
    let decl = rt.declared_modules();
    assert_eq!(decl.len(), 3);
    assert!(decl.contains_key("surround"));
    assert!(decl.contains_key("dashboard"));
    assert!(decl.contains_key("tables"));
}

#[test]
fn package_bang_basic() {
    let mut rt = new_runtime();
    rt.eval(r#"(package! "org-roam" :source "github:user/mae-org-roam")"#)
        .unwrap();
    let pkgs = rt.declared_packages();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0].name, "org-roam");
    assert_eq!(pkgs[0].source.as_deref(), Some("github:user/mae-org-roam"));
    assert!(!pkgs[0].disable);
}

#[test]
fn package_bang_pin() {
    let mut rt = new_runtime();
    rt.eval(r#"(package! "my-theme" :source "github:u/r" :pin "abc123")"#)
        .unwrap();
    let pkgs = rt.declared_packages();
    assert_eq!(pkgs[0].pin.as_deref(), Some("abc123"));
}

#[test]
fn package_bang_disable() {
    let mut rt = new_runtime();
    rt.eval(r#"(package! "dashboard" :disable #t)"#).unwrap();
    let pkgs = rt.declared_packages();
    assert!(pkgs[0].disable);
}

#[test]
fn define_kb_node_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval(r#"(define-kb-node! "module:test:guide" "Test Guide" "Some body text")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);

    let node = editor.kb.primary.get("module:test:guide");
    assert!(node.is_some(), "expected kb node to be registered");
    assert_eq!(node.unwrap().title, "Test Guide");
}

#[test]
fn undeclared_modules_not_in_declared() {
    let mut rt = new_runtime();
    rt.eval(r#"(mae! :editor "surround")"#).unwrap();
    let decl = rt.declared_modules();
    assert!(!decl.contains_key("dashboard"), "dashboard not declared");
    assert!(decl.contains_key("surround"), "surround declared");
}

// --- Phase A: New Scheme API tests ---

#[test]
fn string_split_works() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(string-split "a,b,c" ",")"#).unwrap();
    assert!(result.contains("a"));
    assert!(result.contains("b"));
    assert!(result.contains("c"));
}

#[test]
fn string_join_works() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(string-join '("a" "b" "c") ",")"#).unwrap();
    assert_eq!(result, "a,b,c");
}

#[test]
fn string_trim_works() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(string-trim "  hello  ")"#).unwrap();
    assert_eq!(result, "hello");
}

#[test]
fn string_contains_works() {
    let mut rt = new_runtime();
    assert_eq!(
        rt.eval(r#"(string-contains? "hello world" "world")"#)
            .unwrap(),
        "#t"
    );
    assert_eq!(
        rt.eval(r#"(string-contains? "hello" "xyz")"#).unwrap(),
        "#f"
    );
}

#[test]
fn string_replace_works() {
    let mut rt = new_runtime();
    let result = rt
        .eval(r#"(string-replace "hello world" "world" "rust")"#)
        .unwrap();
    assert_eq!(result, "hello rust");
}

#[test]
fn string_upcase_downcase_works() {
    let mut rt = new_runtime();
    assert_eq!(rt.eval(r#"(string-upcase "hello")"#).unwrap(), "HELLO");
    assert_eq!(rt.eval(r#"(string-downcase "HELLO")"#).unwrap(), "hello");
}

#[test]
fn shell_command_works() {
    let mut rt = new_runtime();
    let result = rt.eval(r#"(shell-command "echo hello")"#).unwrap();
    assert_eq!(result.trim(), "hello");
}

#[test]
fn create_buffer_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    let initial_count = editor.buffers.len();
    rt.eval(r#"(create-buffer "test-buf")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers.len(), initial_count + 1);
    assert_eq!(editor.buffers.last().unwrap().name, "test-buf");
}

#[test]
fn kill_buffer_by_name_works() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    // Create a buffer first
    let mut buf = mae_core::Buffer::new();
    buf.name = "kill-me".to_string();
    editor.buffers.push(buf);
    assert_eq!(editor.buffers.len(), 2);
    rt.eval(r#"(kill-buffer-by-name "kill-me")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert_eq!(editor.buffers.len(), 1);
}

#[test]
fn buffer_introspection_functions() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        for ch in "hello\nworld".chars() {
            editor.buffers[0].insert_char(win, ch);
        }
    }
    rt.inject_editor_state(&editor);
    assert_eq!(rt.eval("(current-line-number)").unwrap(), "2");
    // point-min is always 0
    assert_eq!(rt.eval("(point-min)").unwrap(), "0");
    // point-max = total chars
    let pmax = rt.eval("(point-max)").unwrap();
    assert!(pmax.parse::<i64>().unwrap() > 0);
    // current-buffer-name
    let name = rt.eval("(current-buffer-name)").unwrap();
    assert!(!name.is_empty());
}

#[test]
fn region_inactive_in_normal_mode() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    assert_eq!(rt.eval("(region-active?)").unwrap(), "#f");
}

#[test]
fn advice_add_and_remove() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(advice-add! "save" ":before" "my-before-save")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let before = editor
        .hooks
        .get_advice("save", mae_core::hooks::AdviceKind::Before);
    assert_eq!(before, vec!["my-before-save"]);

    rt.eval(r#"(advice-remove! "save" "my-before-save")"#)
        .unwrap();
    rt.apply_to_editor(&mut editor);
    let before = editor
        .hooks
        .get_advice("save", mae_core::hooks::AdviceKind::Before);
    assert!(before.is_empty());
}

#[test]
fn current_command_variable_exists() {
    let mut rt = new_runtime();
    let editor = Editor::new();
    rt.inject_editor_state(&editor);
    // Should not error — variable exists
    let result = rt.eval("*current-command*").unwrap();
    assert!(result.is_empty());
}

// --- Native KB graph view primitives (Part C Phase 1) ---
//
// Same 3-step pattern as `define_key_from_scheme` etc.: eval, then
// `apply_to_editor` to drain the queued `GraphViewIntent` into the matching
// `Editor::kb_graph_view_*` method — confirming the Scheme primitive and the
// `Editor` method (also used by the MCP tool + keybinding) are the same
// code path, not a parallel reimplementation (CLAUDE.md principle #3).

#[test]
fn kb_graph_view_open_from_scheme_creates_graph_buffer() {
    let mut rt = new_runtime();
    let mut editor = Editor::new(); // seeds the built-in "index" node

    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);

    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .expect("kb-graph-view-open should create a Graph buffer");
    assert_eq!(
        editor.buffers[idx]
            .graph_view()
            .unwrap()
            .center_node
            .as_deref(),
        Some("index")
    );
    assert_eq!(editor.buffers[idx].graph_view().unwrap().depth, 1);
}

#[test]
fn kb_graph_view_open_defaults_center_and_depth_when_omitted() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();

    rt.eval("(kb-graph-view-open)").unwrap();
    rt.apply_to_editor(&mut editor);

    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .unwrap();
    assert_eq!(
        editor.buffers[idx]
            .graph_view()
            .unwrap()
            .center_node
            .as_deref(),
        Some("index")
    );
    assert_eq!(
        editor.buffers[idx].graph_view().unwrap().depth,
        editor.kb_graph_default_depth
    );
}

#[test]
fn kb_graph_view_state_from_scheme_is_false_when_not_open() {
    let mut rt = new_runtime();
    let editor = Editor::new();

    rt.inject_editor_state(&editor);
    let result = rt.eval("(kb-graph-view-state)").unwrap();
    assert_eq!(result, "#f");
}

#[test]
fn kb_graph_view_state_from_scheme_reflects_open_graph() {
    let mut rt = new_runtime();
    let mut editor = Editor::new(); // seeds the built-in "index" node

    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);

    // Read-only snapshot: refresh the injected state now that the graph
    // buffer exists, then query it — mirrors `option_values`'s
    // snapshot-per-eval pattern (see `inject_graph_view_state`'s doc
    // comment).
    rt.inject_editor_state(&editor);
    let result = rt.eval("(kb-graph-view-state)").unwrap();
    assert!(result.contains("index"), "got: {result}");
    assert!(!result.starts_with("#f"), "got: {result}");
}

#[test]
fn kb_preview_show_from_scheme_populates_popup() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.open_help_at("index"); // active buffer must be KB-kind
    rt.eval(r#"(kb-preview-show "index")"#).unwrap();
    rt.apply_to_editor(&mut editor);

    let popup = editor
        .kb_preview_popup()
        .expect("kb-preview-show should populate the popup");
    assert!(popup.contents.contains("MAE Help Index"));
}

#[test]
fn kb_preview_dismiss_from_scheme_clears_popup() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    editor.open_help_at("index");
    rt.eval(r#"(kb-preview-show "index")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor.kb_preview_popup().is_some());

    rt.eval("(kb-preview-dismiss)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(
        editor.kb_preview_popup().is_none(),
        "kb-preview-dismiss should clear the popup"
    );
}

#[test]
fn kb_preview_show_from_scheme_outside_kb_buffer_is_noop() {
    let mut rt = new_runtime();
    let mut editor = Editor::new(); // active buffer is scratch, not KB
    rt.eval(r#"(kb-preview-show "index")"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(
        editor.kb_preview_popup().is_none(),
        "kb-preview-show must not populate a popup outside a KB buffer"
    );
}

#[test]
fn kb_graph_view_close_from_scheme_removes_the_buffer() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(editor
        .buffers
        .iter()
        .any(|b| b.kind == mae_core::BufferKind::Graph));

    rt.eval("(kb-graph-view-close)").unwrap();
    rt.apply_to_editor(&mut editor);
    assert!(!editor
        .buffers
        .iter()
        .any(|b| b.kind == mae_core::BufferKind::Graph));
}

#[test]
fn kb_graph_view_refresh_from_scheme_is_a_no_op_when_not_open() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval("(kb-graph-view-refresh)").unwrap();
    // Must not panic/error even with nothing open.
    rt.apply_to_editor(&mut editor);
    assert!(!editor
        .buffers
        .iter()
        .any(|b| b.kind == mae_core::BufferKind::Graph));
}

#[test]
fn kb_graph_view_set_depth_from_scheme_updates_depth_in_place() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let window_count_before = editor.window_mgr.iter_windows().count();

    rt.eval("(kb-graph-view-set-depth 3)").unwrap();
    rt.apply_to_editor(&mut editor);

    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .unwrap();
    assert_eq!(editor.buffers[idx].graph_view().unwrap().depth, 3);
    assert_eq!(
        editor.window_mgr.iter_windows().count(),
        window_count_before,
        "set-depth must refresh in place, not re-split"
    );
}

#[test]
fn kb_graph_view_navigate_valid_direction_from_scheme() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);

    rt.eval(r#"(kb-graph-view-navigate "right")"#).unwrap();
    // Must not panic/error — the exact selection outcome depends on the
    // seeded KB's topology, so this just confirms the primitive routes
    // through to `Editor::kb_graph_view_navigate` without error.
    rt.apply_to_editor(&mut editor);
}

#[test]
fn kb_graph_view_navigate_invalid_direction_errors() {
    let mut rt = new_runtime();
    let err = rt
        .eval(r#"(kb-graph-view-navigate "sideways")"#)
        .unwrap_err();
    assert!(
        err.to_string().contains("sideways")
            || err.to_string().to_lowercase().contains("direction"),
        "error should mention the invalid direction: {err}"
    );
}

#[test]
fn kb_graph_view_select_current_from_scheme_navigates_companion() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);

    rt.eval("(kb-graph-view-select-current)").unwrap();
    rt.apply_to_editor(&mut editor);

    assert!(
        editor
            .buffers
            .iter()
            .any(|b| b.kind == mae_core::BufferKind::Kb),
        "select-current should have opened/found a KB buffer for the selected node"
    );
}

#[test]
fn kb_graph_view_zoom_to_from_scheme_sets_the_focused_windows_zoom() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .unwrap();
    let win_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.buffer_idx == idx)
        .map(|w| w.id)
        .unwrap();
    editor.window_mgr.set_focused(win_id);

    rt.eval("(kb-graph-view-zoom-to 4.0)").unwrap();
    rt.apply_to_editor(&mut editor);

    assert_eq!(
        editor.buffers[idx]
            .graph_view()
            .unwrap()
            .viewports
            .get(&win_id)
            .unwrap()
            .zoom,
        4.0
    );
}

#[test]
fn kb_graph_view_set_pinned_from_scheme_pins_and_repositions_a_node() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .unwrap();
    let node_id = editor.buffers[idx].graph_view().unwrap().scene.nodes[0]
        .id
        .clone();

    rt.eval(&format!(
        r#"(kb-graph-view-set-pinned "{node_id}" #t 7.0 8.0)"#
    ))
    .unwrap();
    rt.apply_to_editor(&mut editor);

    let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
    assert!(node.pinned);
    assert_eq!(node.x, 7.0);
    assert_eq!(node.y, 8.0);
}

#[test]
fn kb_graph_view_set_pinned_from_scheme_without_position_leaves_it_in_place() {
    let mut rt = new_runtime();
    let mut editor = Editor::new();
    rt.eval(r#"(kb-graph-view-open "index" 1)"#).unwrap();
    rt.apply_to_editor(&mut editor);
    let idx = editor
        .buffers
        .iter()
        .position(|b| b.kind == mae_core::BufferKind::Graph)
        .unwrap();
    let (node_id, x0, y0) = {
        let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
        (node.id.clone(), node.x, node.y)
    };

    rt.eval(&format!(r#"(kb-graph-view-set-pinned "{node_id}" #t)"#))
        .unwrap();
    rt.apply_to_editor(&mut editor);

    let node = &editor.buffers[idx].graph_view().unwrap().scene.nodes[0];
    assert!(node.pinned);
    assert_eq!(node.x, x0);
    assert_eq!(node.y, y0);
}

#[test]
fn kb_graph_view_set_pinned_from_scheme_rejects_a_lone_x_argument() {
    let mut rt = new_runtime();
    let err = rt
        .eval(r#"(kb-graph-view-set-pinned "index" #t 1.0)"#)
        .unwrap_err();
    assert!(
        err.to_string().contains("2 or 4"),
        "error should explain the expected arity: {err}"
    );
}
