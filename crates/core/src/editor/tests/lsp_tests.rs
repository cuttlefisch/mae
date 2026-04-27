use super::*;
use crate::buffer::Buffer;
use crate::keymap::{parse_key_seq, parse_key_seq_spaced, KeyPress};
use crate::{LookupResult, Mode, VisualType};
use std::fs;

#[test]
fn m6_m7_commands_registered() {
    let editor = Editor::new();
    let cmds = [
        "join-lines",
        "indent-line",
        "dedent-line",
        "toggle-case",
        "uppercase-line",
        "lowercase-line",
        "alternate-file",
        "shell-command",
    ];
    for cmd in &cmds {
        assert!(
            editor.commands.contains(cmd),
            "Command '{}' not registered",
            cmd
        );
    }
}

#[test]
fn m6_m7_keybindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    assert_eq!(
        normal.lookup(&parse_key_seq("J")),
        LookupResult::Exact("join-lines")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq(">>")),
        LookupResult::Exact("indent-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("<<")),
        LookupResult::Exact("dedent-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("~")),
        LookupResult::Exact("toggle-case")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("g U U")),
        LookupResult::Exact("uppercase-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("g u u")),
        LookupResult::Exact("lowercase-line")
    );
    assert_eq!(
        normal.lookup(&[KeyPress::ctrl('6')]),
        LookupResult::Exact("alternate-file")
    );
}

#[test]
fn normal_keymap_has_lsp_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap");
    assert_eq!(
        normal.lookup(&parse_key_seq("gd")),
        LookupResult::Exact("lsp-goto-definition")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("gr")),
        LookupResult::Exact("lsp-find-references")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("K")),
        LookupResult::Exact("lsp-hover")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("]d")),
        LookupResult::Exact("lsp-next-diagnostic")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("[d")),
        LookupResult::Exact("lsp-prev-diagnostic")
    );
}

#[test]
fn dispatch_lsp_next_diagnostic_moves_cursor() {
    use crate::editor::DiagnosticSeverity;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    buf.insert_text_at(0, "line0\nline1\nline2\n");
    let mut editor = Editor::with_buffer(buf);
    editor.diagnostics.set(
        "file:///tmp/test.rs".into(),
        vec![crate::editor::Diagnostic {
            line: 2,
            col_start: 1,
            col_end: 3,
            end_line: 2,
            severity: DiagnosticSeverity::Error,
            message: "boom".into(),
            source: None,
            code: None,
        }],
    );
    editor.dispatch_builtin("lsp-next-diagnostic");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn dispatch_lsp_show_diagnostics_opens_buffer() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("lsp-show-diagnostics");
    assert_eq!(editor.active_buffer().name, "*Diagnostics*");
}

#[test]
fn colon_diagnostics_opens_buffer() {
    let mut editor = Editor::new();
    editor.execute_command("diagnostics");
    assert_eq!(editor.active_buffer().name, "*Diagnostics*");
}

#[test]
fn dispatch_lsp_goto_definition_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-goto-definition");
    assert_eq!(editor.pending_lsp_requests.len(), 1);
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::GotoDefinition { .. }
    ));
}

#[test]
fn dispatch_lsp_hover_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-hover");
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::Hover { .. }
    ));
}

#[test]
fn dispatch_lsp_find_references_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-find-references");
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::FindReferences { .. }
    ));
}

// --- Tree-sitter syntax highlighting (Phase 4b M1/M2) ---

#[test]
fn with_buffer_attaches_rust_language_from_extension() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/example.rs"));
    let editor = Editor::with_buffer(buf);
    assert_eq!(
        editor.syntax.language_of(0),
        Some(crate::syntax::Language::Rust)
    );
}

#[test]
fn with_buffer_without_file_has_no_language() {
    let editor = Editor::with_buffer(Buffer::new());
    assert_eq!(editor.syntax.language_of(0), None);
}

#[test]
fn open_file_detects_language_for_toml() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "[package]\nname = \"mae\"").unwrap();
    drop(f);

    let mut editor = Editor::new();
    editor.open_file(path.to_str().unwrap());
    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.syntax.language_of(idx),
        Some(crate::syntax::Language::Toml)
    );
}

#[test]
fn record_edit_invalidates_syntax_cache() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    // Prime the cache
    let _ = editor.syntax.spans_for(0, "fn x() {}", 0);
    // Force invalidation via the edit-recording path
    editor.record_edit("delete-line");
    // After invalidate, a fresh call should produce spans again. The generation
    // bump (or explicit invalidate) causes recompute against new source.
    let spans = editor.syntax.spans_for(0, "let y = 42;", 1).unwrap();
    assert!(spans.iter().any(|s| s.theme_key == "keyword"));
}

#[test]
fn kill_buffer_removes_syntax_entry_for_scratch_fallback() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    assert!(editor.syntax.language_of(0).is_some());
    editor.dispatch_builtin("kill-buffer");
    // Single-buffer case replaces with scratch; syntax entry must be cleared.
    assert_eq!(editor.syntax.language_of(0), None);
}

#[test]
fn kill_buffer_shifts_syntax_indices() {
    // Two buffers: 0 rust, 1 toml. Kill index 0 -> former 1 becomes 0.
    let mut buf0 = Buffer::new();
    buf0.set_file_path(std::path::PathBuf::from("/tmp/a.rs"));
    let mut editor = Editor::with_buffer(buf0);

    let mut buf1 = Buffer::new();
    buf1.set_file_path(std::path::PathBuf::from("/tmp/b.toml"));
    editor.buffers.push(buf1);
    editor.syntax.set_language(1, crate::syntax::Language::Toml);

    editor.window_mgr.focused_window_mut().buffer_idx = 0;
    editor.dispatch_builtin("kill-buffer");

    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(
        editor.syntax.language_of(0),
        Some(crate::syntax::Language::Toml)
    );
}

// --- from syntax_tests ---

#[test]
fn syntax_select_node_enters_visual() {
    let mut editor = ed_with_rust("fn main() {}");
    assert!(editor.syntax_select_node());
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    // Selection should cover some bytes.
    let (start, end) = editor.visual_selection_range();
    assert!(end > start);
}

#[test]
fn syntax_select_node_no_language_fails() {
    let mut editor = Editor::new();
    assert!(!editor.syntax_select_node());
    assert!(editor.status_msg.contains("No language"));
}

#[test]
fn syntax_expand_selection_grows_to_parent() {
    let mut editor = ed_with_rust("fn main() { let x = 1; }");
    // Place cursor inside the body on the 'x' identifier (column 16).
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 16;
    // Select the innermost node at cursor.
    assert!(editor.syntax_select_node());
    let initial = editor.visual_selection_range();
    // Expand to parent.
    assert!(editor.syntax_expand_selection());
    let expanded = editor.visual_selection_range();
    // Parent should strictly contain the child range.
    assert!(
        expanded.0 <= initial.0 && expanded.1 >= initial.1,
        "expanded {:?} does not contain {:?}",
        expanded,
        initial
    );
    assert!(
        expanded.1 - expanded.0 > initial.1 - initial.0,
        "expansion did not grow the range ({:?} vs {:?})",
        expanded,
        initial
    );
}

#[test]
fn syntax_contract_selection_restores_previous() {
    let mut editor = ed_with_rust("fn main() { let x = 1; }");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 16;
    assert!(editor.syntax_select_node());
    let initial = editor.visual_selection_range();
    assert!(editor.syntax_expand_selection());
    assert!(editor.syntax_contract_selection());
    let after = editor.visual_selection_range();
    assert_eq!(after, initial);
}

#[test]
fn syntax_contract_without_stack_reports_status() {
    let mut editor = ed_with_rust("fn main() {}");
    assert!(!editor.syntax_contract_selection());
    assert!(editor.status_msg.contains("No prior"));
}

#[test]
fn syntax_tree_sexp_contains_function_item() {
    let mut editor = ed_with_rust("fn main() {}");
    let sexp = editor.syntax_tree_sexp().unwrap();
    assert!(sexp.contains("function_item"), "sexp: {}", sexp);
}

#[test]
fn syntax_node_kind_at_cursor_on_keyword() {
    let mut editor = ed_with_rust("fn main() {}");
    // Cursor at (0,0) — 'f' of 'fn'
    let kind = editor.syntax_node_kind_at_cursor().unwrap();
    // Either the keyword itself or the wrapping function item — just
    // assert we got a non-empty kind.
    assert!(!kind.is_empty());
}
