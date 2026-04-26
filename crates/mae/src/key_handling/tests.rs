use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, Mode};
use mae_scheme::SchemeRuntime;

use crate::ai_event_handler::PendingInteractiveEvent;

use super::{handle_key, is_splash_visible};

fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn setup_splash() -> (Editor, SchemeRuntime) {
    let mut editor = Editor::new();
    editor.install_dashboard();
    let scheme = SchemeRuntime::new().unwrap();
    assert!(is_splash_visible(&editor));
    (editor, scheme)
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
    let (mut editor, mut scheme) = setup_splash();
    assert_eq!(editor.splash_selection, 0);

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 1);

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 2);

    // Wraps at action count
    let count = mae_renderer::splash_render::splash_action_count();
    editor.splash_selection = count - 1;
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    assert_eq!(editor.splash_selection, 0);
}

#[test]
fn splash_k_decrements_selection() {
    let (mut editor, mut scheme) = setup_splash();
    let count = mae_renderer::splash_render::splash_action_count();

    // From 0, k wraps to count-1
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('k')));
    assert_eq!(editor.splash_selection, count - 1);

    // From count-1, k goes to count-2
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('k')));
    assert_eq!(editor.splash_selection, count - 2);
}

#[test]
fn splash_enter_dispatches_command() {
    let (mut editor, mut scheme) = setup_splash();
    // Enter on first action should dispatch and potentially change state
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Enter));
    // After dispatch, the splash may or may not still be visible depending
    // on what the action does, but we verify no panic occurred.
}

#[test]
fn splash_only_intercepts_in_normal_mode() {
    let (mut editor, mut scheme) = setup_splash();
    editor.mode = Mode::Insert;
    let sel_before = editor.splash_selection;

    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    // In insert mode, j should NOT change splash_selection
    assert_eq!(editor.splash_selection, sel_before);
}

#[test]
fn splash_not_active_on_non_dashboard() {
    let mut editor = Editor::new();
    let mut scheme = SchemeRuntime::new().unwrap();
    // No dashboard installed — active buffer is a regular scratch buffer
    assert!(!is_splash_visible(&editor));

    let sel_before = editor.splash_selection;
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('j')));
    // j in normal mode on non-dashboard should not change splash_selection
    assert_eq!(editor.splash_selection, sel_before);
}

fn make_ctrl(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}

#[test]
fn ctrl_o_in_insert_mode_executes_one_normal_command_then_returns() {
    let mut editor = Editor::new();
    let mut scheme = SchemeRuntime::new().unwrap();

    // Enter insert mode
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));
    assert_eq!(editor.mode, Mode::Insert);

    // Type some text so we have content
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('h')));
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('i')));

    // C-o: switch to normal for one command
    dispatch(&mut editor, &mut scheme, make_ctrl('o'));
    assert_eq!(editor.mode, Mode::Normal);
    assert!(editor.insert_mode_oneshot_normal);

    // Execute one normal command (e.g. '0' = move to line start)
    // Note: '0' with no count_prefix is move-to-line-start, not a digit
    dispatch(&mut editor, &mut scheme, make_key(KeyCode::Char('0')));

    // Should be back in insert mode
    assert_eq!(editor.mode, Mode::Insert);
    assert!(!editor.insert_mode_oneshot_normal);
}

#[test]
fn ctrl_o_with_motion_returns_to_insert() {
    let mut editor = Editor::new();
    let mut scheme = SchemeRuntime::new().unwrap();

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
