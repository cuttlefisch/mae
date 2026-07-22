use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, Mode};

pub(super) fn handle_search_mode(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            editor.set_mode(Mode::Normal);
            editor.search_input.clear();
            editor.search_state.highlight_active = false;
        }
        KeyCode::Enter => {
            editor.set_mode(Mode::Normal);
            editor.execute_search();
        }
        KeyCode::Backspace => {
            if editor.search_input.is_empty() {
                editor.set_mode(Mode::Normal);
            } else {
                editor.search_input.pop();
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Readline-style clear-query (missing gap, no tracked issue).
            // Matches don't recompute until Enter, same as Backspace above.
            editor.search_input.clear();
        }
        KeyCode::Char(ch) => {
            editor.search_input.push(ch);
        }
        _ => {}
    }
}
