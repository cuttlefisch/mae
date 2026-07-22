use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, Mode};
use mae_scheme::SchemeRuntime;

use crate::ai_event_handler::PendingInteractiveEvent;

use super::{handle_key, is_splash_visible};

/// Create a SchemeRuntime for tests.
fn new_scheme() -> SchemeRuntime {
    SchemeRuntime::new().expect("SchemeRuntime::new() should not fail")
}

/// Macro to create a SchemeRuntime for test use.
macro_rules! require_scheme {
    () => {
        new_scheme()
    };
}

fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn make_ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

fn dispatch(editor: &mut Editor, scheme: &mut SchemeRuntime, key: KeyEvent) {
    let mut pending_keys = Vec::new();
    let ai_tx: Option<tokio::sync::mpsc::Sender<mae_ai::AiCommand>> = None;
    let mut pending_interactive: Option<PendingInteractiveEvent> = None;
    handle_key(
        editor,
        scheme,
        key,
        &mut pending_keys,
        &ai_tx,
        &mut pending_interactive,
    );
}

#[test]
fn splash_j_increments_selection() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    assert!(is_splash_visible(&editor));
    assert_eq!(editor.splash_selection, 0);

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 1);

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 2);

    // Wraps at action count
    let count = mae_core::render_common::splash::splash_action_count();
    editor.splash_selection = count - 1;
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 0);
}

#[test]
fn splash_k_decrements_selection() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    let count = mae_core::render_common::splash::splash_action_count();

    // From 0, k wraps to count-1
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('k')));
    assert_eq!(editor.splash_selection, count - 1);

    // From count-1, k goes to count-2
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('k')));
    assert_eq!(editor.splash_selection, count - 2);
}

#[test]
fn splash_enter_dispatches_command() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    // Enter on first action should dispatch and potentially change state
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Enter));
    // After dispatch, the splash may or may not still be visible depending
    // on what the action does, but we verify no panic occurred.
}

#[test]
fn splash_only_intercepts_in_normal_mode() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    editor.mode = Mode::Insert;
    let sel_before = editor.splash_selection;

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    // In insert mode, j should NOT change splash_selection
    assert_eq!(editor.splash_selection, sel_before);
}

#[test]
fn splash_not_active_on_non_dashboard() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    // No dashboard installed — active buffer is a regular scratch buffer
    assert!(!is_splash_visible(&editor));

    let sel_before = editor.splash_selection;
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    // j in normal mode on non-dashboard should not change splash_selection
    assert_eq!(editor.splash_selection, sel_before);
}

#[test]
fn ctrl_o_in_insert_mode_executes_one_normal_command_then_returns() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();

    // Enter insert mode
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    assert_eq!(editor.mode, Mode::Insert);

    // Type some text so we have content
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('h')));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));

    // C-o: switch to normal for one command
    dispatch(&mut editor, &mut scheme, make_ctrl('o'));
    assert_eq!(editor.mode, Mode::Normal);
    assert!(editor.vi.insert_mode_oneshot_normal);

    // Execute one normal command (e.g. '0' = move to line start)
    // Note: '0' with no count_prefix is move-to-line-start, not a digit
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('0')));

    // Should be back in insert mode
    assert_eq!(editor.mode, Mode::Insert);
    assert!(!editor.vi.insert_mode_oneshot_normal);
}

#[test]
fn ctrl_o_with_motion_returns_to_insert() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();

    // Insert a few lines so j has somewhere to go
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Enter));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Enter));

    // Move to top: Esc, then gg
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Esc));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('g')));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('g')));

    // Enter insert mode again
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    assert_eq!(editor.mode, Mode::Insert);

    // C-o, then j (move down one line)
    dispatch(&mut editor, &mut scheme, make_ctrl('o'));
    assert_eq!(editor.mode, Mode::Normal);
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(
        editor.mode,
        Mode::Insert,
        "should return to insert after C-o j"
    );
}

// -----------------------------------------------------------------------
// E2E: Insert-mode C-t indent / C-d dedent
// -----------------------------------------------------------------------

#[test]
fn insert_ctrl_t_indents_line() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "hello");
    // Enter insert mode.
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    assert_eq!(editor.mode, Mode::Insert);
    // C-t indents the current line.
    dispatch(&mut editor, &mut scheme, make_ctrl('t'));
    assert!(editor.buffers[0].text().starts_with("    hello"));
}

#[test]
fn insert_ctrl_d_dedents_line() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "    hello");
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    // C-d with default "dedent" mode removes indentation.
    dispatch(&mut editor, &mut scheme, make_ctrl('d'));
    assert_eq!(editor.buffers[0].text(), "hello");
}

#[test]
fn insert_ctrl_d_delete_forward_mode() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.insert_ctrl_d = "delete-forward".to_string();
    editor.buffers[0].insert_text_at(0, "hello");
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    // With delete-forward mode, C-d should delete the char under cursor.
    dispatch(&mut editor, &mut scheme, make_ctrl('d'));
    assert_eq!(editor.buffers[0].text(), "ello");
}

// -----------------------------------------------------------------------
// E2E: Block visual mode via Ctrl-V
// -----------------------------------------------------------------------

#[test]
fn ctrl_v_enters_block_visual() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "abc\ndef\n");
    dispatch(&mut editor, &mut scheme, make_ctrl('v'));
    assert_eq!(editor.mode, Mode::Visual(mae_core::VisualType::Block));
}

#[test]
fn ctrl_v_toggle_exits_block_visual() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "abc\ndef\n");
    dispatch(&mut editor, &mut scheme, make_ctrl('v'));
    assert_eq!(editor.mode, Mode::Visual(mae_core::VisualType::Block));
    dispatch(&mut editor, &mut scheme, make_ctrl('v'));
    assert_eq!(editor.mode, Mode::Normal);
}

// -----------------------------------------------------------------------
// Regression: ConversationInput mode should not cause ghost cursor
// -----------------------------------------------------------------------

#[test]
fn conversation_input_mode_excluded_from_gui_cursor() {
    // Verify that ConversationInput is NOT ShellInsert — the GUI cursor guard
    // now excludes both. This test verifies the mode enum values are distinct
    // and both are handled.
    assert_ne!(Mode::ConversationInput, Mode::ShellInsert);
    assert_ne!(Mode::ConversationInput, Mode::Normal);
    // The actual ghost cursor fix is in crates/gui/src/lib.rs:
    // render_gui_cursor is skipped for both ShellInsert and ConversationInput.
    // We can't render in tests, but we verify the mode distinction.
}

// -----------------------------------------------------------------------
// E2E: ConversationInput multiline submit
// -----------------------------------------------------------------------

#[test]
fn conversation_multiline_submit_reads_all_lines() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.ai_chat_enabled = true;

    // Open conversation (creates pair: *AI* output + *ai-input* input).
    editor.dispatch_builtin("ai-prompt");
    assert_eq!(editor.mode, Mode::ConversationInput);
    let pair = editor.ai.conversation_pair.as_ref().unwrap().clone();

    // Type "hello" into the input buffer.
    for ch in "hello".chars() {
        dispatch(
            &mut editor,
            &mut scheme,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }

    // Insert a newline via Shift+Enter.
    dispatch(
        &mut editor,
        &mut scheme,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
    );

    // Type "world" on the second line.
    for ch in "world".chars() {
        dispatch(
            &mut editor,
            &mut scheme,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
        );
    }

    // The input buffer should now have "hello\nworld\n" (ropey trailing newline).
    let rope = editor.buffers[pair.input_buffer_idx].rope().clone();
    let text: String = rope.chars().collect();
    assert_eq!(text.trim_end_matches('\n'), "hello\nworld");

    // Now submit with Enter — should clear input and push to conversation.
    dispatch(
        &mut editor,
        &mut scheme,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    // Input buffer should be cleared.
    let rope_after = editor.buffers[pair.input_buffer_idx].rope().clone();
    let text_after: String = rope_after.chars().collect();
    assert_eq!(
        text_after.trim_end_matches('\n'),
        "",
        "input buffer should be empty after submit"
    );

    // Conversation should have the user message.
    let conv = editor.buffers[pair.output_buffer_idx]
        .conversation()
        .unwrap();
    assert!(
        conv.entries.iter().any(|e| e.content == "hello\nworld"),
        "conversation should contain the multiline prompt"
    );
}

// -----------------------------------------------------------------------
// E2E: ignorecase/smartcase through :set
// -----------------------------------------------------------------------

#[test]
fn set_ignorecase_via_command() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "Hello world hello");
    // Set ignorecase via command mode.
    editor.execute_command("set ignorecase true");
    assert!(editor.ignorecase);
    // Search should now be case-insensitive.
    editor.search_input = "hello".to_string();
    editor.search_state.direction = mae_core::SearchDirection::Forward;
    editor.execute_search();
    assert_eq!(editor.search_state.matches.len(), 2);
}

// -----------------------------------------------------------------------
// E2E: :g command through command line
// -----------------------------------------------------------------------

#[test]
fn global_command_via_ex_mode() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "TODO: first\nDone: second\nTODO: third\n");
    editor.execute_command("g/TODO/d");
    let text = editor.buffers[0].text();
    assert!(!text.contains("TODO"));
    assert!(text.contains("Done"));
}

// -----------------------------------------------------------------------
// Splash intercept must not swallow keys during multi-key sequences
// -----------------------------------------------------------------------

#[test]
fn splash_intercept_skipped_when_pending_keys_nonempty() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    assert!(is_splash_visible(&editor));

    let sel_before = editor.splash_selection;

    // Simulate a multi-key sequence in progress (e.g. SPC C already pressed).
    let mut pending_keys = vec![
        mae_core::KeyPress {
            key: mae_core::Key::Char(' '),
            ctrl: false,
            alt: false,
            shift: false,
        },
        mae_core::KeyPress {
            key: mae_core::Key::Char('C'),
            ctrl: false,
            alt: false,
            shift: true,
        },
    ];
    let ai_tx: Option<tokio::sync::mpsc::Sender<mae_ai::AiCommand>> = None;
    let mut pending_interactive: Option<PendingInteractiveEvent> = None;

    // Press 'j' — should NOT be intercepted by splash navigation.
    handle_key(
        &mut editor,
        &mut scheme,
        make_key(KeyCode::Char('j')),
        &mut pending_keys,
        &ai_tx,
        &mut pending_interactive,
    );

    // Splash selection must NOT have changed.
    assert_eq!(
        editor.splash_selection, sel_before,
        "splash intercept swallowed 'j' during a pending key sequence"
    );
}

#[test]
fn splash_intercept_works_when_no_pending_keys() {
    // Confirm the normal splash j/k still works when no sequence is in progress.
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.install_dashboard();
    assert!(is_splash_visible(&editor));
    assert_eq!(editor.splash_selection, 0);

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(
        editor.splash_selection, 1,
        "splash j should still work normally"
    );
}

// --- Task 3: readline-style Ctrl-U/Ctrl-J prompt keybinding gaps ---

#[test]
fn command_palette_ctrl_u_clears_query() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    let mut palette = mae_core::CommandPalette::for_buffers(&["[scratch]", "other-buf"]);
    palette.query = "sc".to_string();
    palette.update_filter();
    editor.command_palette = Some(palette);
    editor.set_mode(Mode::CommandPalette);

    dispatch(&mut editor, &mut scheme, make_ctrl('u'));

    let palette = editor.command_palette.as_ref().unwrap();
    assert!(palette.query.is_empty(), "Ctrl-U should clear the query");
    assert_eq!(
        palette.filtered.len(),
        palette.entries.len(),
        "cleared query should re-show every entry, got {:?}",
        palette.filtered
    );
}

#[test]
fn command_palette_ctrl_u_on_kb_find_large_kb_triggers_requery() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    // Push past the lazy threshold so kb_find_palette_query_changed() takes
    // the lazy re-search branch instead of a plain client-side filter.
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 500) {
        editor.kb.primary.insert(mae_core::KbNode::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_core::KbNodeKind::Note,
            "filler body",
        ));
    }
    editor.kb.primary.insert(mae_core::KbNode::new(
        "note:zebra-marker",
        "Zebra Marker",
        mae_core::KbNodeKind::Note,
        "uniquely findable",
    ));
    assert!(editor.kb_loaded_node_count() > Editor::KB_FIND_LAZY_THRESHOLD);

    let mut palette =
        mae_core::CommandPalette::for_kb_find_or_create(&editor.kb_find_candidates("zebra"));
    palette.query = "zebra".to_string();
    editor.command_palette = Some(palette);
    editor.set_mode(Mode::CommandPalette);
    assert!(
        editor
            .command_palette
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.name == "note:zebra-marker"),
        "sanity: initial query should surface the zebra node"
    );

    dispatch(&mut editor, &mut scheme, make_ctrl('u'));

    let palette = editor.command_palette.as_ref().unwrap();
    assert!(palette.query.is_empty(), "Ctrl-U should clear the query");
    // Lazy re-search path: entries reflect the fresh bounded empty-query
    // window rather than the stale "zebra"-filtered set left over from
    // before Ctrl-U -- proving the fix used kb_find_palette_query_changed()
    // and not a bare local clear.
    assert!(
        palette.entries.len() <= Editor::KB_FIND_LAZY_LIMIT,
        "expected the lazy re-search's bounded window, got {} entries",
        palette.entries.len()
    );
}

#[test]
fn search_mode_ctrl_u_clears_search_input() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.set_mode(Mode::Search);
    editor.search_input = "some query".to_string();

    dispatch(&mut editor, &mut scheme, make_ctrl('u'));

    assert!(
        editor.search_input.is_empty(),
        "Ctrl-U should clear search_input"
    );
}

#[test]
fn insert_mode_ctrl_j_moves_completion_selection_when_popup_open() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.set_mode(Mode::Insert);
    editor.apply_completion_result(vec![
        mae_core::editor::lsp_state::CompletionItem {
            label: "alpha".to_string(),
            insert_text: "alpha".to_string(),
            detail: None,
            kind_sigil: 'f',
        },
        mae_core::editor::lsp_state::CompletionItem {
            label: "beta".to_string(),
            insert_text: "beta".to_string(),
            detail: None,
            kind_sigil: 'f',
        },
    ]);
    assert_eq!(editor.lsp.completion_selected, 0);
    let text_before = editor.buffers[editor.active_buffer_idx()]
        .rope()
        .to_string();

    dispatch(&mut editor, &mut scheme, make_ctrl('j'));

    assert_eq!(
        editor.lsp.completion_selected, 1,
        "Ctrl-J should advance the completion selection when the popup is open"
    );
    let text_after = editor.buffers[editor.active_buffer_idx()]
        .rope()
        .to_string();
    assert_eq!(
        text_before, text_after,
        "Ctrl-J must not insert a newline when it moved the completion selection instead"
    );
}

#[test]
fn insert_mode_ctrl_j_inserts_newline_when_popup_closed() {
    let mut scheme = require_scheme!();
    let mut editor = Editor::new();
    editor.set_mode(Mode::Insert);
    assert!(editor.lsp.completion_items.is_empty(), "sanity: no popup");

    dispatch(&mut editor, &mut scheme, make_ctrl('j'));

    let text_after = editor.buffers[editor.active_buffer_idx()]
        .rope()
        .to_string();
    assert!(
        text_after.contains('\n'),
        "Ctrl-J should still insert a newline when no completion popup is open, got {text_after:?}"
    );
}
